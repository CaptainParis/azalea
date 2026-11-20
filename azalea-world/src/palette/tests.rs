use azalea_block::BlockState;
use azalea_core::position::ChunkSectionBlockPos;

use super::*;

#[test]
fn test_resize_0_bits_to_1() {
    let mut palette_container = PalettedContainer::<BlockState>::new();

    assert_eq!(palette_container.bits_per_entry, 0);
    assert_eq!(palette_container.get_at_index(0), BlockState::AIR);
    assert_eq!(
        PaletteKind::from(&palette_container.palette),
        PaletteKind::SingleValue
    );
    let block_state_1 = BlockState::try_from(1_u32).unwrap();
    palette_container.set_at_index(0, block_state_1);
    assert_eq!(palette_container.get_at_index(0), block_state_1);
    assert_eq!(
        PaletteKind::from(&palette_container.palette),
        PaletteKind::Linear
    );
}

#[test]
fn test_resize_0_bits_to_5() {
    let mut palette_container = PalettedContainer::<BlockState>::new();

    let set = |pc: &mut PalettedContainer<BlockState>, i, v: u32| {
        pc.set_at_index(i, BlockState::try_from(v).unwrap());
    };

    set(&mut palette_container, 0, 0); // 0 bits
    assert_eq!(palette_container.bits_per_entry, 0);

    // block states: vanilla's linear palette always uses 4 bits
    set(&mut palette_container, 1, 1);
    assert_eq!(palette_container.bits_per_entry, 4);

    set(&mut palette_container, 2, 2);
    assert_eq!(palette_container.bits_per_entry, 4);
    set(&mut palette_container, 3, 3);

    set(&mut palette_container, 4, 4);
    assert_eq!(palette_container.bits_per_entry, 4);
    set(&mut palette_container, 5, 5);
    set(&mut palette_container, 6, 6);
    set(&mut palette_container, 7, 7);

    set(&mut palette_container, 8, 8); // 4 bits
    assert_eq!(palette_container.bits_per_entry, 4);
    set(&mut palette_container, 9, 9);
    set(&mut palette_container, 10, 10);
    set(&mut palette_container, 11, 11);
    set(&mut palette_container, 12, 12);
    set(&mut palette_container, 13, 13);
    set(&mut palette_container, 14, 14);
    set(&mut palette_container, 15, 15);
    assert_eq!(palette_container.bits_per_entry, 4);

    set(&mut palette_container, 16, 16); // 5 bits
    assert_eq!(palette_container.bits_per_entry, 5);
}

#[test]
fn test_coords_from_index() {
    let palette_container = PalettedContainer::<BlockState>::new();

    for x in 0..15 {
        for y in 0..15 {
            for z in 0..15 {
                assert_eq!(
                    palette_container.pos_from_index(
                        palette_container.index_from_pos(ChunkSectionBlockPos::new(x, y, z))
                    ),
                    ChunkSectionBlockPos::new(x, y, z)
                );
            }
        }
    }
}

#[test]
fn test_read_1_21_4_single_value_container() {
    // bits per entry 0, single value palette (state 1), long array length 0
    let bytes = [0u8, 1, 0];
    let mut buf = std::io::Cursor::new(&bytes[..]);
    let container = PalettedContainer::<BlockState>::read(&mut buf).unwrap();
    assert_eq!(buf.position(), bytes.len() as u64);
    assert_eq!(
        container.get_at_index(0),
        BlockState::try_from(1_u32).unwrap()
    );
}

#[test]
fn test_write_and_read_1_21_4_linear_container() {
    let mut container = PalettedContainer::<BlockState>::new();
    container.set_at_index(0, BlockState::try_from(1_u32).unwrap());
    container.set_at_index(5, BlockState::try_from(2_u32).unwrap());

    let mut bytes = Vec::new();
    container.write(&mut bytes).unwrap();
    // u8 bits, palette (varint len 3 + three varint ids), then the varint
    // length of the long array
    let longs = container.storage.data.len();
    // 4 bits per entry, as vanilla's linear block state palette
    assert_eq!(longs, 256);
    assert_eq!(&bytes[..2], &[container.bits_per_entry, 3]);
    assert_eq!(&bytes[5..7], &[0x80, 0x02]);
    assert_eq!(bytes.len(), 7 + longs * 8);

    let mut buf = std::io::Cursor::new(&bytes[..]);
    let read = PalettedContainer::<BlockState>::read(&mut buf).unwrap();
    assert_eq!(buf.position(), bytes.len() as u64);
    assert_eq!(read, container);
}

