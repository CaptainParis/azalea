//! Light data kept per chunk, as the server sent it (azalea-hunt WP4).
//!
//! Azalea's physics doesn't use light; it's kept so a proxy can replay a
//! chunk to an attached client exactly as received. Each chunk has one entry
//! per light section (the world's sections plus one below and one above, as
//! vanilla's `LevelLightEngine::getLightSectionCount`) for sky light and for
//! block light. A section sent all 0 or all 15 (common for sky light above the
//! terrain and for empty sections) is stored without its bytes; any other
//! section keeps its 2048 received bytes. The data goes with its chunk.

use azalea_core::bitset::BitSet;

/// Bytes of one light section (4 bits for each of 16³ blocks).
pub const LIGHT_SECTION_BYTES: usize = 2048;

/// One light section of one layer.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum LightSection {
    /// Never sent for this chunk.
    #[default]
    Missing,
    /// Sent in the empty mask (vanilla: a new, all-0 `DataLayer`).
    Empty,
    /// Sent with every byte 0.
    Zero,
    /// Sent with every byte 0xFF (light 15 everywhere).
    Full,
    /// Sent with other bytes, kept as received.
    Data(Box<[u8; LIGHT_SECTION_BYTES]>),
}

impl LightSection {
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let arr: &[u8; LIGHT_SECTION_BYTES] = bytes.try_into().ok()?;
        Some(if arr.iter().all(|b| *b == 0) {
            LightSection::Zero
        } else if arr.iter().all(|b| *b == 0xFF) {
            LightSection::Full
        } else {
            LightSection::Data(Box::new(*arr))
        })
    }

    /// The section's bytes as the server sent them (`None` if it was never
    /// sent or sent in the empty mask).
    pub fn bytes(&self) -> Option<[u8; LIGHT_SECTION_BYTES]> {
        match self {
            LightSection::Missing | LightSection::Empty => None,
            LightSection::Zero => Some([0; LIGHT_SECTION_BYTES]),
            LightSection::Full => Some([0xFF; LIGHT_SECTION_BYTES]),
            LightSection::Data(d) => Some(**d),
        }
    }
}

/// Which light layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LightLayer {
    Sky,
    Block,
}

/// A chunk's light, both layers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChunkLight {
    pub sky: Box<[LightSection]>,
    pub block: Box<[LightSection]>,
}

/// The light data of one packet couldn't all be used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LightDataError {
    /// Sections whose array wasn't 2048 bytes or was missing (the mask had
    /// more bits than the packet had arrays); those sections are unchanged.
    pub bad_sections: usize,
}

impl ChunkLight {
    /// Light for a chunk of `world_sections` sections (`world_sections + 2`
    /// light sections per layer), nothing sent yet.
    pub fn new(world_sections: usize) -> Self {
        let n = world_sections + 2;
        Self {
            sky: vec![LightSection::Missing; n].into(),
            block: vec![LightSection::Missing; n].into(),
        }
    }

    /// Apply one layer of a light packet the way vanilla's
    /// `ClientPacketListener.readSectionList` does: for each light section
    /// `k`, a set bit in `mask` takes the next array, a set bit in `empty`
    /// marks it empty, neither leaves it as it was. Bits past the chunk's
    /// light sections are ignored.
    pub fn apply(
        &mut self,
        layer: LightLayer,
        mask: &BitSet,
        empty: &BitSet,
        arrays: &[Box<[u8]>],
    ) -> Result<(), LightDataError> {
        let sections = match layer {
            LightLayer::Sky => &mut self.sky,
            LightLayer::Block => &mut self.block,
        };
        let mut arrays = arrays.iter();
        let mut bad = 0;
        for (k, section) in sections.iter_mut().enumerate() {
            if mask.index(k) {
                match arrays.next().and_then(|a| LightSection::from_bytes(a)) {
                    Some(s) => *section = s,
                    None => bad += 1,
                }
            } else if empty.index(k) {
                *section = LightSection::Empty;
            }
        }
        if bad == 0 {
            Ok(())
        } else {
            Err(LightDataError { bad_sections: bad })
        }
    }

    /// Heap bytes this light holds: the section entries and the received
    /// arrays.
    pub fn heap_bytes(&self) -> usize {
        let entries = (self.sky.len() + self.block.len()) * size_of::<LightSection>();
        entries + self.data_sections() * LIGHT_SECTION_BYTES
    }

