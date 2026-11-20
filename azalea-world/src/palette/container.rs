use std::{
    fmt::Debug,
    io::{self, Cursor, Write},
};

use azalea_block::BlockState;
use azalea_buf::{AzBuf, AzBufVar, BufReadError};
use azalea_core::{
    decode_errors::{DecodeErrorKind, record_decode_error},
    position::{ChunkSectionBiomePos, ChunkSectionBlockPos},
};
use azalea_registry::data::Biome;
use tracing::debug;

use super::{Palette, PaletteKind};
use crate::BitStorage;

#[derive(Clone, Debug, PartialEq)]
pub struct PalettedContainer<S: PalletedContainerKind> {
    pub bits_per_entry: u8,
    /// This is usually a list of unique values that appear in the container so
    /// they can be indexed by the bit storage.
    ///
    /// Sometimes it doesn't contain anything if there's too many unique items
    /// in the bit storage, though.
    pub palette: Palette<S>,
    /// Compacted list of indices pointing to entry IDs in the Palette.
    pub storage: BitStorage,
}

pub trait PalletedContainerKind:
    Copy + Clone + Debug + Default + PartialEq + TryFrom<u32> + Into<u32>
{
    type SectionPos: SectionPos;

    fn size_bits() -> usize;

    fn size() -> usize {
        1 << (Self::size_bits() * 3)
    }

    fn bits_per_entry_to_palette_kind(bits_per_entry: u8) -> PaletteKind;

    /// The number of bits per entry the storage really uses for a container
    /// sent (or resized) with `bits_per_entry`, as vanilla's
    /// `PalettedContainer.Strategy.getConfiguration`: the global palette uses
    /// `ceillog2` of the registry size whatever the sent value is.
    /// `registry_size` is the size of the registry when it's only known at
    /// runtime (biomes).
    fn storage_bits(bits_per_entry: u8, registry_size: Option<usize>) -> u8;
}

/// `Mth.ceillog2`
fn ceil_log2(n: usize) -> u8 {
    if n <= 1 {
        0
    } else {
        (usize::BITS - (n - 1).leading_zeros()) as u8
    }
}
impl PalletedContainerKind for BlockState {
    type SectionPos = ChunkSectionBlockPos;

    fn size_bits() -> usize {
        4
    }

    fn bits_per_entry_to_palette_kind(bits_per_entry: u8) -> PaletteKind {
        match bits_per_entry {
            0 => PaletteKind::SingleValue,
            1..=4 => PaletteKind::Linear,
            5..=8 => PaletteKind::Hashmap,
            _ => PaletteKind::Global,
        }
    }

    fn storage_bits(bits_per_entry: u8, _registry_size: Option<usize>) -> u8 {
        match bits_per_entry {
            0 => 0,
            1..=4 => 4,
            5..=8 => bits_per_entry,
            _ => ceil_log2(BlockState::MAX_STATE as usize + 1),
        }
    }
}
impl PalletedContainerKind for Biome {
    type SectionPos = ChunkSectionBiomePos;

    fn size_bits() -> usize {
        2
    }

    fn bits_per_entry_to_palette_kind(bits_per_entry: u8) -> PaletteKind {
        match bits_per_entry {
            0 => PaletteKind::SingleValue,
            1..=3 => PaletteKind::Linear,
            _ => PaletteKind::Global,
        }
    }

    fn storage_bits(bits_per_entry: u8, registry_size: Option<usize>) -> u8 {
        match bits_per_entry {
            0..=3 => bits_per_entry,
            // without the biome registry's size, the sent value (a vanilla
            // server sends ceillog2 of its registry size); read() refuses
            // more than 32
            _ => registry_size.map_or(bits_per_entry, ceil_log2),
        }
    }
}

/// A trait for position types that are sometimes valid ways to index into a
/// chunk section.
pub trait SectionPos {
    fn coords(&self) -> (usize, usize, usize);
    fn new(x: usize, y: usize, z: usize) -> Self;
}
impl SectionPos for ChunkSectionBlockPos {
    fn coords(&self) -> (usize, usize, usize) {
        (self.x as usize, self.y as usize, self.z as usize)
    }

    fn new(x: usize, y: usize, z: usize) -> Self {
        ChunkSectionBlockPos {
            x: x as u8,
            y: y as u8,
            z: z as u8,
        }
    }
}
impl SectionPos for ChunkSectionBiomePos {
    fn coords(&self) -> (usize, usize, usize) {
        (self.x as usize, self.y as usize, self.z as usize)
    }

