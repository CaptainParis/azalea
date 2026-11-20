//! Bad packet policy, chunks and blocks: a block update that can't be decoded
//! makes its chunk unknown (unloaded); the server sending the chunk again
//! makes it known.

use azalea_block::BlockState;
use azalea_client::{
    bad_packet::{BadPacketAction, BadPacketEvent, PacketClass, UnknownChunks},
    connection::RawConnection,
    test_utils::prelude::*,
};
use azalea_core::{
    decode_errors::decode_errors,
    position::{BlockPos, ChunkPos},
};
use azalea_protocol::packets::{ConnectionProtocol, game::ClientboundBlockUpdate};

#[test]
fn bad_block_update_makes_the_chunk_unknown() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), (384 + 64) / 16));
    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(-1, 0), (384 + 64) / 16));
    simulation.tick();
    assert!(simulation.chunk(ChunkPos::new(-1, 0)).is_some());
    let events = simulation.record_messages::<BadPacketEvent>();
    let before = decode_errors();

    // the position is there (chunk -1 0), the block state is cut
    let mut raw = truncated_packet(
        ClientboundBlockUpdate {
            pos: BlockPos::new(-3, 70, 5),
            block_state: BlockState::from(azalea_registry::builtin::BlockKind::Stone),
        },
        8,
    )
    .into_vec();
    // a VarInt that never ends
    raw.push(0x80);
    simulation.receive_raw_packet(raw);
    simulation.tick();

    assert_eq!(decode_errors().chunk_unknown, before.chunk_unknown + 1);
    {
        let events = events.lock();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].class, Some(PacketClass::Chunk));
        assert_eq!(
            events[0].action,
            BadPacketAction::ChunkUnknown {
                chunks: vec![ChunkPos::new(-1, 0)]
            }
        );
    }
    // treated as unloaded; the other chunk is untouched
    assert!(simulation.chunk(ChunkPos::new(-1, 0)).is_none());
    assert!(simulation.get_block_state(BlockPos::new(-3, 70, 5)).is_none());
    assert!(simulation.chunk(ChunkPos::new(0, 0)).is_some());
    assert!(
        simulation
            .component::<UnknownChunks>()
            .chunks
            .contains(&ChunkPos::new(-1, 0))
    );
    assert!(simulation.app.world().get::<RawConnection>(simulation.entity).unwrap().is_alive());

    // the server sends it again: known
    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(-1, 0), (384 + 64) / 16));
    simulation.tick();
    assert!(simulation.chunk(ChunkPos::new(-1, 0)).is_some());
    assert!(simulation.component::<UnknownChunks>().chunks.is_empty());
}

#[test]
fn chunk_sent_before_a_bad_update_in_the_same_read_stays_unknown() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), (384 + 64) / 16));
    simulation.receive_raw_packet(truncated_packet(
        ClientboundBlockUpdate {
            pos: BlockPos::new(1, 70, 1),
            block_state: BlockState::from(azalea_registry::builtin::BlockKind::Stone),
        },
        8,
    ));
    simulation.tick();
    simulation.tick();
    assert!(simulation.chunk(ChunkPos::new(0, 0)).is_none());
}
