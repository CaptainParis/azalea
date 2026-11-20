//! A process-wide count of data that we received but couldn't decode.
//!
//! Some decode failures don't stop the connection: a packet that fails to
//! parse is logged and skipped, a registry entry that fails to parse is left
//! out, and so on. Every one of those places calls [`record_decode_error`],
//! so tests (and bots) can check [`decode_errors`] to see whether anything was
//! silently dropped.
//!
//! The counters are global atomics, shared by every client in the process.

use std::sync::atomic::{AtomicU64, Ordering};

/// Where a decode error happened.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DecodeErrorKind {
    /// A frame couldn't be split or decompressed, in `azalea_protocol::read`.
    Frame,
    /// A packet ID that doesn't exist in the current protocol state.
    UnknownPacketId,
    /// A packet's ID or body couldn't be parsed, or there were bytes left
    /// over after the body.
    PacketBody,
    /// A client packet handler got data it couldn't use and skipped it (for
    /// example entity metadata of the wrong type).
    PacketHandler,
    /// A registry entry from the configuration phase couldn't be parsed.
    Registry,
    /// Chunk data (sections, paletted containers, heightmaps) couldn't be
    /// parsed, or had to be ignored.
    Chunk,
    /// A packet handler couldn't find our own player's components (for
    /// example the `EntityIdIndex` or `Physics` of the client the packet
    /// arrived on), so the packet was dropped.
    HandlerState,
    /// A packet referred to an entity id that isn't in our entity index.
    ///
    /// With one client this means we missed or mis-decoded an add or remove;
    /// with several clients sharing a world it can be a despawn race, so
    /// multi-client tests may allow it explicitly with
    /// [`DecodeErrorCounts::total_except`].
    UnknownEntity,
    /// The entity indexes disagree with the ECS: an indexed entity is missing
    /// from the world or lacks components every entity has.
    EntityIndex,
    /// We tried to send a packet in a protocol state it doesn't belong to (for
    /// example a game packet while in configuration), so it was dropped.
    ///
    /// Not a decode error but a bug in our own code; it's counted here so
    /// the same zero check covers it.
    SendState,
}

const ALL_KINDS: [DecodeErrorKind; 10] = [
    DecodeErrorKind::Frame,
    DecodeErrorKind::UnknownPacketId,
    DecodeErrorKind::PacketBody,
    DecodeErrorKind::PacketHandler,
    DecodeErrorKind::Registry,
    DecodeErrorKind::Chunk,
    DecodeErrorKind::HandlerState,
    DecodeErrorKind::UnknownEntity,
    DecodeErrorKind::EntityIndex,
    DecodeErrorKind::SendState,
];

static FRAME: AtomicU64 = AtomicU64::new(0);
static UNKNOWN_PACKET_ID: AtomicU64 = AtomicU64::new(0);
static PACKET_BODY: AtomicU64 = AtomicU64::new(0);
static PACKET_HANDLER: AtomicU64 = AtomicU64::new(0);
static REGISTRY: AtomicU64 = AtomicU64::new(0);
static CHUNK: AtomicU64 = AtomicU64::new(0);
static HANDLER_STATE: AtomicU64 = AtomicU64::new(0);
static UNKNOWN_ENTITY: AtomicU64 = AtomicU64::new(0);
static ENTITY_INDEX: AtomicU64 = AtomicU64::new(0);
static SEND_STATE: AtomicU64 = AtomicU64::new(0);

fn counter(kind: DecodeErrorKind) -> &'static AtomicU64 {
    match kind {
        DecodeErrorKind::Frame => &FRAME,
        DecodeErrorKind::UnknownPacketId => &UNKNOWN_PACKET_ID,
        DecodeErrorKind::PacketBody => &PACKET_BODY,
        DecodeErrorKind::PacketHandler => &PACKET_HANDLER,
        DecodeErrorKind::Registry => &REGISTRY,
        DecodeErrorKind::Chunk => &CHUNK,
        DecodeErrorKind::HandlerState => &HANDLER_STATE,
        DecodeErrorKind::UnknownEntity => &UNKNOWN_ENTITY,
        DecodeErrorKind::EntityIndex => &ENTITY_INDEX,
        DecodeErrorKind::SendState => &SEND_STATE,
    }
}

/// Count one decode error of the given kind.
///
/// This doesn't log anything, the caller is expected to do that.
pub fn record_decode_error(kind: DecodeErrorKind) {
    counter(kind).fetch_add(1, Ordering::Relaxed);
}

/// The number of decode errors counted so far, by kind.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DecodeErrorCounts {
    pub frame: u64,
    pub unknown_packet_id: u64,
    pub packet_body: u64,
    pub packet_handler: u64,
    pub registry: u64,
    pub chunk: u64,
    pub handler_state: u64,
    pub unknown_entity: u64,
    pub entity_index: u64,
    pub send_state: u64,
}
impl DecodeErrorCounts {
    pub fn get(&self, kind: DecodeErrorKind) -> u64 {
        match kind {
            DecodeErrorKind::Frame => self.frame,
            DecodeErrorKind::UnknownPacketId => self.unknown_packet_id,
            DecodeErrorKind::PacketBody => self.packet_body,
            DecodeErrorKind::PacketHandler => self.packet_handler,
            DecodeErrorKind::Registry => self.registry,
            DecodeErrorKind::Chunk => self.chunk,
            DecodeErrorKind::HandlerState => self.handler_state,
            DecodeErrorKind::UnknownEntity => self.unknown_entity,
            DecodeErrorKind::EntityIndex => self.entity_index,
            DecodeErrorKind::SendState => self.send_state,
        }
    }

    /// The sum of every counter.
    pub fn total(&self) -> u64 {
        ALL_KINDS.iter().map(|&k| self.get(k)).sum()
    }

    /// The sum of every counter except the given kinds, for tests that
    /// explicitly expect some errors of those kinds (for example
    /// [`DecodeErrorKind::UnknownEntity`] with several clients).
    pub fn total_except(&self, allowed: &[DecodeErrorKind]) -> u64 {
        ALL_KINDS
            .iter()
            .filter(|k| !allowed.contains(k))
            .map(|&k| self.get(k))
            .sum()
    }
}

/// Get the number of decode errors counted so far in this process.
pub fn decode_errors() -> DecodeErrorCounts {
    let load = |kind| counter(kind).load(Ordering::Relaxed);
    DecodeErrorCounts {
        frame: load(DecodeErrorKind::Frame),
        unknown_packet_id: load(DecodeErrorKind::UnknownPacketId),
        packet_body: load(DecodeErrorKind::PacketBody),
        packet_handler: load(DecodeErrorKind::PacketHandler),
        registry: load(DecodeErrorKind::Registry),
        chunk: load(DecodeErrorKind::Chunk),
        handler_state: load(DecodeErrorKind::HandlerState),
        unknown_entity: load(DecodeErrorKind::UnknownEntity),
        entity_index: load(DecodeErrorKind::EntityIndex),
        send_state: load(DecodeErrorKind::SendState),
    }
}

/// Set every counter back to 0.
pub fn reset_decode_errors() {
    for kind in ALL_KINDS {
        counter(kind).store(0, Ordering::Relaxed);
    }
}