#[test]
fn test_read_container_with_wrong_data_length() {
    // like vanilla 1.21.4, a long array of the wrong size is read and ignored
    let bytes = [
        4u8, 2, 0, 1, 1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    ];
    let mut buf = std::io::Cursor::new(&bytes[..]);
    let container = PalettedContainer::<BlockState>::read(&mut buf).unwrap();
    assert_eq!(buf.position(), bytes.len() as u64);
    assert_eq!(container.get_at_index(0), BlockState::AIR);
}

#[test]
fn block_state_storage_bits_follow_vanilla() {
    // sent bits 1..=4 are a 4-bit linear palette, 9 and up the global
    // palette at ceillog2(block state count) bits, whatever was sent
    for (sent, storage) in [
        (0, 0),
        (1, 4),
        (3, 4),
        (4, 4),
        (5, 5),
        (8, 8),
        (9, 15),
        (200, 15),
    ] {
        assert_eq!(BlockState::storage_bits(sent, None), storage, "{sent}");
    }
    // a global container sent with 200 bits reads 15-bit storage
    let mut bytes = vec![200u8];
    // long array: 4096 * 15 bits in 4 values per long = 1024 longs
    bytes.extend([0x80, 0x08]);
    bytes.extend(std::iter::repeat_n(0, 1024 * 8));
    let mut buf = std::io::Cursor::new(&bytes[..]);
    let container = PalettedContainer::<BlockState>::read(&mut buf).unwrap();
    assert_eq!(buf.position() as usize, bytes.len());
    assert_eq!(container.bits_per_entry, 15);
}

#[test]
fn biome_global_bits_come_from_the_registry_size() {
    use azalea_registry::data::Biome;
    assert_eq!(Biome::storage_bits(3, Some(65)), 3);
    assert_eq!(Biome::storage_bits(4, Some(65)), 7);
    assert_eq!(Biome::storage_bits(30, Some(65)), 7);
    assert_eq!(Biome::storage_bits(6, None), 6);
    // without the registry size, more than 32 bits is an error
    let bytes = [33u8, 0];
    assert!(PalettedContainer::<Biome>::read(&mut std::io::Cursor::new(&bytes[..])).is_err());
    let bytes = [255u8, 0];
    assert!(PalettedContainer::<Biome>::read(&mut std::io::Cursor::new(&bytes[..])).is_err());
}

#[test]
fn bad_palettes_are_errors() {
    // a linear palette (sent 4 bits) with 17 entries: LinearPalette holds 16
    let mut bytes = vec![4u8, 17];
    bytes.extend(1..=17u8);
    assert!(PalettedContainer::<BlockState>::read(&mut std::io::Cursor::new(&bytes[..])).is_err());
    // 16 entries are fine
    let mut bytes = vec![4u8, 16];
    bytes.extend(1..=16u8);
    bytes.extend([0x80, 0x02]);
    bytes.extend(std::iter::repeat_n(0, 256 * 8));
    assert!(PalettedContainer::<BlockState>::read(&mut std::io::Cursor::new(&bytes[..])).is_ok());
    // a palette entry that isn't a block state (byIdOrThrow)
    let mut bytes = vec![0u8];
    bytes.extend([0xff, 0xff, 0xff, 0x0f]);
    bytes.push(0);
    assert!(PalettedContainer::<BlockState>::read(&mut std::io::Cursor::new(&bytes[..])).is_err());
    // a long array length the bytes left can't hold
    let bytes = [4u8, 1, 1, 0xff, 0xff, 0x03];
    assert!(PalettedContainer::<BlockState>::read(&mut std::io::Cursor::new(&bytes[..])).is_err());
}

#[test]
fn resizing_past_the_hashmap_palette_uses_global_bits() {
    // 257 distinct states: linear (4), hashmap (5..=8), then global (15
    // bits, which holds every state id; 9 bits would not)
    let mut container = PalettedContainer::<BlockState>::new();
    for i in 0..257u32 {
        container.set_at_index(i as usize, BlockState::try_from(i * 100).unwrap());
    }
    assert_eq!(container.bits_per_entry, 15);
    assert_eq!(PaletteKind::from(&container.palette), PaletteKind::Global);
    for i in 0..257u32 {
        assert_eq!(
            container.get_at_index(i as usize),
            BlockState::try_from(i * 100).unwrap()
        );
    }
}