    /// Sections that keep their bytes (neither all 0 nor all 15).
    pub fn data_sections(&self) -> usize {
        self.sky
            .iter()
            .chain(self.block.iter())
            .filter(|s| matches!(s, LightSection::Data(_)))
            .count()
    }

    /// The most heap bytes one chunk's light can hold (every section of both
    /// layers sent with mixed data): about 106 KB for 24 world sections.
    pub fn worst_case_bytes(world_sections: usize) -> usize {
        let n = world_sections + 2;
        2 * n * (size_of::<LightSection>() + LIGHT_SECTION_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(bits: &[usize]) -> BitSet {
        let mut b = BitSet::new(64);
        for &i in bits {
            b.set(i);
        }
        b
    }

    fn arr(fill: u8) -> Box<[u8]> {
        vec![fill; LIGHT_SECTION_BYTES].into()
    }

    #[test]
    fn shared_sections_hold_no_bytes() {
        let mut l = ChunkLight::new(24);
        assert_eq!(l.sky.len(), 26);
        let mut mixed = vec![0u8; LIGHT_SECTION_BYTES];
        mixed[7] = 0x3F;
        l.apply(
            LightLayer::Sky,
            &mask(&[0, 1, 2]),
            &mask(&[5]),
            &[arr(0), arr(0xFF), mixed.clone().into()],
        )
        .unwrap();
        assert_eq!(l.sky[0], LightSection::Zero);
        assert_eq!(l.sky[1], LightSection::Full);
        assert!(matches!(l.sky[2], LightSection::Data(_)));
        assert_eq!(l.sky[3], LightSection::Missing);
        assert_eq!(l.sky[5], LightSection::Empty);
        assert_eq!(l.data_sections(), 1);
        assert_eq!(l.sky[2].bytes().unwrap().to_vec(), mixed);
        assert_eq!(l.sky[1].bytes().unwrap(), [0xFF; LIGHT_SECTION_BYTES]);
        let entries = 52 * size_of::<LightSection>();
        assert_eq!(l.heap_bytes(), entries + LIGHT_SECTION_BYTES);
    }

    #[test]
    fn update_changes_only_the_sections_it_names() {
        let mut l = ChunkLight::new(24);
        l.apply(
            LightLayer::Block,
            &mask(&[3, 4]),
            &BitSet::new(0),
            &[arr(1), arr(2)],
        )
        .unwrap();
        l.apply(LightLayer::Block, &mask(&[4]), &mask(&[3]), &[arr(0)])
            .unwrap();
        assert_eq!(l.block[3], LightSection::Empty);
        assert_eq!(l.block[4], LightSection::Zero);
        assert_eq!(l.data_sections(), 0);
        assert!(l.sky.iter().all(|s| *s == LightSection::Missing));
    }

    #[test]
    fn bad_arrays_leave_sections_unchanged_and_bits_past_the_end_are_ignored() {
        let mut l = ChunkLight::new(24);
        let short: Box<[u8]> = vec![1; 100].into();
        let err = l
            .apply(
                LightLayer::Sky,
                &mask(&[0, 1, 2, 40]),
                &BitSet::new(0),
                &[short, arr(1)],
            )
            .unwrap_err();
        // section 0's array is short, section 2 has none; bit 40 is past 26
        assert_eq!(err.bad_sections, 2);
        assert_eq!(l.sky[0], LightSection::Missing);
        assert!(matches!(l.sky[1], LightSection::Data(_)));
        assert_eq!(l.sky[2], LightSection::Missing);
    }

    #[test]
    fn worst_case_is_about_106_kb() {
        let mut l = ChunkLight::new(24);
        let all: Vec<usize> = (0..26).collect();
        let arrays: Vec<Box<[u8]>> = (0..26).map(|_| arr(0x12)).collect();
        l.apply(LightLayer::Sky, &mask(&all), &BitSet::new(0), &arrays)
            .unwrap();
        l.apply(LightLayer::Block, &mask(&all), &BitSet::new(0), &arrays)
            .unwrap();
        assert_eq!(l.heap_bytes(), ChunkLight::worst_case_bytes(24));
        assert!(l.heap_bytes() <= 106 * 1024 + 2 * 26 * size_of::<LightSection>());
        assert_eq!(l.data_sections(), 52);
    }
}
