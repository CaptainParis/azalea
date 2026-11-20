use std::{
    fmt::{self, Debug},
    io::Cursor,
    sync::Arc,
};

use azalea_block::BlockState;
use azalea_buf::BufReadError;
use azalea_core::{
    heightmap_kind::HeightmapKind,
    position::{BlockPos, ChunkBlockPos, ChunkPos},
};
use parking_lot::RwLock;
use tracing::{debug, trace, warn};

use crate::{Chunk, chunk::storage::ChunkStorage};

/// An efficient storage of chunks for a client that has a limited render
/// distance.
///
/// This has support for using a shared [`ChunkStorage`].
pub struct PartialChunkStorage {
    /// The center of the view, i.e. the chunk the player is currently in.
    view_center: ChunkPos,
    pub(crate) chunk_radius: u32,
    view_range: u32,
    // chunks is a list of size view_range * view_range
    chunks: Box<[Option<Arc<RwLock<Chunk>>>]>,
    /// The position of the chunk in each slot of `chunks` (a slot is shared
    /// by every position with the same coordinates modulo `view_range`).
    positions: Box<[Option<ChunkPos>]>,
}

impl PartialChunkStorage {
    pub fn new(chunk_radius: u32) -> Self {
        let view_range = chunk_radius * 2 + 1;
        PartialChunkStorage {
            view_center: ChunkPos::new(0, 0),
            chunk_radius,
            view_range,
            chunks: vec![None; (view_range * view_range) as usize].into(),
            positions: vec![None; (view_range * view_range) as usize].into(),
        }
    }

    /// The most chunks this storage can hold: `view_range²`.
    pub fn capacity(&self) -> usize {
        self.chunks.len()
    }

    /// The radius of the ring in chunks (the client view distance plus 3,
    /// see [`calculate_chunk_storage_range`](super::calculate_chunk_storage_range)).
    pub fn chunk_radius(&self) -> u32 {
        self.chunk_radius
    }

    /// How many slots hold a chunk (stale slots included: chunks the centre
    /// moved away from stay until a chunk for the same slot replaces them or
    /// the server forgets them).
    pub fn loaded_count(&self) -> usize {
        self.chunks.iter().filter(|c| c.is_some()).count()
    }

