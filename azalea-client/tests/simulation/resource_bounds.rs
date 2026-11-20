//! WP4 resource bounds: what a client keeps is bounded by what it can see.

use azalea_client::{local_player::WorldHolder, test_utils::prelude::*};
use azalea_core::position::ChunkPos;
use azalea_protocol::packets::{
    ConnectionProtocol,
    game::{ClientboundForgetLevelChunk, ClientboundSetChunkCacheCenter},
};

const SECTIONS: usize = 384 / 16;

fn weak_map_len(s: &Simulation) -> usize {
    let mut n = 0;
    s.with_component::<WorldHolder>(|wh| n = wh.shared.read().chunks.len());
    n
}

fn ring_count(s: &Simulation) -> usize {
    let mut n = 0;
    s.with_component::<WorldHolder>(|wh| n = wh.partial.read().chunks.loaded_count());
    n
}

/// A forgotten chunk and a chunk the ring evicts leave the shared weak map at
/// once (the map held a dead `Weak` per chunk ever received before).
#[test]
fn weak_map_pruned_on_forget_and_eviction() {
    let _lock = init();
    let mut s = Simulation::new(ConnectionProtocol::Game);
    s.receive_packet(default_login_packet());
    s.tick();
    // default client view distance 8: ring radius 11, 23 slots per side
    s.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), SECTIONS));
    s.receive_packet(make_basic_empty_chunk(ChunkPos::new(1, 0), SECTIONS));
    s.tick();
    assert_eq!(weak_map_len(&s), 2);
    assert_eq!(ring_count(&s), 2);

    s.receive_packet(ClientboundForgetLevelChunk {
        pos: ChunkPos::new(0, 0),
    });
    s.tick();
    assert_eq!(weak_map_len(&s), 1);
    assert_eq!(ring_count(&s), 1);

    // the centre moves 23 chunks east: (1, 0) is stale, (24, 0) takes its slot
    s.receive_packet(ClientboundSetChunkCacheCenter { x: 23, z: 0 });
    s.tick();
    assert_eq!(ring_count(&s), 1, "the stale slot still counts");
    s.receive_packet(make_basic_empty_chunk(ChunkPos::new(24, 0), SECTIONS));
    s.tick();
    assert_eq!(ring_count(&s), 1);
    assert_eq!(weak_map_len(&s), 1, "the evicted chunk's entry is gone");
    assert!(s.chunk(ChunkPos::new(1, 0)).is_none());
    assert!(s.chunk(ChunkPos::new(24, 0)).is_some());
}

/// An entity that moves to another chunk leaves no empty set behind in
/// `World::entities_by_chunk`; a removed entity leaves nothing.
#[test]
fn entities_by_chunk_has_no_empty_sets() {
    use azalea_core::{entity_id::MinecraftEntityId, position::Vec3};
    use azalea_protocol::{
        common::movements::{PositionMoveRotation, RelativeMovements},
        packets::game::{ClientboundRemoveEntities, ClientboundTeleportEntity},
    };
    use azalea_registry::builtin::EntityKind;

    let _lock = init();
    let mut s = Simulation::new(ConnectionProtocol::Game);
    s.receive_packet(default_login_packet());
    for x in 0..4 {
        s.receive_packet(make_basic_empty_chunk(ChunkPos::new(x, 0), SECTIONS));
    }
    s.tick();
    s.receive_packet(make_basic_add_entity(EntityKind::Cow, 123, (0.5, 64., 0.5)));
    s.tick();
    let chunk_entries = |s: &Simulation| {
        let mut v = Vec::new();
        s.with_component::<WorldHolder>(|wh| {
            let w = wh.shared.read();
            v = w
                .entities_by_chunk
                .iter()
                .map(|(p, e)| (*p, e.len()))
                .collect();
        });
        v.sort_by_key(|(p, _)| (p.x, p.z));
        v
    };
    // our own player (at the origin) and the cow
    assert_eq!(chunk_entries(&s), vec![(ChunkPos::new(0, 0), 2)]);
    for step in 1..4 {
        s.receive_packet(ClientboundTeleportEntity {
            id: MinecraftEntityId(123),
            change: PositionMoveRotation {
                pos: Vec3::new(16., 0., 0.),
                delta: Vec3::ZERO,
                look_direction: Default::default(),
            },
            relative: RelativeMovements::all_relative(),
            on_ground: true,
        });
        s.tick();
        let entries = chunk_entries(&s);
        assert!(entries.iter().all(|(_, n)| *n > 0), "{entries:?}");
        assert_eq!(
            entries,
            vec![(ChunkPos::new(0, 0), 1), (ChunkPos::new(step, 0), 1)]
        );
    }
    s.receive_packet(ClientboundRemoveEntities {
        entity_ids: vec![MinecraftEntityId(123)],
    });
    s.tick();
    assert_eq!(chunk_entries(&s), vec![(ChunkPos::new(0, 0), 1)]);
}

