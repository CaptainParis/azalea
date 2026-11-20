//! Resource bounds (azalea-hunt WP4): what a client keeps is bounded by what
//! it can see.
//!
//! - The shared [`WeakChunkStorage`] map keeps a `Weak` per chunk. An entry is
//!   pruned when the client's ring drops or replaces its chunk (on forget, on
//!   ring eviction), and every [`ResourceBounds::sweep_every`] a sweep forgets
//!   the entries whose chunk nothing holds anymore (another holder may have
//!   dropped it later).
//! - `PartialEntityInfos::updates_received` has entries only for entities the
//!   client knows: an entity's entry goes when the server removes it, and the
//!   sweep drops entries of ids no longer in the client's [`EntityIdIndex`].
//!
//! [`WeakChunkStorage`]: azalea_world::chunk::storage::WeakChunkStorage

use std::time::{Duration, Instant};

use azalea_core::entity_id::MinecraftEntityId;
use azalea_entity::indexing::EntityIdIndex;
use azalea_world::Worlds;
use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;

use crate::local_player::WorldHolder;

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
    players: Query<(&WorldHolder, &EntityIdIndex)>,
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
    for (holder, index) in &players {
        holder
            .partial
            .write()
            .entity_infos
            .updates_received
            .retain(|id, _| index.contains_minecraft_entity(*id));
    }
}

/// The server removed these entities for `player`: their update counters go
/// too.
pub(crate) fn forget_entity_updates(
    ecs: &World,
    player: Entity,
    ids: impl IntoIterator<Item = MinecraftEntityId>,
) {
    let Some(holder) = ecs.get::<WorldHolder>(player) else {
        return;
    };
    let mut partial = holder.partial.write();
    for id in ids {
        // network-safe: HashMap::remove by key, not an index
        partial.entity_infos.updates_received.remove(&id);
    }
}
