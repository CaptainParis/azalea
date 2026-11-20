//! Used for Minecraft's chunk batching introduced in 23w31a (1.20.2). It's used
//! for making the server spread out how often it sends us chunk packets
//! depending on our receiving speed.

use std::{
    io::Cursor,
    time::{Duration, Instant},
};

use azalea_core::{
    decode_errors::{DecodeErrorKind, record_decode_error},
    position::ChunkPos,
};
use azalea_protocol::packets::game::{
    c_level_chunk_with_light::ClientboundLevelChunkWithLight,
    s_chunk_batch_received::ServerboundChunkBatchReceived,
};
use azalea_registry::identifier::Identifier;
use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;
use tracing::{error, trace, warn};

use crate::{
    bad_packet::UnknownChunks, inventory::InventorySystems, local_player::WorldHolder,
    packet::game::SendGamePacketEvent, respawn::perform_respawn,
};

pub struct ChunksPlugin;
impl Plugin for ChunksPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                handle_chunk_batch_start_event,
                handle_receive_chunk_event,
                handle_chunk_batch_finished_event,
            )
                .chain()
                .before(InventorySystems)
                .before(perform_respawn),
        )
        .add_message::<ReceiveChunkEvent>()
        .add_message::<ChunkBatchStartEvent>()
        .add_message::<ChunkBatchFinishedEvent>();
    }
}

#[derive(Message)]
pub struct ReceiveChunkEvent {
    pub entity: Entity,
    pub packet: ClientboundLevelChunkWithLight,
}

#[derive(Clone, Component, Debug)]
pub struct ChunkBatchInfo {
    pub start_time: Instant,
    pub aggregated_duration_per_chunk: Duration,
    pub old_samples_weight: u32,
}

#[derive(Message)]
pub struct ChunkBatchStartEvent {
    pub entity: Entity,
}
#[derive(Message)]
pub struct ChunkBatchFinishedEvent {
    pub entity: Entity,
    pub batch_size: u32,
}

pub fn handle_receive_chunk_event(
    mut commands: Commands,
    mut events: MessageReader<ReceiveChunkEvent>,
    mut query: Query<(&WorldHolder, Option<&mut UnknownChunks>)>,
) {
    for event in events.read() {
        let pos = ChunkPos::new(event.packet.x, event.packet.z);

        let Ok((local_player, mut unknown)) = query.get_mut(event.entity) else {
            warn!("Got a chunk but our player is missing components");
            // decode error hook: our own player's components are missing
            record_decode_error(DecodeErrorKind::HandlerState);
            continue;
        };
        // sent again: known
        crate::bad_packet::chunk_known(unknown.as_mut().map(|u| u.reborrow()), &pos);

        let mut world = local_player.shared.write();
        let mut partial_world = local_player.partial.write();

        // OPTIMIZATION: if we already know about the chunk from the shared
        // world (and not ourselves), then we don't need to parse it
        // again. This is only used when we have a shared world, since
        // we check that the chunk isn't currently owned by this client.
        let shared_chunk = world.chunks.get(&pos);
        let this_client_has_chunk = partial_world.chunks.limited_get(&pos).is_some();

        if !this_client_has_chunk && let Some(shared_chunk) = shared_chunk {
            trace!("Skipping parsing chunk {pos:?} because we already know about it");
            partial_world
                .chunks
                .limited_set_and_prune(&pos, Some(shared_chunk), &mut world.chunks);
            continue;
        }

        let heightmaps = &event.packet.chunk_data.heightmaps;

        // the biome registry sizes globally paletted biome storage
        let biome_registry_size = world
            .registries
            .extra
            .get(&Identifier::new("minecraft:worldgen/biome"))
            .map(|r| r.map.len());
        if let Err(e) = partial_world
            .chunks
            .replace_with_packet_data_and_biome_registry_size(
                &pos,
                &mut Cursor::new(&event.packet.chunk_data.data),
                heightmaps,
                &mut world.chunks,
                biome_registry_size,
            )
        {
            error!(
                "Couldn't set chunk data: {e}. World height: {}",
                world.chunks.height()
            );
            // decode error hook: chunks that fail to parse are unknown
            // (treated as unloaded, bad packet policy)
            record_decode_error(DecodeErrorKind::Chunk);
            record_decode_error(DecodeErrorKind::ChunkUnknown);
            partial_world
                .chunks
                .limited_set_and_prune(&pos, None, &mut world.chunks);
            match unknown {
                Some(mut u) => {
                    u.chunks.insert(pos);
                }
                None => {
                    commands.entity(event.entity).insert(UnknownChunks {
                        chunks: [pos].into(),
                        ..Default::default()
                    });
                }
            }
        }
    }
}