    /// The positions of the chunks held, stale slots included.
    pub fn loaded_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks
            .iter()
            .zip(self.positions.iter())
            .filter_map(|(c, p)| c.as_ref().and(*p))
    }

    /// Update the chunk to center the view on.
    ///
    /// This should be called when the client receives a `SetChunkCacheCenter`
    /// packet.
    pub fn update_view_center(&mut self, view_center: ChunkPos) {
        // this code block makes it force unload the chunks that are out of
        // range after updating the view center. it's usually fine without it
        // but the commented code is there in case you want to temporarily
        // uncomment to test something

        // ```
        // for index in 0..self.chunks.len() {
        //     let chunk_pos = self.chunk_pos_from_index(index);
        //     if !in_range_for_view_center_and_radius(&chunk_pos, view_center, self.chunk_radius) {
        //         self.chunks[index] = None;
        //     }
        // }
        // ```

        self.view_center = view_center;
    }

    /// Get the center of the view. This is usually the chunk that the player is
    /// in.
    pub fn view_center(&self) -> ChunkPos {
        self.view_center
    }

    pub fn view_range(&self) -> u32 {
        self.view_range
    }

    pub fn index_from_chunk_pos(&self, chunk_pos: &ChunkPos) -> usize {
        let view_range = self.view_range as i32;

        let x = i32::rem_euclid(chunk_pos.x, view_range) * view_range;
        let z = i32::rem_euclid(chunk_pos.z, view_range);
        (x + z) as usize
    }

    pub fn chunk_pos_from_index(&self, index: usize) -> ChunkPos {
        let view_range = self.view_range as i32;

        // find the base from the view center
        let base_x = self.view_center.x.div_euclid(view_range) * view_range;
        let base_z = self.view_center.z.div_euclid(view_range) * view_range;

        // add the offset from the base
        // network-safe: view_range is radius * 2 + 1 >= 1 (the radius comes
        // from our own view distance, a u8)
        let offset_x = index as i32 / view_range;
        // network-safe: view_range >= 1, as above
        let offset_z = index as i32 % view_range;

        ChunkPos::new(base_x + offset_x, base_z + offset_z)
    }

    pub fn in_range(&self, chunk_pos: &ChunkPos) -> bool {
        in_range_for_view_center_and_radius(chunk_pos, self.view_center, self.chunk_radius)
    }

    pub fn set_block_state(
        &self,
        pos: BlockPos,
        state: BlockState,
        chunk_storage: &ChunkStorage,
    ) -> Option<BlockState> {
        if pos.y < chunk_storage.min_y()
            || pos.y >= (chunk_storage.min_y() + chunk_storage.height() as i32)
        {
            return None;
        }
        let chunk_pos = ChunkPos::from(pos);
        let chunk_lock = chunk_storage.get(&chunk_pos)?;
        let mut chunk = chunk_lock.write();
        Some(chunk.get_and_set_block_state(&ChunkBlockPos::from(pos), state, chunk_storage.min_y()))
    }

    pub fn replace_with_packet_data(
        &mut self,
        pos: &ChunkPos,
        data: &mut Cursor<&[u8]>,
        heightmaps: &[(HeightmapKind, Box<[u64]>)],
        chunk_storage: &mut ChunkStorage,
    ) -> Result<(), BufReadError> {
        self.replace_with_packet_data_and_biome_registry_size(
            pos,
            data,
            heightmaps,
            chunk_storage,
            None,
        )
    }

    /// [`Self::replace_with_packet_data`] with the size of the biome
    /// registry (see [`Chunk::read_with_dimension`]).
    pub fn replace_with_packet_data_and_biome_registry_size(
        &mut self,
        pos: &ChunkPos,
        data: &mut Cursor<&[u8]>,
        heightmaps: &[(HeightmapKind, Box<[u64]>)],
        chunk_storage: &mut ChunkStorage,
        biome_registry_size: Option<usize>,
    ) -> Result<(), BufReadError> {
        debug!("Replacing chunk at {:?}", pos);
        if !self.in_range(pos) {
            warn!("Ignoring chunk since it's not in the view range: {pos:?}");
            return Ok(());
        }

        let chunk = Chunk::read_with_dimension(
            data,
            chunk_storage.height(),
            chunk_storage.min_y(),
            heightmaps,
            biome_registry_size,
        )?;

        self.set(pos, Some(chunk), chunk_storage);
        trace!("Loaded chunk {pos:?}");

        Ok(())
    }

    /// Returns whether the given chunk is currently loaded (which also requires
    /// to be in range).
    pub fn has(&self, pos: &ChunkPos) -> bool {
        self.limited_get(pos).is_some()
    }

    /// Get a [`Chunk`] within render distance, or `None` if it's not loaded.
    /// Use [`ChunkStorageTrait::get`] to get a chunk from the shared storage.
    ///
    /// [`ChunkStorageTrait::get`]: crate::chunk::storage::ChunkStorageTrait::get
    pub fn limited_get(&self, pos: &ChunkPos) -> Option<&Arc<RwLock<Chunk>>> {
        if !self.in_range(pos) {
            warn!(
                "Chunk at {:?} is not in the render distance (center: {:?}, {} chunks)",
                pos, self.view_center, self.chunk_radius,
            );
            return None;
        }

        let index = self.index_from_chunk_pos(pos);
        // a slot holding another position's (stale) chunk doesn't hold this one
        // network-safe: index_from_chunk_pos is below view_range^2, the length
        if self.positions[index] != Some(*pos) {
            return None;
        }
        // network-safe: index_from_chunk_pos is below view_range^2, the length
        self.chunks[index].as_ref()
    }
    /// Get a mutable reference to a loaded [`Chunk`] within render distance,
    /// or `None` if it's not loaded.
    ///
    /// Use [`ChunkStorageTrait::get`] to get a chunk from the shared storage.
    ///
    /// [`ChunkStorageTrait::get`]: crate::chunk::storage::ChunkStorageTrait::get
    ///
    /// Only for a slot that holds this position's chunk; use
    /// [`Self::limited_set`] to put a chunk in a slot.
    pub fn limited_get_mut(&mut self, pos: &ChunkPos) -> Option<&mut Arc<RwLock<Chunk>>> {
        if !self.in_range(pos) {
            return None;
        }

        let index = self.index_from_chunk_pos(pos);
        // network-safe: index_from_chunk_pos is below view_range^2, the length
        if self.positions[index] != Some(*pos) {
            return None;
        }
        // network-safe: index_from_chunk_pos is below view_range^2, the length
        self.chunks[index].as_mut()
    }

    /// Set a chunk in the shared storage and reference it from the limited
    /// storage.
    ///
    /// Use [`Self::limited_set`] if you already have an `Arc<RwLock<Chunk>>`.
    ///
    /// A chunk out of the ring's range isn't stored. The chunk this replaces
    /// in its slot (the same position, or a stale one the slot held) is
    /// pruned from `chunk_storage` once nothing else holds it.
    pub fn set(&mut self, pos: &ChunkPos, chunk: Option<Chunk>, chunk_storage: &mut ChunkStorage) {
        if !self.in_range(pos) {
            return;
        }
        let new_chunk = chunk.map(|c| chunk_storage.upsert(*pos, c));
        self.limited_set_and_prune(pos, new_chunk, chunk_storage);
    }

    /// Set a chunk in our limited storage, useful if your chunk is already
    /// referenced somewhere else and you want to make it also be referenced by
    /// this storage.
    ///
    /// Use [`Self::set`] if you don't already have an `Arc<RwLock<Chunk>>`.
    ///
    /// Does nothing if the chunk is not in the render distance. Returns the
    /// position of the chunk the slot held before, if it held one (the same
    /// position, or a stale one), so the caller can prune it from the shared
    /// storage ([`ChunkStorageTrait::prune`]), or use
    /// [`Self::limited_set_and_prune`].
    ///
    /// [`ChunkStorageTrait::prune`]: crate::chunk::storage::ChunkStorageTrait::prune
    pub fn limited_set(
        &mut self,
        pos: &ChunkPos,
        chunk: Option<Arc<RwLock<Chunk>>>,
    ) -> Option<ChunkPos> {
        if !self.in_range(pos) {
            return None;
        }
        let index = self.index_from_chunk_pos(pos);
        let new_pos = chunk.as_ref().map(|_| *pos);
        // network-safe: index_from_chunk_pos is below view_range^2, the length
        let old = std::mem::replace(&mut self.chunks[index], chunk);
        // network-safe: index_from_chunk_pos is below view_range^2, the length
        let old_pos = std::mem::replace(&mut self.positions[index], new_pos);
        // the old Arc is dropped here, before the caller prunes
        drop(old?);
        old_pos
    }

    /// [`Self::limited_set`], then prune the replaced chunk from the shared
    /// storage if nothing else holds it.
    pub fn limited_set_and_prune(
        &mut self,
        pos: &ChunkPos,
        chunk: Option<Arc<RwLock<Chunk>>>,
        chunk_storage: &mut ChunkStorage,
    ) {
        if let Some(old) = self.limited_set(pos, chunk) {
            chunk_storage.prune(&old);
        }
    }

    /// Get an iterator over all the chunks in the storage.
    pub fn chunks(&self) -> impl Iterator<Item = &Option<Arc<RwLock<Chunk>>>> {
        self.chunks.iter()
    }
}

