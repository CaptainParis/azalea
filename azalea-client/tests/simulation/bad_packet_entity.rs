//! Bad packet policy, other entities: a packet about another entity that
//! can't be decoded leaves it in the model with its last good state, marked
//! stale and counted; later packets for it still apply; the server's remove
//! clears it.

use azalea_client::{
    bad_packet::{BadPacketAction, BadPacketEvent, PacketClass, StaleEntity},
    connection::RawConnection,
    test_utils::prelude::*,
};
use azalea_core::{
    decode_errors::decode_errors,
    delta::PositionDelta8,
    entity_id::MinecraftEntityId,
    position::{ChunkPos, Vec3},
};
use azalea_entity::{Position, indexing::EntityIdIndex};
use azalea_protocol::packets::{
    ConnectionProtocol,
    game::{ClientboundMoveEntityPos, ClientboundRemoveEntities},
};
use azalea_registry::builtin::EntityKind;
use bevy_ecs::entity::Entity;

fn cow(simulation: &Simulation) -> Option<Entity> {
    simulation
        .app
        .world()
        .get::<EntityIdIndex>(simulation.entity)?
        .get_by_minecraft_entity(MinecraftEntityId(123))
}

fn position(simulation: &Simulation, e: Entity) -> Vec3 {
    **simulation.app.world().get::<Position>(e).unwrap()
}

fn is_stale(simulation: &Simulation, e: Entity) -> bool {
    simulation.app.world().get::<StaleEntity>(e).is_some()
}

#[test]
fn bad_entity_packet_marks_the_entity_stale() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), (384 + 64) / 16));
    simulation.tick();
    simulation.receive_packet(make_basic_add_entity(EntityKind::Cow, 123, (0.5, 64., 0.5)));
    simulation.tick();
    simulation.tick();
    let e = cow(&simulation).expect("cow added");
    let events = simulation.record_messages::<BadPacketEvent>();
    let before = decode_errors();

    // the id is there, the delta is cut
    simulation.receive_raw_packet(truncated_packet(
        ClientboundMoveEntityPos {
            entity_id: MinecraftEntityId(123),
            delta: PositionDelta8 {
                xa: 4096,
                ya: 0,
                za: 0,
            },
            on_ground: true,
        },
        2,
    ));
    simulation.tick();

    assert_eq!(decode_errors().stale_entity, before.stale_entity + 1);
    assert!(is_stale(&simulation, e));
    // last good state
    assert_eq!(position(&simulation, e), Vec3::new(0.5, 64., 0.5));
    {
        let events = events.lock();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].class, Some(PacketClass::Entity));
        assert_eq!(
            events[0].action,
            BadPacketAction::EntityStale {
                entities: vec![MinecraftEntityId(123)]
            }
        );
    }
    assert!(
        simulation
            .app
            .world()
            .get::<RawConnection>(simulation.entity)
            .unwrap()
            .is_alive()
    );

    // later packets for it still apply (it isn't an unknown id), it stays
    // stale
    simulation.receive_packet(ClientboundMoveEntityPos {
        entity_id: MinecraftEntityId(123),
        delta: PositionDelta8 {
            xa: 4096,
            ya: 0,
            za: 0,
        },
        on_ground: true,
    });
    simulation.tick();
    simulation.tick();
    assert_eq!(position(&simulation, e), Vec3::new(1.5, 64., 0.5));
    assert!(is_stale(&simulation, e));
    assert_eq!(decode_errors().unknown_entity, before.unknown_entity);

    // the server's remove clears it
    simulation.receive_packet(ClientboundRemoveEntities {
        entity_ids: vec![MinecraftEntityId(123)],
    });
    simulation.tick();
    simulation.tick();
    assert!(cow(&simulation).is_none());
    let w = simulation.app.world();
    assert!(w.get_entity(e).is_err() || w.get::<StaleEntity>(e).is_none());
}

#[test]
fn stale_entity_is_cleared_when_its_chunk_unloads() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), (384 + 64) / 16));
    simulation.tick();
    simulation.receive_packet(make_basic_add_entity(EntityKind::Cow, 123, (0.5, 64., 0.5)));
    simulation.tick();
    simulation.tick();
    let e = cow(&simulation).unwrap();
    simulation.receive_raw_packet(truncated_packet(
        ClientboundMoveEntityPos {
            entity_id: MinecraftEntityId(123),
            delta: PositionDelta8::default(),
            on_ground: true,
        },
        1,
    ));
    simulation.tick();
    assert!(is_stale(&simulation, e));

    simulation.receive_packet(
        azalea_protocol::packets::game::ClientboundForgetLevelChunk {
            pos: ChunkPos::new(0, 0),
        },
    );
    simulation.tick();
    simulation.tick();
    simulation.tick();
    let w = simulation.app.world();
    assert!(w.get_entity(e).is_err() || w.get::<StaleEntity>(e).is_none());
}

