//! WP4 resource bounds: what a client keeps is bounded by what it can see.

use azalea_client::{local_player::WorldHolder, test_utils::prelude::*};
use azalea_core::position::ChunkPos;
use azalea_protocol::packets::{
    ConnectionProtocol,
    game::{ClientboundForgetLevelChunk, ClientboundSetChunkCacheCenter},
};

const SECTIONS: usize = 384 / 16;

fn weak_map_len(s: &Simulation) -> usize {
    let mut n = 0;
    s.with_component::<WorldHolder>(|wh| n = wh.shared.read().chunks.len());
    n
}

fn ring_count(s: &Simulation) -> usize {
    let mut n = 0;
    s.with_component::<WorldHolder>(|wh| n = wh.partial.read().chunks.loaded_count());
    n
}

/// A forgotten chunk and a chunk the ring evicts leave the shared weak map at
/// once (the map held a dead `Weak` per chunk ever received before).
#[test]
fn weak_map_pruned_on_forget_and_eviction() {
    let _lock = init();
    let mut s = Simulation::new(ConnectionProtocol::Game);
    s.receive_packet(default_login_packet());
    s.tick();
    // default client view distance 8: ring radius 11, 23 slots per side
    s.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), SECTIONS));
    s.receive_packet(make_basic_empty_chunk(ChunkPos::new(1, 0), SECTIONS));
    s.tick();
    assert_eq!(weak_map_len(&s), 2);
    assert_eq!(ring_count(&s), 2);

    s.receive_packet(ClientboundForgetLevelChunk {
        pos: ChunkPos::new(0, 0),
    });
    s.tick();
    assert_eq!(weak_map_len(&s), 1);
    assert_eq!(ring_count(&s), 1);

    // the centre moves 23 chunks east: (1, 0) is stale, (24, 0) takes its slot
    s.receive_packet(ClientboundSetChunkCacheCenter { x: 23, z: 0 });
    s.tick();
    assert_eq!(ring_count(&s), 1, "the stale slot still counts");
    s.receive_packet(make_basic_empty_chunk(ChunkPos::new(24, 0), SECTIONS));
    s.tick();
    assert_eq!(ring_count(&s), 1);
    assert_eq!(weak_map_len(&s), 1, "the evicted chunk's entry is gone");
    assert!(s.chunk(ChunkPos::new(1, 0)).is_none());
    assert!(s.chunk(ChunkPos::new(24, 0)).is_some());
}
