//! A process-wide count of data that we received but couldn't decode.
//!
//! Some decode failures don't stop the connection: a packet that fails to
//! parse is logged and skipped, a registry entry that fails to parse is kept
//! as a placeholder, and so on. Every one of those places calls
//! [`record_decode_error`], so tests (and bots) can check [`decode_errors`] to
//! see whether anything was silently dropped.
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
    /// A frame length prefix wider than 21 bits (3 bytes), as vanilla's
    /// `Varint21FrameDecoder`. A frame error: the stream is lost.
    FrameLength,
    /// A compressed packet that declares more than 8 388 608 bytes, or
    /// whose inflated size isn't the declared one (`CompressionDecoder`). A
    /// frame error: the stream is lost.
    DecompressedSize,
    /// A string longer than its limit
    /// ([`azalea_buf::limits::LimitKind::StringLength`]).
    StringLength,
    /// A collection length prefix that is negative, over its limit, or more
    /// than the bytes left can hold
    /// ([`azalea_buf::limits::LimitKind::CollectionLength`]).
    CollectionLength,
    /// A packet with more zero-size elements than the per-packet budget
    /// ([`azalea_buf::limits::LimitKind::ElementBudget`]).
    ElementBudget,
    /// Network NBT nested deeper than 512
    /// ([`azalea_buf::limits::LimitKind::NbtDepth`]).
    NbtDepth,
    /// Network NBT over the 2 097 152 byte accounting quota
    /// ([`azalea_buf::limits::LimitKind::NbtSize`]).
    NbtSize,
    /// A self-containing type (item stacks in containers, slot displays)
    /// nested deeper than 512
    /// ([`azalea_buf::limits::LimitKind::NestingDepth`]).
    NestingDepth,
    /// Collections in one packet that would hold more than 64 MiB in memory
    /// ([`azalea_buf::limits::LimitKind::PacketMemory`]).
    PacketMemory,
    /// A dimension type from registry data outside vanilla's bounds (height,
    /// min_y, logical height); the entry is rejected before any chunk
    /// storage is sized from it.
    DimensionType,
    /// A panic on a bot's own thread, or in a task it spawned on a shared
    /// pool (the task carries the bot's id). The bot's connection is closed
    /// and its app dropped; it's never resumed.
    ///
    /// Not a decode error, but a fault the same zero check must see.
    Panic,
    /// A panic on a thread that doesn't belong to any bot (a task spawned
    /// without a bot id): a process-level fault.
    ProcessPanic,
    /// A bot whose game tick didn't advance for the watchdog's stall time:
    /// its connection is closed and its thread abandoned.
    Stall,
    /// Bad packet policy (WP2 section 4): a packet that couldn't be decoded
    /// was dropped with no model change (state-free packets: sounds,
    /// particles, chat display, titles). The decode error itself is counted
    /// under its own kind; this counts the outcome.
    BadPacketDropped,
    /// Bad packet policy: an inventory or container packet couldn't be
    /// decoded, so the inventory was marked unknown and a resync click sent.
    InventoryUnknown,
    /// Bad packet policy: the inventory resync didn't arrive in time, so the
    /// connection was closed.
    InventoryResyncFailed,
    /// Bad packet policy: a packet about another entity couldn't be decoded,
    /// so that entity keeps its last good state, marked stale.
    StaleEntity,
    /// Bad packet policy: a chunk or block packet couldn't be decoded (or
    /// its chunk data couldn't be used), so the chunk is unknown (unloaded).
    ChunkUnknown,
    /// Bad packet policy: a packet with no safe repair (login, respawn,
    /// teleport, start configuration, our own entity, frame errors) couldn't
    /// be decoded, so the connection was closed for a reconnect.
    PolicyClose,
    /// Bad packet policy: a packet that couldn't be decoded whole was
    /// applied from the part of its body that could be read, as vanilla
    /// would have applied it (keep-alive, ping, chunk batch finished and
    /// block changed ack get the reply the server waits for; an explosion's
    /// knockback is applied, its particle and sound dropped).
    PrefixApplied,
    /// Bad packet policy: an entity spawn couldn't be decoded, so a
    /// placeholder entity was spawned from the fields that could be read
    /// (id, uuid, type, position), marked stale.
    PlaceholderEntity,
    /// Bad packet policy: a tab list update or removal couldn't be decoded,
    /// so the client's tab list (and the game profiles taken from it) is
    /// stale until the next login.
    TabListStale,
    /// A player entity was spawned whose uuid isn't in our tab list (vanilla
    /// would skip the spawn); it's kept, without a game profile, marked
    /// `MissingProfile` (azalea-client).
    MissingProfile,
}