/// `updates_received` has entries only for entities the client knows: the
/// server's removal takes the entity's entry out.
#[test]
fn updates_received_forgets_removed_entities() {
    use azalea_core::{entity_id::MinecraftEntityId, position::Vec3};
    use azalea_protocol::{
        common::movements::{PositionMoveRotation, RelativeMovements},
        packets::game::{ClientboundRemoveEntities, ClientboundTeleportEntity},
    };
    use azalea_registry::builtin::EntityKind;

    let _lock = init();
    let mut s = Simulation::new(ConnectionProtocol::Game);
    s.receive_packet(default_login_packet());
    s.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), SECTIONS));
    s.tick();
    let ids = [100, 101, 102];
    for id in ids {
        let mut add = make_basic_add_entity(EntityKind::Cow, id, (0.5, 64., 0.5));
        add.uuid = uuid::Uuid::from_u128(id as u128);
        s.receive_packet(add);
    }
    s.tick();
    for id in ids {
        s.receive_packet(ClientboundTeleportEntity {
            id: MinecraftEntityId(id),
            change: PositionMoveRotation {
                pos: Vec3::new(1., 0., 0.),
                delta: Vec3::ZERO,
                look_direction: Default::default(),
            },
            relative: RelativeMovements::all_relative(),
            on_ground: true,
        });
    }
    s.tick();
    let counted = |s: &Simulation| {
        let mut v = Vec::new();
        s.with_component::<WorldHolder>(|wh| {
            v = wh
                .partial
                .read()
                .entity_infos
                .updates_received
                .keys()
                .map(|k| k.0)
                .collect();
        });
        v.sort();
        v
    };
    assert_eq!(counted(&s), vec![100, 101, 102]);
    s.receive_packet(ClientboundRemoveEntities {
        entity_ids: vec![MinecraftEntityId(100), MinecraftEntityId(102)],
    });
    s.tick();
    assert_eq!(counted(&s), vec![101]);
}

/// Changing dimension back and forth keeps the `Worlds` map at the worlds in
/// use (at most overworld, nether and end).
#[test]
fn worlds_map_bounded_across_dimension_changes() {
    use azalea_registry::{DataRegistry, data::DimensionKind, identifier::Identifier};

    let _lock = init();
    let mut s = Simulation::new(ConnectionProtocol::Game);
    s.receive_packet(default_login_packet());
    s.tick();
    let names = [
        "minecraft:the_nether",
        "minecraft:the_end",
        "minecraft:overworld",
    ];
    for i in 0..12 {
        s.receive_packet(make_basic_respawn_packet(
            DimensionKind::new_raw(0),
            Identifier::new(names[i % 3]),
        ));
        s.tick();
        let mut n = 0;
        s.with_resource::<azalea_world::Worlds>(|w| n = w.map.len());
        assert!(n <= 3, "{n} worlds after {i} changes");
    }
    let mut live = 0;
    s.with_resource::<azalea_world::Worlds>(|w| live = w.live_count());
    assert_eq!(live, 1);
}

