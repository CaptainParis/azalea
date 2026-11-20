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
    assert!(simulation.app.world().get::<RawConnection>(simulation.entity).unwrap().is_alive());

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

    simulation.receive_packet(azalea_protocol::packets::game::ClientboundForgetLevelChunk {
        pos: ChunkPos::new(0, 0),
    });
    simulation.tick();
    simulation.tick();
    simulation.tick();
    let w = simulation.app.world();
    assert!(w.get_entity(e).is_err() || w.get::<StaleEntity>(e).is_none());
}
