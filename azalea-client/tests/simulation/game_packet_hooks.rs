use std::sync::Arc;

use azalea_client::{
    packet::game::{GamePacketHooks, SendGamePacketEvent},
    test_utils::prelude::*,
};
use azalea_protocol::packets::{
    ConnectionProtocol,
    game::{self, ClientboundGamePacket, ServerboundGamePacket},
};
use bevy_ecs::observer::On;
use parking_lot::Mutex;

#[test]
fn game_packet_hooks_see_packets_before_the_handler() {
    let _lock = init();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);

    // (what, ping id) in the order it happened
    let log = Arc::new(Mutex::new(Vec::<(&str, u32)>::new()));
    let l = log.clone();
    let entity = simulation.entity;
    simulation
        .app
        .insert_resource(GamePacketHooks(vec![Arc::new(move |c, p| {
            assert_eq!(c.entity, entity);
            assert!(!c.synthesised);
            if let ClientboundGamePacket::Ping(p) = p {
                l.lock().push(("hook", p.id));
            }
        })]));
    let l = log.clone();
    simulation
        .app
        .add_observer(move |sent: On<SendGamePacketEvent>| {
            if let ServerboundGamePacket::Pong(p) = &sent.packet {
                l.lock().push(("pong", p.id));
            }
        });

    simulation.receive_packet(game::ClientboundPing { id: 7 });
    simulation.tick();
    assert_eq!(*log.lock(), vec![("hook", 7), ("pong", 7)]);
}

#[test]
fn hooks_see_packets_the_bad_packet_policy_built_as_synthesised() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();

    let log = Arc::new(Mutex::new(Vec::<(u64, bool)>::new()));
    let l = log.clone();
    simulation
        .app
        .insert_resource(GamePacketHooks(vec![Arc::new(move |c, p| {
            if let ClientboundGamePacket::KeepAlive(k) = p {
                l.lock().push((k.id, c.synthesised));
            }
        })]));

    // a keep-alive the server sent as it is
    simulation.receive_packet(game::ClientboundKeepAlive { id: 1 });
    simulation.tick();
    // one with a byte left over: answered from its id, built by the policy
    let mut raw = azalea_protocol::write::serialize_packet(&ClientboundGamePacket::KeepAlive(
        game::ClientboundKeepAlive { id: 2 },
    ))
    .unwrap()
    .into_vec();
    raw.push(0x2a);
    simulation.receive_raw_packet(raw);
    simulation.tick();
    assert_eq!(*log.lock(), vec![(1, false), (2, true)]);
}
