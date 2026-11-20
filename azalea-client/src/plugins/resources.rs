//! Resource bounds (azalea-hunt WP4): what a client keeps is bounded by what
//! it can see.
//!
//! - The shared [`WeakChunkStorage`] map keeps a `Weak` per chunk. An entry is
//!   pruned when the client's ring drops or replaces its chunk (on forget, on
//!   ring eviction), and every [`ResourceBounds::sweep_every`] a sweep forgets
//!   the entries whose chunk nothing holds anymore (another holder may have
//!   dropped it later).
//!
//! [`WeakChunkStorage`]: azalea_world::chunk::storage::WeakChunkStorage

use std::time::{Duration, Instant};

use azalea_world::Worlds;
use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;

/// The limits [`ResourcesPlugin`] keeps. Insert your own before the app
/// starts to change them.
#[derive(Clone, Debug, Resource)]
pub struct ResourceBounds {
    /// How often dead entries of the shared chunk maps are swept.
    pub sweep_every: Duration,
}

impl Default for ResourceBounds {
    fn default() -> Self {
        Self {
            sweep_every: Duration::from_secs(30),
        }
    }
}

pub struct ResourcesPlugin;
impl Plugin for ResourcesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ResourceBounds>()
            .add_systems(Update, sweep_chunk_maps);
    }
}

/// When the last sweep ran.
#[derive(Default)]
pub struct LastSweep(Option<Instant>);

/// Every [`ResourceBounds::sweep_every`]: forget the shared storage entries
/// whose chunk nothing holds.
pub fn sweep_chunk_maps(
    bounds: Res<ResourceBounds>,
    worlds: Res<Worlds>,
    mut last: Local<LastSweep>,
) {
    let now = Instant::now();
    let due = last
        .0
        .is_none_or(|t| now.duration_since(t) >= bounds.sweep_every);
    if !due {
        return;
    }
    last.0 = Some(now);
    for weak in worlds.map.values() {
        if let Some(world) = weak.upgrade() {
            world.write().chunks.sweep();
        }
    }
}
