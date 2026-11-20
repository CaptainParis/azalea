use azalea_client::test_utils::prelude::*;
use azalea_core::{decode_errors::decode_errors, position::Vec3};
use azalea_entity::{LookDirection, Physics};
use azalea_protocol::{
    common::movements::{PositionMoveRotation, RelativeMovements},
    packets::{
        ConnectionProtocol,
        game::{ClientboundPlayerPosition, ServerboundAcceptTeleportation, ServerboundGamePacket},
    },
};

// 1.21.4 handleMovePlayer always sends ServerboundAcceptTeleportation with the
// server's id; without our player's Physics we can't apply the position, but we
// still accept (or the server would wait for it), and count HandlerState.
#[test]
fn test_teleport_without_physics_is_still_accepted() {
    let _lock = init();

    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    let sent_packets = SentPackets::new(&mut simulation);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    sent_packets.clear();

    let entity = simulation.entity;
    simulation
        .app
        .world_mut()
        .entity_mut(entity)
        .remove::<Physics>();

    let before = decode_errors();
    simulation.receive_packet(ClientboundPlayerPosition {
        id: 7,
        change: PositionMoveRotation {
            pos: Vec3::new(1.5, 70., 2.5),
            delta: Vec3::ZERO,
            look_direction: LookDirection::default(),
        },
        relative: RelativeMovements::all_absolute(),
    });
    // the handler warns; keep that off the test layer that panics on warnings
    tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
        simulation.tick();
    });

    sent_packets.expect("AcceptTeleportation", |p| {
        matches!(
            p,
            ServerboundGamePacket::AcceptTeleportation(ServerboundAcceptTeleportation { id: 7 })
        )
    });
    assert!(decode_errors().handler_state > before.handler_state);
}
