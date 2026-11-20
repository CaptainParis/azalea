//! Self-containing item data (item stacks in containers and bundles, effect
//! details in effect details) nests at most 512 deep, and the deepest
//! allowed input decodes in linear time.

use std::{
    io::Cursor,
    time::{Duration, Instant},
};

use azalea_buf::{AzBuf, AzBufVar, BufReadError, limits::LimitKind};
use azalea_inventory::{ItemStack, components::MobEffectDetails};
use azalea_registry::builtin::{DataComponentKind, ItemKind};

fn on_bot_thread(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

/// `depth` item stacks, each but the last holding the next in a component
/// of the given kind (`container` or `bundle_contents`, both a list of
/// stacks).
pub fn nested_items(depth: usize, kind: DataComponentKind) -> Vec<u8> {
    let mut v = Vec::new();
    for _ in 0..depth - 1 {
        1i32.azalea_write_var(&mut v).unwrap();
        ItemKind::ShulkerBox.azalea_write(&mut v).unwrap();
        // one component with data, none removed
        1u32.azalea_write_var(&mut v).unwrap();
        0u32.azalea_write_var(&mut v).unwrap();
        kind.azalea_write(&mut v).unwrap();
        1u32.azalea_write_var(&mut v).unwrap();
    }
    1i32.azalea_write_var(&mut v).unwrap();
    ItemKind::Stone.azalea_write(&mut v).unwrap();
    0u32.azalea_write_var(&mut v).unwrap();
    0u32.azalea_write_var(&mut v).unwrap();
    v
}

fn nesting_error(e: BufReadError) -> bool {
    matches!(
        e,
        BufReadError::Limit {
            kind: LimitKind::NestingDepth,
            ..
        }
    )
}

#[test]
fn nested_items_at_the_limit() {
    on_bot_thread(|| {
        for kind in [
            DataComponentKind::Container,
            DataComponentKind::BundleContents,
        ] {
            let data = nested_items(512, kind);
            let start = Instant::now();
            let mut cursor = Cursor::new(&data[..]);
            let item = ItemStack::azalea_read(&mut cursor).unwrap();
            assert!(start.elapsed() < Duration::from_secs(1));
            assert_eq!(cursor.position() as usize, data.len());
            // and it writes back the same bytes
            let mut out = Vec::new();
            item.azalea_write(&mut out).unwrap();
            assert_eq!(out, data);

            let data = nested_items(513, kind);
            let e = ItemStack::azalea_read(&mut Cursor::new(&data[..])).unwrap_err();
            assert!(nesting_error(e));
        }
    });
}

fn nested_effect_details(depth: usize) -> Vec<u8> {
    let mut v = Vec::new();
    for i in 0..depth {
        // amplifier, duration, ambient, show_particles, show_icon
        v.extend([0, 1, 0, 1, 1]);
        // hidden_effect present, except on the last
        v.push(u8::from(i + 1 < depth));
    }
    v
}

#[test]
fn nested_effect_details_at_the_limit() {
    on_bot_thread(|| {
        let data = nested_effect_details(512);
        let start = Instant::now();
        let mut cursor = Cursor::new(&data[..]);
        MobEffectDetails::azalea_read(&mut cursor).unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(cursor.position() as usize, data.len());

        let data = nested_effect_details(513);
        let e = MobEffectDetails::azalea_read(&mut Cursor::new(&data[..])).unwrap_err();
        assert!(nesting_error(e));
    });
}
