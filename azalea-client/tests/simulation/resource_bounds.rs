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
