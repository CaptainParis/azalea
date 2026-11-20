//! Regression (azalea-hunt WP4 reconnect test): parsed enchantment effects
//! are freed with their registry. `EffectComponentUnion`'s fields are
//! `ManuallyDrop`, so before `EffectComponent` every effect leaked when the
//! registry went: a few KB per session.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicIsize, Ordering},
};

use azalea_client::test_utils::prelude::*;
use azalea_protocol::packets::{ConnectionProtocol, config::ClientboundRegistryData};
use azalea_registry::identifier::Identifier;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

fn list(c: Vec<NbtCompound>) -> NbtTag {
    NbtTag::List(NbtList::from(c))
}

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);
// SAFETY: forwards to System with the same layout; only counts around it
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(l.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        LIVE.fetch_add(n as isize - l.size() as isize, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

/// Vanilla's `minecraft:wind_burst` (a post_attack effect with nested
/// all_of effects, the case the leak was found on), and an attributes one.
fn enchantments() -> ClientboundRegistryData {
    let wind_burst = NbtCompound::from([(
        "effects",
        [(
            "minecraft:post_attack",
            list(vec![NbtCompound::from([
                ("enchanted", "attacker".into()),
                ("affected", "attacker".into()),
                (
                    "effect",
                    [
                        ("type", "minecraft:all_of".into()),
                        (
                            "effects",
                            list(vec![
                                NbtCompound::from([
                                    ("type", "minecraft:ignite".into()),
                                    ("duration", 4.0f32.into()),
                                ]),
                                NbtCompound::from([
                                    ("type", "minecraft:ignite".into()),
                                    ("duration", 2.0f32.into()),
                                ]),
                            ]),
                        ),
                    ]
                    .into(),
                ),
            ])]),
        )]
        .into(),
    )]);
    let efficiency = NbtCompound::from([(
        "effects",
        [(
            "minecraft:attributes",
            [
                ("operation", "add_value".into()),
                ("attribute", "minecraft:mining_efficiency".into()),
                (
                    "amount",
                    [
                        ("type", "minecraft:levels_squared".into()),
                        ("added", 1.0f32.into()),
                    ]
                    .into(),
                ),
                ("id", "minecraft:enchantment.efficiency".into()),
            ]
            .into(),
        )]
        .into(),
    )]);
    ClientboundRegistryData {
        registry_id: Identifier::new("minecraft:enchantment"),
        entries: vec![
            (Identifier::new("minecraft:wind_burst"), Some(wind_burst)),
            (Identifier::new("minecraft:efficiency"), Some(efficiency)),
        ]
        .into_iter()
        .collect(),
    }
}

#[test]
fn enchantment_effects_are_freed_with_their_registry() {
    let _lock = init();
    let mut s = Simulation::new(ConnectionProtocol::Configuration);
    let packet = enchantments();
    let mut samples = Vec::new();
    for _ in 0..60 {
        s.reset(ConnectionProtocol::Configuration);
        s.receive_packet(packet.clone());
        s.tick();
        samples.push(LIVE.load(Ordering::Relaxed));
    }
    // the parsed registry exists in each session, then goes with it
    let per_session = (samples[59] - samples[29]) as f64 / 30.0;
    assert!(per_session < 200.0, "{per_session:.0} B per session stay");
}
