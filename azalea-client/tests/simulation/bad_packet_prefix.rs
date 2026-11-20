//! Bad packet policy, packets whose first fields are all that matters: a
//! keep-alive, ping, chunk batch end or block change ack whose body has bytes
//! left over (or is cut after the fields) is answered from the readable
//! fields as vanilla would, instead of closing; an explosion's knockback is
//! applied when the particle after it is bad. If not even the fields can be
//! read, they still close. A bad set_passengers about our own entity is
//! dropped (Azalea doesn't model passengers).

use azalea_client::{
    bad_packet::{BadPacketAction, BadPacketEvent, PacketClass},
    connection::RawConnection,
    test_utils::prelude::*,
};
use azalea_core::{decode_errors::decode_errors, entity_id::MinecraftEntityId, position::Vec3};
use azalea_entity::Physics;
use azalea_protocol::packets::{
    ConnectionProtocol, Packet, ProtocolPacket,
    config::ClientboundConfigPacket,
    game::{
        ClientboundBlockChangedAck, ClientboundChunkBatchFinished, ClientboundExplode,
        ClientboundKeepAlive, ClientboundPing, ClientboundSetPassengers, ServerboundGamePacket,
    },
};

fn alive(simulation: &Simulation) -> bool {
    simulation
        .app
        .world()
        .get::<RawConnection>(simulation.entity)
        .is_some_and(|c| c.is_alive())
}

/// The packet's bytes with one byte left over after the body.
fn with_leftover<P: ProtocolPacket + std::fmt::Debug>(packet: impl Packet<P>) -> Box<[u8]> {
    let mut raw = azalea_protocol::write::serialize_packet(&packet.into_variant())
        .unwrap()
        .into_vec();
    raw.push(0x2a);
    raw.into()
}

fn game() -> (Simulation, SentPackets) {
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    let sent = SentPackets::new(&mut simulation);
    (simulation, sent)
}

fn all_sent(sent: &SentPackets) -> Vec<ServerboundGamePacket> {
    std::iter::from_fn(|| sent.next()).collect()
}

#[test]
fn keep_alive_with_leftover_bytes_is_answered() {
    let _lock = init_expecting_errors();
    let (mut simulation, sent) = game();
    let events = simulation.record_messages::<BadPacketEvent>();
    let before = decode_errors();

    simulation.receive_raw_packet(with_leftover(ClientboundKeepAlive { id: 0xdead_beef }));
    simulation.update();

    let after = decode_errors();
    assert_eq!(after.prefix_applied, before.prefix_applied + 1);
    assert_eq!(after.policy_close, before.policy_close);
    assert!(alive(&simulation));
    assert!(
        all_sent(&sent)
            .iter()
            .any(|p| matches!(p, ServerboundGamePacket::KeepAlive(k) if k.id == 0xdead_beef)),
        "the keep-alive is answered"
    );
    let events = events.lock();
    assert_eq!(events[0].class, Some(PacketClass::Critical));
    assert!(matches!(
        events[0].action,
        BadPacketAction::PrefixApplied { .. }
    ));
}

#[test]
fn keep_alive_without_a_readable_id_closes() {
    let _lock = init_expecting_errors();
    let (mut simulation, _sent) = game();
    let before = decode_errors();
    simulation.receive_raw_packet(truncated_packet(ClientboundKeepAlive { id: 1 }, 4));
    simulation.tick();
    simulation.tick();
    assert_eq!(decode_errors().policy_close, before.policy_close + 1);
    assert_eq!(decode_errors().prefix_applied, before.prefix_applied);
    assert!(!alive(&simulation));
}

#[test]
fn ping_chunk_batch_and_block_ack_with_leftover_bytes_are_answered() {
    let _lock = init_expecting_errors();
    let (mut simulation, sent) = game();
    let before = decode_errors();

    simulation.receive_raw_packet(with_leftover(ClientboundPing { id: 77 }));
    simulation.receive_raw_packet(with_leftover(ClientboundChunkBatchFinished {
        batch_size: 9,
    }));
    simulation.receive_raw_packet(with_leftover(ClientboundBlockChangedAck { seq: 3 }));
    simulation.update();
    simulation.update();

    assert_eq!(decode_errors().prefix_applied, before.prefix_applied + 3);
    assert_eq!(decode_errors().policy_close, before.policy_close);
    assert!(alive(&simulation));
    let sent = all_sent(&sent);
    assert!(
        sent.iter()
            .any(|p| matches!(p, ServerboundGamePacket::Pong(p) if p.id == 77)),
        "{sent:?}"
    );
    assert!(
        sent.iter()
            .any(|p| matches!(p, ServerboundGamePacket::ChunkBatchReceived(_))),
        "{sent:?}"
    );
}

#[test]
fn explosion_knockback_is_applied_when_its_particle_is_bad() {
    let _lock = init_expecting_errors();
    let (mut simulation, _sent) = game();
    let before = decode_errors();
    let v0 = simulation.component::<Physics>().velocity;

    let packet = ClientboundExplode {
        center: Vec3::new(1., 64., 1.),
        player_knockback: Some(Vec3::new(0.5, 0.25, -0.5)),
        explosion_particle: azalea_entity::particle::Particle::Explosion,
        explosion_sound: azalea_registry::Holder::Reference(
            azalea_registry::builtin::SoundEvent::EntityGenericExplode,
        ),
    };
    // centre (24 bytes) and knockback (1 + 24) are there, the particle cut
    simulation.receive_raw_packet(truncated_packet(packet, 24 + 1 + 24));
    simulation.update();

    assert_eq!(decode_errors().prefix_applied, before.prefix_applied + 1);
    assert!(alive(&simulation));
    let v1 = simulation.component::<Physics>().velocity;
    assert_eq!(v1 - v0, Vec3::new(0.5, 0.25, -0.5));
}

#[test]
fn bad_set_passengers_about_us_is_dropped() {
    let _lock = init_expecting_errors();
    let (mut simulation, _sent) = game();
    let events = simulation.record_messages::<BadPacketEvent>();
    let before = decode_errors();
    // vehicle 9, three passengers announced, only us (id 0) there
    let id = azalea_protocol::packets::game::ClientboundGamePacket::SetPassengers(
        ClientboundSetPassengers {
            vehicle: MinecraftEntityId(9),
            passengers: vec![],
        },
    )
    .id();
    let mut raw = Vec::new();
    azalea_buf::AzBufVar::azalea_write_var(&id, &mut raw).unwrap();
    raw.extend_from_slice(&[9, 3, 0]);
    simulation.receive_raw_packet(raw);
    simulation.tick();
    simulation.tick();
    assert_eq!(
        decode_errors().bad_packet_dropped,
        before.bad_packet_dropped + 1
    );
    assert_eq!(decode_errors().policy_close, before.policy_close);
    assert!(alive(&simulation));
    assert_eq!(events.lock()[0].action, BadPacketAction::Dropped);
}

#[test]
fn configuration_keep_alive_with_leftover_bytes_is_answered() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Configuration);
    simulation.tick();
    let before = decode_errors();
    let mut raw = azalea_protocol::write::serialize_packet(&ClientboundConfigPacket::KeepAlive(
        azalea_protocol::packets::config::ClientboundKeepAlive { id: 5 },
    ))
    .unwrap()
    .into_vec();
    raw.push(1);
    simulation.receive_raw_packet(raw);
    simulation.tick();
    simulation.tick();
    assert_eq!(decode_errors().prefix_applied, before.prefix_applied + 1);
    assert_eq!(decode_errors().policy_close, before.policy_close);
    assert!(alive(&simulation));
}