    fn new(x: usize, y: usize, z: usize) -> Self {
        ChunkSectionBiomePos {
            x: x as u8,
            y: y as u8,
            z: z as u8,
        }
    }
}

impl<S: PalletedContainerKind> PalettedContainer<S> {
    pub fn new() -> Self {
        let palette = Palette::SingleValue(S::default());
        let size = S::size();
        // network-safe: empty data with 0 bits is always valid
        let storage = BitStorage::new(0, size, Some(Box::new([]))).unwrap();

        PalettedContainer {
            bits_per_entry: 0,
            palette,
            storage,
        }
    }

    pub fn read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        Self::read_with_registry_size(buf, None)
    }

    /// Read a container as vanilla's `PalettedContainer.read`: the palette
    /// kind and storage size follow the sent bits per entry through
    /// [`PalletedContainerKind::storage_bits`], palette entries must be valid
    /// ids (`byIdOrThrow`), a linear palette holds at most 2^bits entries,
    /// and a long array of the wrong length is read and ignored (counted as
    /// [`DecodeErrorKind::Chunk`]).
    pub fn read_with_registry_size(
        buf: &mut Cursor<&[u8]>,
        registry_size: Option<usize>,
    ) -> Result<Self, BufReadError> {
        let sent_bits = u8::azalea_read(buf)?;
        let palette_type = S::bits_per_entry_to_palette_kind(sent_bits);
        let bits_per_entry = S::storage_bits(sent_bits, registry_size);
        if bits_per_entry > 32 {
            return Err(BufReadError::Custom(format!(
                "Paletted container with {bits_per_entry} bits per entry"
            )));
        }
        let palette = palette_type.read(buf)?;
        if let Palette::Linear(values) = &palette
            && values.len() > 1 << bits_per_entry
        {
            // LinearPalette.read writes into an array of 2^bits
            return Err(BufReadError::Custom(format!(
                "Linear palette with {} entries for {bits_per_entry} bits",
                values.len()
            )));
        }
        let size = S::size();

        let mut storage = BitStorage::new(
            bits_per_entry as usize,
            size,
            if bits_per_entry == 0 {
                Some(Box::new([]))
            } else {
                // we're going to update the data after creating the bitstorage
                None
            },
        )
        .map_err(|e| BufReadError::Custom(format!("Failed to create bit storage: {e:?}")))?;

        // now read the data. 1.21.4 prefixes the long array with its length
        // (PalettedContainer.read -> FriendlyByteBuf.readLongArray(long[])). if
        // the length doesn't match the storage we expect, vanilla reads the
        // longs into a new array and throws it away, so the storage stays
        // zeroed. we do the same.
        let data_len = azalea_buf::limits::read_collection_len(buf, 8)?;
        if data_len == storage.data.len() {
            for word in storage.data.iter_mut() {
                *word = u64::azalea_read(buf)?;
            }
        } else {
            debug!(
                "Paletted container data length {data_len} doesn't match expected {}, ignoring the data",
                storage.data.len()
            );
            record_decode_error(DecodeErrorKind::Chunk);
            // network-safe: read_collection_len checked that data_len * 8
            // bytes are left
            buf.set_position(buf.position() + data_len as u64 * 8);
        }

        Ok(PalettedContainer {
            bits_per_entry,
            palette,
            storage,
        })
    }

    /// Calculates the index of the given position.
    pub fn index_from_pos(&self, pos: S::SectionPos) -> usize {
        let size_bits = S::size_bits();
        let (x, y, z) = pos.coords();
        (((y << size_bits) | z) << size_bits) | x
    }

    pub fn pos_from_index(&self, index: usize) -> S::SectionPos {
        let size_bits = S::size_bits();
        let mask = (1 << size_bits) - 1;
        S::SectionPos::new(
            index & mask,
            (index >> size_bits >> size_bits) & mask,
            (index >> size_bits) & mask,
        )
    }

    /// Returns the value at the given index.
    ///
    /// # Panics
    ///
    /// This function panics if the index is greater than or equal to the number
    /// of things in the storage. For example, for block states, it must be less
    /// than 4096.
    #[inline]
    pub fn get_at_index(&self, index: usize) -> S {
        // first get the palette id
        let paletted_value = self.storage.get(index);
        // and then get the value from that id
        self.palette.value_for(paletted_value as usize)
    }

    /// Returns the value at the given position.
    pub fn get(&self, pos: S::SectionPos) -> S {
        self.get_at_index(self.index_from_pos(pos))
    }

    /// Sets the ID at the given position and return the previous ID.
    pub fn get_and_set(&mut self, pos: S::SectionPos, value: S) -> S {
        let paletted_value = self.id_for(value);
        let old_paletted_value = self
            .storage
            .get_and_set(self.index_from_pos(pos), paletted_value as u64);
        self.palette.value_for(old_paletted_value as usize)
    }

    /// Sets the ID at the given index and return the previous ID. You probably
    /// want `.set` instead.
    pub fn set_at_index(&mut self, index: usize, value: S) {
        let paletted_value = self.id_for(value);
        self.storage.set(index, paletted_value as u64);
    }

    /// Sets the ID at the given position and return the previous ID.
    pub fn set(&mut self, pos: S::SectionPos, value: S) {
        self.set_at_index(self.index_from_pos(pos), value);
    }

    fn create_or_reuse_data(&self, bits_per_entry: u8) -> PalettedContainer<S> {
        let new_palette_type = S::bits_per_entry_to_palette_kind(bits_per_entry);

        let old_palette_type = (&self.palette).into();
        if S::storage_bits(bits_per_entry, None) == self.bits_per_entry
            && new_palette_type == old_palette_type
        {
            return self.clone();
        }
        let storage_bits = S::storage_bits(bits_per_entry, None);
        let storage = BitStorage::new(
            storage_bits as usize,
            S::size(),
            (storage_bits == 0).then(|| Box::new([]) as Box<[u64]>),
        )
        // network-safe: storage_bits is at most 32 (biomes past 3 bits only
        // get here through a container read with at most 32) and S::size()
        // fits BitStorage
        .expect("valid bit storage");

        // sanity check
        debug_assert_eq!(storage.size(), S::size());

        // let palette = new_palette_type.as_empty_palette(1usize <<
        // (bits_per_entry as usize));
        let palette = new_palette_type.as_empty_palette();
        PalettedContainer {
            bits_per_entry: storage_bits,
            palette,
            storage,
        }
    }

    fn on_resize(&mut self, bits_per_entry: u8, value: S) -> usize {
        debug!(
            "Resizing PalettedContainer from {} bpe to {bits_per_entry} for {value:?} with palette={:?}",
            self.bits_per_entry, self.palette
        );
        // in vanilla this is always true, but it's sometimes false in purpur
        // servers
        // ```
        // assert!(bits_per_entry <= 5, "bits_per_entry must be <= 5");
        // ```
        let mut new_data = self.create_or_reuse_data(bits_per_entry);
        new_data.copy_from(&self.palette, &self.storage);
        *self = new_data;
        self.id_for(value)
    }

    fn copy_from(&mut self, palette: &Palette<S>, storage: &BitStorage) {
        for i in 0..storage.size() {
            let value = palette.value_for(storage.get(i) as usize);
            let id = self.id_for(value) as u64;
            self.storage.set(i, id);
        }
    }

    pub fn id_for(&mut self, value: S) -> usize {
        match &mut self.palette {
            Palette::SingleValue(v) => {
                if (*v).into() != value.into() {
                    self.on_resize(1, value)
                } else {
                    0
                }
            }
            Palette::Linear(palette) => {
                if let Some(index) = palette.iter().position(|&v| v.into() == value.into()) {
                    return index;
                }
                let capacity = 2usize.pow(self.bits_per_entry.into());
                if capacity > palette.len() {
                    palette.push(value);
                    palette.len() - 1
                } else {
                    self.on_resize(self.bits_per_entry + 1, value)
                }
            }
            Palette::Hashmap(palette) => {
                // TODO? vanilla keeps this in memory as a hashmap, but it
                // should be benchmarked before changing it
                if let Some(index) = palette.iter().position(|v| (*v).into() == value.into()) {
                    return index;
                }
                let capacity = 2usize.pow(self.bits_per_entry.into());
                if capacity > palette.len() {
                    palette.push(value);
                    palette.len() - 1
                } else {
                    self.on_resize(self.bits_per_entry + 1, value)
                }
            }
            Palette::Global => value.into() as usize,
        }
    }
}

impl<S: PalletedContainerKind> PalettedContainer<S> {
    pub fn write(&self, buf: &mut impl Write) -> io::Result<()> {
        self.bits_per_entry.azalea_write(buf)?;
        self.palette.write(buf)?;
        // 1.21.4 writes the length of the long array before it
        (self.storage.data.len() as u32).azalea_write_var(buf)?;
        for word in &self.storage.data {
            word.azalea_write(buf)?;
        }

        Ok(())
    }
}

impl<S: PalletedContainerKind> Default for PalettedContainer<S> {
    fn default() -> Self {
        Self::new()
    }
}
