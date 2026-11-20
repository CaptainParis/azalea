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
}

static FRAME: AtomicU64 = AtomicU64::new(0);
static UNKNOWN_PACKET_ID: AtomicU64 = AtomicU64::new(0);
static PACKET_BODY: AtomicU64 = AtomicU64::new(0);
static PACKET_HANDLER: AtomicU64 = AtomicU64::new(0);
static REGISTRY: AtomicU64 = AtomicU64::new(0);
static CHUNK: AtomicU64 = AtomicU64::new(0);

fn counter(kind: DecodeErrorKind) -> &'static AtomicU64 {
    match kind {
        DecodeErrorKind::Frame => &FRAME,
        DecodeErrorKind::UnknownPacketId => &UNKNOWN_PACKET_ID,
        DecodeErrorKind::PacketBody => &PACKET_BODY,
        DecodeErrorKind::PacketHandler => &PACKET_HANDLER,
        DecodeErrorKind::Registry => &REGISTRY,
        DecodeErrorKind::Chunk => &CHUNK,
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
        }
    }

    /// The sum of every counter.
    pub fn total(&self) -> u64 {
        self.frame
            + self.unknown_packet_id
            + self.packet_body
            + self.packet_handler
            + self.registry
            + self.chunk
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
    }
}

/// Set every counter back to 0.
pub fn reset_decode_errors() {
    for kind in [
        DecodeErrorKind::Frame,
        DecodeErrorKind::UnknownPacketId,
        DecodeErrorKind::PacketBody,
        DecodeErrorKind::PacketHandler,
        DecodeErrorKind::Registry,
        DecodeErrorKind::Chunk,
    ] {
        counter(kind).store(0, Ordering::Relaxed);
    }
}