/// A chunk out of the ring is refused and counted (`ChunkCap`); the ring and
/// the weak map never hold more than the ring's capacity.
#[test]
fn chunk_out_of_ring_refused_and_counted() {
    use azalea_core::decode_errors::decode_errors;

    let _lock = init_expecting_errors();
    let mut s = Simulation::new(ConnectionProtocol::Game);
    // client view distance 7: ring radius 10
    s.with_component_mut::<azalea_client::ClientInformation>(|mut ci| ci.view_distance = 7);
    s.receive_packet(default_login_packet());
    s.tick();
    let before = decode_errors().chunk_cap;
    s.receive_packet(make_basic_empty_chunk(ChunkPos::new(10, -10), SECTIONS));
    s.receive_packet(make_basic_empty_chunk(ChunkPos::new(11, 0), SECTIONS));
    s.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, -11), SECTIONS));
    s.tick();
    assert_eq!(decode_errors().chunk_cap - before, 2);
    assert_eq!(ring_count(&s), 1);
    assert_eq!(weak_map_len(&s), 1);
    assert!(s.chunk(ChunkPos::new(11, 0)).is_none());
}

/// WP4 ring and view distance: with the hunt config (client view distance 7,
/// the 2b2t server's), the ring radius is 10 (441 chunks).
#[test]
fn ring_radius_is_ten_at_view_distance_seven() {
    let _lock = init();
    let mut s = Simulation::new(ConnectionProtocol::Game);
    s.with_component_mut::<azalea_client::ClientInformation>(|mut ci| ci.view_distance = 7);
    s.receive_packet(default_login_packet());
    s.tick();
    s.with_component::<WorldHolder>(|wh| {
        let p = wh.partial.read();
        assert_eq!(p.chunks.chunk_radius(), 10);
        assert_eq!(p.chunks.capacity(), 441);
    });
}

/// Past the entity cap, the farthest non-player entities are dropped from the
/// model and counted (`EntityCap`); players are never dropped.
#[test]
fn entity_cap_drops_farthest_non_players() {
    use azalea_client::resources::ResourceBounds;
    use azalea_core::decode_errors::decode_errors;
    use azalea_entity::indexing::EntityIdIndex;
    use azalea_registry::builtin::EntityKind;

    let _lock = init_expecting_errors();
    let mut s = Simulation::new(ConnectionProtocol::Game);
    s.with_resource_mut::<ResourceBounds>(|mut b| b.entity_cap = 4);
    s.receive_packet(default_login_packet());
    for x in 0..4 {
        s.receive_packet(make_basic_empty_chunk(ChunkPos::new(x, 0), SECTIONS));
    }
    s.tick();
    let before = decode_errors().entity_cap;
    // our player (at the origin) + 2 cows: under the cap
    for (id, x) in [(10, 5.0), (11, 50.0)] {
        let mut add = make_basic_add_entity(EntityKind::Cow, id, (x, 64., 0.5));
        add.uuid = uuid::Uuid::from_u128(id as u128);
        s.receive_packet(add);
    }
    s.tick();
    // a far player and two more cows: 6 known, 2 over
    let mut far_player = make_basic_add_entity(EntityKind::Player, 12, (60.0, 64., 0.5));
    far_player.uuid = uuid::Uuid::from_u128(12);
    s.receive_packet(far_player);
    for (id, x) in [(13, 40.0), (14, 2.0)] {
        let mut add = make_basic_add_entity(EntityKind::Cow, id, (x, 64., 0.5));
        add.uuid = uuid::Uuid::from_u128(id as u128);
        s.receive_packet(add);
    }
    s.tick();
    s.tick();
    let mut known: Vec<i32> =
        s.query_self::<&EntityIdIndex, _>(|i| i.iter().map(|(id, _)| id.0).collect());
    known.sort();
    // the player at 60 stays; the cows at 50 and 40 go
    assert_eq!(known, vec![0, 10, 12, 14]);
    assert_eq!(decode_errors().entity_cap - before, 2);
}
