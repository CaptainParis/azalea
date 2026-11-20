use azalea_client::{packet::game::SendGamePacketEvent, test_utils::prelude::*};
use azalea_core::decode_errors::decode_errors;
use azalea_protocol::packets::{
    ConnectionProtocol,
    game::ServerboundSwing,
};

// A game packet sent while in the configuration state is dropped and counted as
// SendState, so the zero check in the live proofs sees it.
#[test]
fn test_game_packet_in_configuration_is_counted() {
    let _lock = init();

    let mut simulation = Simulation::new(ConnectionProtocol::Configuration);
    let entity = simulation.entity;
    let before = decode_errors();
    tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
        simulation.trigger(SendGamePacketEvent::new(
            entity,
            ServerboundSwing {
                hand: Default::default(),
            },
        ));
    });
    assert!(decode_errors().send_state > before.send_state);
}