impl DecodeErrorKind {
    /// Whether this kind means the byte stream itself is lost (bad length,
    /// inflate failure or size mismatch), so the connection must be closed.
    pub fn is_frame_error(self) -> bool {
        matches!(
            self,
            DecodeErrorKind::Frame
                | DecodeErrorKind::FrameLength
                | DecodeErrorKind::DecompressedSize
        )
    }
}

impl From<azalea_buf::limits::LimitKind> for DecodeErrorKind {
    fn from(kind: azalea_buf::limits::LimitKind) -> Self {
        use azalea_buf::limits::LimitKind;
        match kind {
            LimitKind::StringLength => DecodeErrorKind::StringLength,
            LimitKind::CollectionLength => DecodeErrorKind::CollectionLength,
            LimitKind::ElementBudget => DecodeErrorKind::ElementBudget,
            LimitKind::NbtDepth => DecodeErrorKind::NbtDepth,
            LimitKind::NbtSize => DecodeErrorKind::NbtSize,
            LimitKind::NestingDepth => DecodeErrorKind::NestingDepth,
            LimitKind::PacketMemory => DecodeErrorKind::PacketMemory,
        }
    }
}

/// Every kind, and the name of its field in [`DecodeErrorCounts`].
macro_rules! kinds {
    ($($kind:ident => $field:ident),* $(,)?) => {
        const ALL_KINDS: &[DecodeErrorKind] = &[$(DecodeErrorKind::$kind),*];

        static COUNTERS: [AtomicU64; ALL_KINDS.len()] =
            [const { AtomicU64::new(0) }; ALL_KINDS.len()];

        /// The number of decode errors counted so far, by kind.
        #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
        pub struct DecodeErrorCounts {
            $(pub $field: u64,)*
        }

        impl DecodeErrorCounts {
            pub fn get(&self, kind: DecodeErrorKind) -> u64 {
                match kind {
                    $(DecodeErrorKind::$kind => self.$field,)*
                }
            }
        }

        /// Get the number of decode errors counted so far in this process.
        pub fn decode_errors() -> DecodeErrorCounts {
            DecodeErrorCounts {
                $($field: counter(DecodeErrorKind::$kind).load(Ordering::Relaxed),)*
            }
        }
    };
}

kinds! {
    Frame => frame,
    UnknownPacketId => unknown_packet_id,
    PacketBody => packet_body,
    PacketHandler => packet_handler,
    Registry => registry,
    Chunk => chunk,
    HandlerState => handler_state,
    UnknownEntity => unknown_entity,
    EntityIndex => entity_index,
    SendState => send_state,
    FrameLength => frame_length,
    DecompressedSize => decompressed_size,
    StringLength => string_length,
    CollectionLength => collection_length,
    ElementBudget => element_budget,
    NbtDepth => nbt_depth,
    NbtSize => nbt_size,
    NestingDepth => nesting_depth,
    PacketMemory => packet_memory,
    DimensionType => dimension_type,
    Panic => panic,
    ProcessPanic => process_panic,
    Stall => stall,
    BadPacketDropped => bad_packet_dropped,
    InventoryUnknown => inventory_unknown,
    InventoryResyncFailed => inventory_resync_failed,
    StaleEntity => stale_entity,
    ChunkUnknown => chunk_unknown,
    PolicyClose => policy_close,
    PrefixApplied => prefix_applied,
    PlaceholderEntity => placeholder_entity,
    TabListStale => tab_list_stale,
    MissingProfile => missing_profile,
}

fn counter(kind: DecodeErrorKind) -> &'static AtomicU64 {
    // network-safe: ALL_KINDS lists every variant once, in declaration
    // order, and COUNTERS has one counter per entry
    &COUNTERS[kind as usize]
}

/// Count one decode error of the given kind.
///
/// This doesn't log anything, the caller is expected to do that.
pub fn record_decode_error(kind: DecodeErrorKind) {
    counter(kind).fetch_add(1, Ordering::Relaxed);
}

impl DecodeErrorCounts {
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

/// Set every counter back to 0.
pub fn reset_decode_errors() {
    for c in &COUNTERS {
        c.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_in_declaration_order() {
        for (i, &kind) in ALL_KINDS.iter().enumerate() {
            assert_eq!(kind as usize, i, "{kind:?}");
        }
        // and each kind's field is its own
        let before = decode_errors();
        for &kind in ALL_KINDS {
            record_decode_error(kind);
        }
        let after = decode_errors();
        for &kind in ALL_KINDS {
            assert!(after.get(kind) > before.get(kind), "{kind:?}");
        }
    }
}