impl Debug for PartialChunkStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PartialChunkStorage")
            .field("view_center", &self.view_center)
            .field("chunk_radius", &self.chunk_radius)
            .field("view_range", &self.view_range)
            // .field("chunks", &self.chunks)
            .field("chunks", &format_args!("{} items", self.chunks.len()))
            .finish()
    }
}

impl Default for PartialChunkStorage {
    fn default() -> Self {
        Self::new(8)
    }
}

pub fn in_range_for_view_center_and_radius(
    chunk_pos: &ChunkPos,
    view_center: ChunkPos,
    chunk_radius: u32,
) -> bool {
    (chunk_pos.x - view_center.x).unsigned_abs() <= chunk_radius
        && (chunk_pos.z - view_center.z).unsigned_abs() <= chunk_radius
}

#[cfg(test)]
mod tests {
    use azalea_core::position::ChunkPos;

    use crate::{
        Chunk,
        chunk::{
            calculate_chunk_storage_range, partial::PartialChunkStorage, storage::ChunkStorage,
        },
    };

    #[test]
    fn test_chunk_pos_from_index() {
        let mut partial_chunk_storage = PartialChunkStorage::new(5);
        partial_chunk_storage.update_view_center(ChunkPos::new(0, -1));
        assert_eq!(
            partial_chunk_storage.chunk_pos_from_index(
                partial_chunk_storage.index_from_chunk_pos(&ChunkPos::new(2, -1))
            ),
            ChunkPos::new(2, -1),
        );
    }

    /// WP4: the ring for client view distance 7 is radius 10, 441 chunks.
    #[test]
    fn cap_formula() {
        assert_eq!(calculate_chunk_storage_range(7), 10);
        let ring = PartialChunkStorage::new(calculate_chunk_storage_range(7));
        assert_eq!(ring.chunk_radius(), 10);
        assert_eq!(ring.view_range(), 21);
        assert_eq!(ring.capacity(), 441);
        // Azalea's default view distance 8 makes it radius 11, 529
        assert_eq!(
            PartialChunkStorage::new(calculate_chunk_storage_range(8)).capacity(),
            529
        );
    }

