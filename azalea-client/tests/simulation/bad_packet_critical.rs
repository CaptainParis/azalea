//! Bad packet policy, no safe repair: a teleport, our own entity's data, a
//! packet with an unknown id, and a configuration packet that can't be
//! decoded close the connection (for the supervisor to reconnect) and raise
//! an alert.

use azalea_client::{
    bad_packet::{AlertReason, BadPacketAction, BadPacketAlert, BadPacketEvent, PacketClass},
    test_utils::prelude::*,
};
use azalea_core::{decode_errors::decode_errors, entity_id::MinecraftEntityId, position::Vec3};
use azalea_protocol::{
    common::movements::{PositionMoveRotation, RelativeMovements},
    packets::{
        ConnectionProtocol,
        config::ClientboundUpdateEnabledFeatures,
        game::{ClientboundPlayerPosition, ClientboundSetHealth},
    },
};
use azalea_world::WorldName;

fn closes(
    simulation: &mut Simulation,
    raw: Box<[u8]>,
    name: Option<&str>,
    class: Option<PacketClass>,
) {
    let events = simulation.record_messages::<BadPacketEvent>();
    let alerts = simulation.record_messages::<BadPacketAlert>();
    let disconnects = simulation.record_disconnects();
    let before = decode_errors();

    simulation.receive_raw_packet(raw.clone());
    simulation.tick();
    simulation.tick();

    assert_eq!(decode_errors().policy_close, before.policy_close + 1);
    let events = events.lock();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].packet_name.as_deref(), name);
    assert_eq!(events[0].class, class);
    assert!(matches!(events[0].action, BadPacketAction::Closed { .. }));
    assert_eq!(&*events[0].raw, &*raw);
    let alerts = alerts.lock();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert_eq!(alerts[0].reason, AlertReason::Closed);
    let disconnects = disconnects.lock();
    assert_eq!(disconnects.len(), 1, "{disconnects:?}");
    let reason = disconnects[0].as_deref().unwrap_or_default();
    assert!(
        reason.contains("no safe repair") || reason.contains("own entity"),
        "{reason}"
    );
    // disconnected: the client's world is gone
    assert!(!simulation.has_component::<WorldName>());
}

#[test]
fn bad_teleport_closes_the_connection() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    let raw = truncated_packet(
        ClientboundPlayerPosition {
            id: 1,
            change: PositionMoveRotation {
                pos: Vec3::new(100., 70., 100.),
                delta: Vec3::ZERO,
                look_direction: Default::default(),
            },
            relative: RelativeMovements::default(),
        },
        3,
    );
    closes(
        &mut simulation,
        raw,
        Some("player_position"),
        Some(PacketClass::Critical),
    );
}

#[test]
fn bad_packet_about_our_own_entity_closes_the_connection() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();

    // set_entity_data with our own id (0 in the login packet): the id is
    // there, the metadata is cut
    let mut raw = vec![];
    let good = azalea_protocol::write::serialize_packet(
        &azalea_protocol::packets::game::ClientboundGamePacket::SetEntityData(
            azalea_protocol::packets::game::ClientboundSetEntityData {
                id: MinecraftEntityId(0),
                packed_items: azalea_entity::EntityMetadataItems(vec![]),
            },
        ),
    )
    .unwrap();
    raw.extend_from_slice(&good[..good.len() - 1]);
    // a metadata index with no type after it
    raw.push(3);
    closes(
        &mut simulation,
        raw.into(),
        Some("set_entity_data"),
        Some(PacketClass::Entity),
    );
}

#[test]
fn bad_health_closes_the_connection() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    let raw = truncated_packet(
        ClientboundSetHealth {
            health: 3.,
            food: 20,
            saturation: 5.,
        },
        2,
    );
    closes(
        &mut simulation,
        raw,
        Some("set_health"),
        Some(PacketClass::Critical),
    );
}

#[test]
fn unknown_packet_id_closes_the_connection() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    // no clientbound game packet has id 0x7f00
    closes(
        &mut simulation,
        vec![0x80, 0xfe, 0x01, 1, 2].into(),
        None,
        None,
    );
}

#[test]
fn bad_configuration_packet_closes_the_connection() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Configuration);
    simulation.tick();
    let raw = truncated_packet(
        ClientboundUpdateEnabledFeatures {
            features: vec!["minecraft:vanilla".into()],
        },
        2,
    );
    let events = simulation.record_messages::<BadPacketEvent>();
    let disconnects = simulation.record_disconnects();
    let before = decode_errors();
    simulation.receive_raw_packet(raw);
    simulation.tick();
    simulation.tick();
    assert_eq!(decode_errors().policy_close, before.policy_close + 1);
    let events = events.lock();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].state, ConnectionProtocol::Configuration);
    assert_eq!(events[0].class, Some(PacketClass::Critical));
    assert!(matches!(events[0].action, BadPacketAction::Closed { .. }));
    assert_eq!(disconnects.lock().len(), 1);
}
