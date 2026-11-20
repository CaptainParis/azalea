//! Bad packet policy in configuration: a display-only configuration packet
//! (server links) that can't be decoded is dropped and counted, also in the
//! configuration phase a proxy runs when it moves us from the queue to the
//! main server (`start_configuration` in the middle of the session). The
//! session goes on into play.

use azalea_client::{
    InConfigState,
    bad_packet::{BadPacketAction, BadPacketEvent, PacketClass},
    connection::RawConnection,
    test_utils::prelude::*,
};
use azalea_core::decode_errors::decode_errors;
use azalea_protocol::packets::{
    ConnectionProtocol, ProtocolPacket,
    config::{
        ClientboundConfigPacket, ClientboundFinishConfiguration, ClientboundRegistryData,
        ClientboundServerLinks,
    },
    game::ClientboundStartConfiguration,
};
use azalea_registry::identifier::Identifier;
use azalea_world::WorldName;

fn alive(simulation: &Simulation) -> bool {
    simulation
        .app
        .world()
        .get::<RawConnection>(simulation.entity)
        .is_some_and(|c| c.is_alive())
}

fn bad_server_links() -> Box<[u8]> {
    let id = ClientboundConfigPacket::ServerLinks(ClientboundServerLinks { links: vec![] }).id();
    let mut raw = Vec::new();
    azalea_buf::AzBufVar::azalea_write_var(&id, &mut raw).unwrap();
    // five links announced, none there
    raw.push(5);
    raw.into()
}

#[test]
fn bad_server_links_during_a_transfer_keeps_the_session() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    assert!(simulation.has_component::<WorldName>());

    // the proxy moves us to the main server
    simulation.receive_packet(ClientboundStartConfiguration);
    simulation.tick();
    assert!(simulation.has_component::<InConfigState>());

    let events = simulation.record_messages::<BadPacketEvent>();
    let disconnects = simulation.record_disconnects();
    let before = decode_errors();
    simulation.receive_raw_packet(bad_server_links());
    simulation.tick();
    simulation.tick();

    let after = decode_errors();
    assert_eq!(after.bad_packet_dropped, before.bad_packet_dropped + 1);
    assert_eq!(after.policy_close, before.policy_close);
    {
        let events = events.lock();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].state, ConnectionProtocol::Configuration);
        assert_eq!(events[0].packet_name.as_deref(), Some("server_links"));
        assert_eq!(events[0].class, Some(PacketClass::StateFree));
        assert_eq!(events[0].action, BadPacketAction::Dropped);
    }
    assert!(disconnects.lock().is_empty());
    assert!(alive(&simulation));

    // the rest of the configuration, then play on the main server
    simulation.receive_packet(ClientboundFinishConfiguration);
    simulation.tick();
    assert!(!simulation.has_component::<InConfigState>());
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    simulation.tick();
    assert!(simulation.has_component::<WorldName>());
    assert!(alive(&simulation));
    assert!(disconnects.lock().is_empty());
}

#[test]
fn bad_registry_data_still_closes() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Configuration);
    simulation.tick();
    let events = simulation.record_messages::<BadPacketEvent>();
    let id = ClientboundConfigPacket::RegistryData(ClientboundRegistryData {
        registry_id: Identifier::new("minecraft:dimension_type"),
        entries: Default::default(),
    })
    .id();
    let mut raw = Vec::new();
    azalea_buf::AzBufVar::azalea_write_var(&id, &mut raw).unwrap();
    // a registry id that's cut
    raw.extend_from_slice(&[10, b'm']);
    simulation.receive_raw_packet(raw);
    simulation.tick();
    simulation.tick();
    let events = events.lock();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].class, Some(PacketClass::Critical));
    assert!(matches!(events[0].action, BadPacketAction::Closed { .. }));
}