fn add_cow_packet() -> azalea_protocol::packets::game::ClientboundAddEntity {
    make_basic_add_entity(EntityKind::Cow, 123, (0.5, 64., 0.5))
}

#[test]
fn bad_spawn_spawns_a_stale_placeholder_and_alerts_at_once() {
    use azalea_client::bad_packet::{AlertReason, BadPacketAlert, PlaceholderEntity};
    use azalea_entity::EntityKindComponent;

    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), (384 + 64) / 16));
    simulation.tick();
    let events = simulation.record_messages::<BadPacketEvent>();
    let alerts = simulation.record_messages::<BadPacketAlert>();
    let before = decode_errors();

    // id (1 byte), uuid (16), type (1), position (24) are there; the
    // rotations, data and velocity are cut
    simulation.receive_raw_packet(truncated_packet(add_cow_packet(), 1 + 16 + 1 + 24 + 1));
    simulation.tick();
    simulation.tick();

    let after = decode_errors();
    assert_eq!(after.placeholder_entity, before.placeholder_entity + 1);
    assert_eq!(after.stale_entity, before.stale_entity + 1);
    assert_eq!(after.policy_close, before.policy_close);
    let e = cow(&simulation).expect("placeholder spawned");
    assert!(is_stale(&simulation, e));
    let w = simulation.app.world();
    assert_eq!(
        w.get::<PlaceholderEntity>(e),
        Some(&PlaceholderEntity {
            uuid_known: true,
            kind_known: true,
            position_known: true,
            rest_known: false,
        })
    );
    assert_eq!(**w.get::<EntityKindComponent>(e).unwrap(), EntityKind::Cow);
    assert_eq!(position(&simulation, e), Vec3::new(0.5, 64., 0.5));
    {
        let events = events.lock();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].class, Some(PacketClass::Entity));
        assert!(matches!(
            events[0].action,
            BadPacketAction::PlaceholderSpawned {
                id: MinecraftEntityId(123),
                ..
            }
        ));
        // the first lost spawn alerts, it doesn't wait for a second
        let alerts = alerts.lock();
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert_eq!(alerts[0].reason, AlertReason::ModelGap);
        assert_eq!(alerts[0].count_in_window, 1);
    }
    assert!(
        simulation
            .app
            .world()
            .get::<RawConnection>(simulation.entity)
            .unwrap()
            .is_alive()
    );

    // later packets for its id apply (no unknown entity)
    simulation.receive_packet(ClientboundMoveEntityPos {
        entity_id: MinecraftEntityId(123),
        delta: PositionDelta8 {
            xa: 4096,
            ya: 0,
            za: 0,
        },
        on_ground: true,
    });
    simulation.tick();
    simulation.tick();
    assert_eq!(position(&simulation, e), Vec3::new(1.5, 64., 0.5));
    assert_eq!(decode_errors().unknown_entity, before.unknown_entity);

    // the server's remove takes it out
    simulation.receive_packet(ClientboundRemoveEntities {
        entity_ids: vec![MinecraftEntityId(123)],
    });
    simulation.tick();
    simulation.tick();
    assert!(cow(&simulation).is_none());
}

#[test]
fn bad_spawn_with_only_the_id_readable_spawns_a_marker_placeholder() {
    use azalea_client::bad_packet::PlaceholderEntity;
    use azalea_entity::EntityKindComponent;

    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    let before = decode_errors();
    // the id and half of the uuid
    simulation.receive_raw_packet(truncated_packet(add_cow_packet(), 1 + 8));
    simulation.tick();
    simulation.tick();

    assert_eq!(
        decode_errors().placeholder_entity,
        before.placeholder_entity + 1
    );
    let e = cow(&simulation).expect("placeholder spawned");
    let w = simulation.app.world();
    assert_eq!(
        w.get::<PlaceholderEntity>(e),
        Some(&PlaceholderEntity {
            uuid_known: false,
            kind_known: false,
            position_known: false,
            rest_known: false,
        })
    );
    // a type no server sends
    assert_eq!(
        **w.get::<EntityKindComponent>(e).unwrap(),
        EntityKind::Marker
    );
    // its position isn't known, so it stays stale even out of loaded chunks
    simulation.tick();
    assert!(is_stale(&simulation, e));
}
