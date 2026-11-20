use azalea_client::{SprintDirection, StartSprintEvent, test_utils::prelude::*};
use azalea_core::position::{BlockPos, ChunkPos, Vec3};
use azalea_entity::LookDirection;
use azalea_protocol::{
    common::movements::{PositionMoveRotation, RelativeMovements},
    packets::{
        ConnectionProtocol,
        game::{
            ClientboundBlockUpdate, ClientboundPlayerPosition, ServerboundGamePacket,
            ServerboundPlayerInput, s_player_command,
        },
    },
};
use azalea_registry::builtin::BlockKind;

/// 1.21.4 LocalPlayer.hasEnoughImpulseToStartSprinting checks the raw forward
/// impulse (`forwardImpulse >= 0.8`), so sprinting diagonally works. The
/// diagonal input is only normalized in Entity.getInputVector.
#[test]
fn test_correct_diagonal_sprint_movement() {
    let _lock = init();

    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    let sent_packets = SentPackets::new(&mut simulation);

    simulation.receive_packet(default_login_packet());
    simulation.tick();

    sent_packets.expect_tick_end();
    sent_packets.expect_empty();

    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), (384 + 64) / 16));
    for (x, z) in [(0, 0), (0, 1), (-1, 0), (-1, 1)] {
        simulation.receive_packet(ClientboundBlockUpdate {
            pos: BlockPos::new(x, 119, z),
            block_state: BlockKind::Stone.into(),
        });
    }
    simulation.receive_packet(ClientboundPlayerPosition {
        id: 1,
        change: PositionMoveRotation {
            pos: Vec3::new(0.5, 120., 0.5),
            delta: Vec3::ZERO,
            look_direction: LookDirection::default(),
        },
        relative: RelativeMovements::all_absolute(),
    });
    simulation.tick();
    simulation.tick();
    simulation.tick();
    sent_packets.clear();

    simulation.write_message(StartSprintEvent {
        entity: simulation.entity,
        direction: SprintDirection::ForwardRight,
    });
    simulation.tick();
    sent_packets.expect("PlayerInput", |p| {
        matches!(
            p,
            ServerboundGamePacket::PlayerInput(p)
            if *p == ServerboundPlayerInput { forward: true, right: true, sprint: true, ..Default::default() }
        )
    });
    sent_packets.expect("PlayerCommand(StartSprinting)", |p| {
        matches!(
            p,
            ServerboundGamePacket::PlayerCommand(p)
            if p.action == s_player_command::Action::StartSprinting
        )
    });
    // moving right (-x) and forward (+z) at the sprinting speed, with the
    // input normalized to length 1
    sent_packets.expect(
        "MovePlayerPos { x: 0.4080761112807846, z: 0.5919238887192154 }",
        |p| {
            matches!(
                p,
                ServerboundGamePacket::MovePlayerPos(p)
                if p.pos == Vec3::new(0.4080761112807846, 120., 0.5919238887192154)
            )
        },
    );
    sent_packets.expect_tick_end();
    sent_packets.expect_empty();
}
