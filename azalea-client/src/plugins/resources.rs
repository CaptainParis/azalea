//! Resource bounds (azalea-hunt WP4): what a client keeps is bounded by what
//! it can see.
//!
//! - The shared [`WeakChunkStorage`] map keeps a `Weak` per chunk. An entry is
//!   pruned when the client's ring drops or replaces its chunk (on forget, on
//!   ring eviction), and every [`ResourceBounds::sweep_every`] a sweep forgets
//!   the entries whose chunk nothing holds anymore (another holder may have
//!   dropped it later).
//! - The [`Worlds`] map keeps a `Weak` per world: worlds no client holds are
//!   forgotten when another world is added and by the sweep.
//! - `PartialEntityInfos::updates_received` has entries only for entities the
//!   client knows: an entity's entry goes when the server removes it, and the
//!   sweep drops entries of ids no longer in the client's [`EntityIdIndex`].
//!
//! - Entities: a client knows at most [`ResourceBounds::entity_cap`] entities;
//!   past it, the farthest non-player entities are dropped from its model as
//!   the server's `remove_entities` would drop them, and counted
//!   (`DecodeErrorKind::EntityCap`).
//!
//! [`WeakChunkStorage`]: azalea_world::chunk::storage::WeakChunkStorage

use std::time::{Duration, Instant};

use azalea_core::{
    decode_errors::{DecodeErrorKind, record_decode_error},
    entity_id::MinecraftEntityId,
};
use azalea_entity::{
    EntityKindComponent, LoadedBy, LocalEntity, Position, indexing::EntityIdIndex,
};
use azalea_registry::builtin::EntityKind;
use azalea_world::Worlds;
use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;
use tracing::warn;

use crate::local_player::WorldHolder;

/// The limits [`ResourcesPlugin`] keeps. Insert your own before the app
/// starts to change them.
#[derive(Clone, Debug, Resource)]
pub struct ResourceBounds {
    /// How often dead entries of the shared chunk maps are swept.
    pub sweep_every: Duration,
    /// Most entities one client keeps (its own player included).
    pub entity_cap: usize,
}

impl Default for ResourceBounds {
    fn default() -> Self {
        Self {
            sweep_every: Duration::from_secs(30),
            entity_cap: 4096,
        }
    }
}

pub struct ResourcesPlugin;
impl Plugin for ResourcesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ResourceBounds>()
            // only forgets what nothing holds: its order against other
            // systems doesn't matter
            .add_systems(
                Update,
                (
                    sweep_chunk_maps.ambiguous_with_all(),
                    // drops what a remove_entities would; order doesn't matter
                    enforce_entity_cap.ambiguous_with_all(),
                ),
            );
    }
}

/// When the last sweep ran.
#[derive(Default)]
pub struct LastSweep(Option<Instant>);