impl ChunkBatchInfo {
    pub fn batch_finished(&mut self, batch_size: u32) {
        if batch_size == 0 {
            return;
        }
        let batch_duration = self.start_time.elapsed();
        // network-safe: batch_size is checked != 0 above
        let duration_per_chunk = batch_duration / batch_size;
        let clamped_duration = Duration::clamp(
            duration_per_chunk,
            self.aggregated_duration_per_chunk / 3,
            self.aggregated_duration_per_chunk * 3,
        );
        self.aggregated_duration_per_chunk =
            ((self.aggregated_duration_per_chunk * self.old_samples_weight) + clamped_duration)
                / (self.old_samples_weight + 1);
        self.old_samples_weight = u32::min(49, self.old_samples_weight + 1);
    }

    pub fn desired_chunks_per_tick(&self) -> f32 {
        (7000000. / self.aggregated_duration_per_chunk.as_nanos() as f64) as f32
    }
}

pub fn handle_chunk_batch_start_event(
    mut query: Query<&mut ChunkBatchInfo>,
    mut events: MessageReader<ChunkBatchStartEvent>,
) {
    for event in events.read() {
        let Ok(mut chunk_batch_info) = query.get_mut(event.entity) else {
            error!("Got chunk batch start but our player doesn't have ChunkBatchInfo");
            // decode error hook: the batch is not timed
            record_decode_error(DecodeErrorKind::HandlerState);
            continue;
        };
        chunk_batch_info.start_time = Instant::now();
    }
}

pub fn handle_chunk_batch_finished_event(
    mut query: Query<&mut ChunkBatchInfo>,
    mut events: MessageReader<ChunkBatchFinishedEvent>,
    mut commands: Commands,
) {
    for event in events.read() {
        let Ok(mut chunk_batch_info) = query.get_mut(event.entity) else {
            error!("Got chunk batch finished but our player doesn't have ChunkBatchInfo");
            // decode error hook: no chunk_batch_received is sent, so the server
            // stops sending chunks
            record_decode_error(DecodeErrorKind::HandlerState);
            continue;
        };
        chunk_batch_info.batch_finished(event.batch_size);
        let desired_chunks_per_tick = chunk_batch_info.desired_chunks_per_tick();
        commands.trigger(SendGamePacketEvent::new(
            event.entity,
            ServerboundChunkBatchReceived {
                desired_chunks_per_tick,
            },
        ));
    }
}

#[derive(Clone, Debug)]
pub struct ChunkReceiveSpeedAccumulator {
    batch_sizes: Vec<u32>,
    /// as milliseconds
    batch_durations: Vec<u32>,
    index: usize,
    filled_size: usize,
}
impl ChunkReceiveSpeedAccumulator {
    pub fn new(capacity: usize) -> Self {
        // at least one entry, so the index always has a slot
        let capacity = capacity.max(1);
        Self {
            batch_sizes: vec![0; capacity],
            batch_durations: vec![0; capacity],
            index: 0,
            filled_size: 0,
        }
    }

    pub fn accumulate(&mut self, batch_size: u32, batch_duration: Duration) {
        // network-safe: index is always < len: it starts at 0 and wraps by `%
        // len`, and new() makes len at least 1
        self.batch_sizes[self.index] = batch_size;
        // network-safe: index < len, as above
        self.batch_durations[self.index] =
            f32::clamp(batch_duration.as_millis() as f32, 0., 15000.) as u32;
        // network-safe: len is at least 1 (new())
        self.index = (self.index + 1) % self.batch_sizes.len();
        if self.filled_size < self.batch_sizes.len() {
            self.filled_size += 1;
        }
    }

    pub fn get_millis_per_chunk(&self) -> f64 {
        let mut total_batch_size = 0;
        let mut total_batch_duration = 0;
        for i in 0..self.filled_size {
            // network-safe: i < filled_size <= len
            total_batch_size += self.batch_sizes[i];
            // network-safe: i < filled_size <= len
            total_batch_duration += self.batch_durations[i];
        }
        if total_batch_size == 0 {
            return 0.;
        }
        // network-safe: f64 division, the zero total is handled above
        total_batch_duration as f64 / total_batch_size as f64
    }
}

impl Default for ChunkBatchInfo {
    fn default() -> Self {
        Self {
            start_time: Instant::now(),
            aggregated_duration_per_chunk: Duration::from_millis(2),
            old_samples_weight: 1,
        }
    }
}