    /// Every slot full: the count is the capacity, never more; a chunk out of
    /// range is refused.
    #[test]
    fn full_ring_is_the_cap() {
        let mut storage = ChunkStorage::default();
        let mut ring = PartialChunkStorage::new(10);
        for x in -10..=10 {
            for z in -10..=10 {
                ring.set(&ChunkPos::new(x, z), Some(Chunk::default()), &mut storage);
            }
        }
        assert_eq!(ring.loaded_count(), 441);
        assert_eq!(storage.len(), 441);
        ring.set(&ChunkPos::new(11, 0), Some(Chunk::default()), &mut storage);
        assert_eq!(ring.loaded_count(), 441);
        assert_eq!(storage.len(), 441);
        assert!(!ring.has(&ChunkPos::new(11, 0)));
    }

    /// Forgetting a chunk drops its weak map entry at once.
    #[test]
    fn forget_prunes_the_weak_entry() {
        let mut storage = ChunkStorage::default();
        let mut ring = PartialChunkStorage::new(10);
        let pos = ChunkPos::new(3, -2);
        ring.set(&pos, Some(Chunk::default()), &mut storage);
        assert_eq!(storage.len(), 1);
        ring.limited_set_and_prune(&pos, None, &mut storage);
        assert_eq!(storage.len(), 0);
        assert!(storage.get(&pos).is_none());
        assert_eq!(ring.loaded_count(), 0);
    }

    /// The centre moves on: the old chunk stays in its slot (stale, counted)
    /// until a chunk for the same slot arrives, which evicts it and prunes
    /// its weak map entry; a stale slot never answers for the new position.
    #[test]
    fn centre_move_keeps_stale_slots_until_replaced() {
        let mut storage = ChunkStorage::default();
        let mut ring = PartialChunkStorage::new(10);
        let old = ChunkPos::new(-10, 0);
        ring.set(&old, Some(Chunk::default()), &mut storage);
        // 21 chunks east: old is out of range, its slot is the new pos's
        ring.update_view_center(ChunkPos::new(21, 0));
        let new = ChunkPos::new(11, 0);
        assert_eq!(
            ring.index_from_chunk_pos(&old),
            ring.index_from_chunk_pos(&new)
        );
        assert_eq!(ring.loaded_count(), 1, "stale slot counts");
        assert!(
            ring.limited_get(&new).is_none(),
            "stale slot isn't the new chunk"
        );
        assert!(storage.get(&old).is_some(), "still held by the stale slot");
        ring.set(&new, Some(Chunk::default()), &mut storage);
        assert_eq!(ring.loaded_count(), 1);
        assert!(storage.get(&old).is_none());
        assert_eq!(storage.len(), 1, "evicted entry pruned");
        assert!(ring.limited_get(&new).is_some());
        assert_eq!(ring.loaded_positions().collect::<Vec<_>>(), vec![new]);
    }

    /// Eviction order: a slot's chunk goes when a chunk for the same slot
    /// arrives; nothing else is evicted, so the chunk under the client (the
    /// centre's own slot) only goes when its own slot is reused, which takes
    /// a chunk 21 away from it, out of range of the centre.
    #[test]
    fn eviction_is_by_slot_only() {
        let mut storage = ChunkStorage::default();
        let mut ring = PartialChunkStorage::new(10);
        let centre = ChunkPos::new(0, 0);
        ring.set(&centre, Some(Chunk::default()), &mut storage);
        for x in 1..=10 {
            ring.set(&ChunkPos::new(x, 0), Some(Chunk::default()), &mut storage);
        }
        assert!(ring.limited_get(&centre).is_some());
        // same slot as the centre, but out of range of it: refused
        ring.set(&ChunkPos::new(21, 0), Some(Chunk::default()), &mut storage);
        assert!(ring.limited_get(&centre).is_some());
        assert_eq!(storage.len(), 11);
    }

    /// The same position again replaces the chunk in place (one entry).
    #[test]
    fn same_position_again_is_one_entry() {
        let mut storage = ChunkStorage::default();
        let mut ring = PartialChunkStorage::new(10);
        let pos = ChunkPos::new(1, 1);
        for _ in 0..5 {
            ring.set(&pos, Some(Chunk::default()), &mut storage);
        }
        assert_eq!(storage.len(), 1);
        assert_eq!(ring.loaded_count(), 1);
    }

    /// The sweep forgets entries whose chunk another holder dropped later.
    #[test]
    fn sweep_forgets_dead_entries() {
        let mut storage = ChunkStorage::default();
        let mut ring = PartialChunkStorage::new(10);
        let pos = ChunkPos::new(0, 0);
        ring.set(&pos, Some(Chunk::default()), &mut storage);
        let held = storage.get(&pos).unwrap();
        ring.limited_set_and_prune(&pos, None, &mut storage);
        // still held elsewhere: not pruned
        assert_eq!(storage.len(), 1);
        drop(held);
        assert_eq!(storage.len(), 1);
        assert_eq!(storage.sweep(), 1);
        assert_eq!(storage.len(), 0);
    }
}