/// Every [`ResourceBounds::sweep_every`]: forget the shared storage entries
/// whose chunk nothing holds.
pub fn sweep_chunk_maps(
    bounds: Res<ResourceBounds>,
    mut worlds: ResMut<Worlds>,
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
    worlds.prune();
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

/// Past [`ResourceBounds::entity_cap`], drop the farthest non-player
/// entities from the client's model (out of its id index and `LoadedBy`, as
/// `remove_entities` does; `remove_despawned_entities_from_indexes` then
/// despawns what no client loads), counted once per entity.
pub fn enforce_entity_cap(ecs: &mut World) {
    let cap = ecs
        .get_resource::<ResourceBounds>()
        .map_or(usize::MAX, |b| b.entity_cap);
    let mut players =
        ecs.query_filtered::<(Entity, &EntityIdIndex, &Position), With<LocalEntity>>();
    let over: Vec<(Entity, azalea_core::position::Vec3)> = players
        .iter(ecs)
        .filter(|(_, index, _)| index.len() > cap)
        .map(|(e, _, p)| (e, **p))
        .collect();
    for (player, at) in over {
        let Some(index) = ecs.get::<EntityIdIndex>(player) else {
            continue;
        };
        let excess = index.len().saturating_sub(cap);
        let mut candidates: Vec<(f64, MinecraftEntityId, Entity)> = index
            .iter()
            .filter(|(_, e)| *e != player)
            .filter(|(_, e)| {
                ecs.get::<EntityKindComponent>(*e)
                    .is_some_and(|k| **k != EntityKind::Player)
            })
            .map(|(id, e)| {
                let d = ecs
                    .get::<Position>(e)
                    .map_or(f64::MAX, |p| (**p).distance_squared_to(at));
                (d, id, e)
            })
            .collect();
        // farthest first
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        candidates.truncate(excess);
        warn!(
            "client {player:?} knows more than {cap} entities: dropping the {} farthest",
            candidates.len()
        );
        for (_, id, entity) in candidates {
            if let Some(mut index) = ecs.get_mut::<EntityIdIndex>(player) {
                index.remove_by_minecraft_entity(id);
                index.mark_capped(id);
            }
            if let Some(mut loaded_by) = ecs.get_mut::<LoadedBy>(entity) {
                loaded_by.remove(&player);
            }
            crate::bad_packet::entity_removed(ecs, entity);
            forget_entity_updates(ecs, player, [id]);
            // decode error hook: an entity past the per-client cap
            record_decode_error(DecodeErrorKind::EntityCap);
        }
    }
}

/// What one client holds, for metrics and the WP4 bound checks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientResources {
    pub player: Option<Entity>,
    /// Ring slots holding a chunk (stale slots included).
    pub ring_loaded: usize,
    /// The ring's capacity (its hard cap).
    pub ring_capacity: usize,
    pub ring_radius: u32,
    /// Entries in the shared weak chunk map of the client's world.
    pub weak_map_len: usize,
    /// Heap bytes of the light kept in the ring's chunks.
    pub light_bytes: usize,
    /// The most light bytes one of those chunks holds.
    pub light_max_chunk: usize,
    /// Entries of the world's `entities_by_chunk`.
    pub entities_by_chunk: usize,
    /// Of those, entries whose set is empty (0 when bounded).
    pub entities_by_chunk_empty: usize,
    /// Entries of `updates_received`.
    pub updates_received: usize,
    /// Of those, entries for ids the client doesn't know (0 when bounded).
    pub updates_received_unknown: usize,
    /// Entities the client knows (its id index, itself included).
    pub entities: usize,
}

/// What an app holds: every client's [`ClientResources`] plus the app-wide
/// maps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppResources {
    pub clients: Vec<ClientResources>,
    /// Entries of the [`Worlds`] map.
    pub worlds: usize,
    /// Of those, worlds still held.
    pub worlds_live: usize,
    /// ECS entities spawned in the app.
    pub ecs_entities: usize,
}

impl AppResources {
    /// Read the counts from an app's ECS (O(entities + ring); for metrics,
    /// not every tick).
    pub fn read(ecs: &mut World) -> Self {
        let (worlds, worlds_live) = ecs
            .get_resource::<Worlds>()
            .map_or((0, 0), |w| (w.map.len(), w.live_count()));
        let ecs_entities = ecs.entities().count_spawned() as usize;
        let mut q = ecs
            .query_filtered::<(Entity, &WorldHolder, Option<&EntityIdIndex>), With<LocalEntity>>();
        let clients = q
            .iter(ecs)
            .map(|(player, holder, index)| client_resources(player, holder, index))
            .collect();
        Self {
            clients,
            worlds,
            worlds_live,
            ecs_entities,
        }
    }
}

fn client_resources(
    player: Entity,
    holder: &WorldHolder,
    index: Option<&EntityIdIndex>,
) -> ClientResources {
    let mut r = ClientResources {
        player: Some(player),
        entities: index.map_or(0, |i| i.len()),
        ..Default::default()
    };
    {
        let partial = holder.partial.read();
        r.ring_loaded = partial.chunks.loaded_count();
        r.ring_capacity = partial.chunks.capacity();
        r.ring_radius = partial.chunks.chunk_radius();
        for chunk in partial.chunks.chunks().flatten() {
            let b = chunk.read().light.heap_bytes();
            r.light_bytes += b;
            r.light_max_chunk = r.light_max_chunk.max(b);
        }
        let updates = &partial.entity_infos.updates_received;
        r.updates_received = updates.len();
        r.updates_received_unknown = updates
            .keys()
            .filter(|id| !index.is_some_and(|i| i.contains_minecraft_entity(**id)))
            .count();
    }
    let world = holder.shared.read();
    r.weak_map_len = world.chunks.len();
    r.entities_by_chunk = world.entities_by_chunk.len();
    r.entities_by_chunk_empty = world
        .entities_by_chunk
        .values()
        .filter(|s| s.is_empty())
        .count();
    r
}
