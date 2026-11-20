//! Bad packet policy, state-free class: a sound that can't be decoded is
//! dropped and counted, the connection stays up, the raw bytes are kept, and
//! the second bad packet with the same id raises an alert.

use azalea_client::{
    bad_packet::{AlertReason, BadPacketAction, BadPacketAlert, BadPacketEvent, PacketClass},
    connection::RawConnection,
    test_utils::prelude::*,
};
use azalea_core::decode_errors::decode_errors;
use azalea_protocol::packets::{
    ConnectionProtocol,
    game::{ClientboundSound, ClientboundSystemChat},
};
use azalea_registry::{Holder, builtin::SoundEvent};

fn sound() -> ClientboundSound {
    ClientboundSound {
        sound: Holder::Reference(SoundEvent::EntityCowAmbient),
        source: azalea_protocol::packets::game::c_sound::SoundSource::Neutral,
        x: 8,
        y: 64,
        z: 8,
        volume: 1.,
        pitch: 1.,
        seed: 7,
    }
}

#[test]
fn bad_state_free_packet_is_dropped_and_counted() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    let events = simulation.record_messages::<BadPacketEvent>();
    let alerts = simulation.record_messages::<BadPacketAlert>();
    let disconnects = simulation.record_disconnects();
    let before = decode_errors();

    let raw = truncated_packet(sound(), 3);
    simulation.receive_raw_packet(raw.clone());
    simulation.tick();

    let after = decode_errors();
    assert_eq!(after.packet_body, before.packet_body + 1);
    assert_eq!(after.bad_packet_dropped, before.bad_packet_dropped + 1);
    assert_eq!(after.policy_close, before.policy_close);
    {
        let events = events.lock();
        assert_eq!(events.len(), 1, "{events:?}");
        let e = &events[0];
        assert_eq!(e.class, Some(PacketClass::StateFree));
        assert_eq!(e.action, BadPacketAction::Dropped);
        assert_eq!(e.packet_name.as_deref(), Some("sound"));
        // the raw bytes stay available to the connection layer
        assert_eq!(&*e.raw, &*raw);
    }
    // once is no alert
    assert!(alerts.lock().is_empty());
    assert!(
        simulation
            .app
            .world()
            .get::<RawConnection>(simulation.entity)
            .unwrap()
            .is_alive()
    );
    assert!(disconnects.lock().is_empty());

    // a good packet of the same kind still works, and other state-free ids
    // are counted on their own
    simulation.receive_packet(sound());
    simulation.receive_raw_packet(truncated_packet(
        ClientboundSystemChat {
            content: "hi".into(),
            overlay: false,
        },
        1,
    ));
    simulation.tick();
    assert!(alerts.lock().is_empty());

    // the second bad sound in the hour raises an alert
    simulation.receive_raw_packet(raw);
    simulation.tick();
    let alerts = alerts.lock();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert_eq!(alerts[0].reason, AlertReason::Repeated);
    assert_eq!(alerts[0].count_in_window, 2);
    assert_eq!(alerts[0].packet_name.as_deref(), Some("sound"));
    assert!(
        simulation
            .app
            .world()
            .get::<RawConnection>(simulation.entity)
            .unwrap()
            .is_alive()
    );
    assert_eq!(decode_errors().policy_close, before.policy_close);
}
