//! Size and depth limits for data read from the network.
//!
//! The values follow vanilla 1.21.4 where vanilla has one
//! (`FriendlyByteBuf`, `ByteBufCodecs`, `NbtAccounter`); the rest are ours
//! and documented on each constant. Every limit error is a
//! [`BufReadError::Limit`], and the kind of the last limit that tripped on
//! this thread can be read with [`take_limit_hit`], so the packet reader can
//! count it in the decode error counter even if a reader further up wrapped
//! or replaced the error.

use std::{cell::Cell, fmt, io::Cursor, mem};

use crate::{AzBufVar, BufReadError};

/// Vanilla `ByteBufCodecs.MAX_INITIAL_COLLECTION_SIZE`: a collection never
/// preallocates more elements than this, it grows as elements are read.
pub const MAX_INITIAL_COLLECTION_SIZE: usize = 65536;

/// Ours: a collection never preallocates more than this many bytes either,
/// so a large element type can't turn a small length prefix into a large
/// allocation.
pub const MAX_INITIAL_COLLECTION_BYTES: usize = 1 << 20;

/// Ours: the number of elements of a type that may encode to zero bytes
/// (see [`crate::AzBuf::MIN_ENCODED_SIZE`]) that one packet may read. It's the
/// largest decompressed packet (8 388 608 bytes), so it never binds for an
/// element type that really reads at least one byte.
pub const ELEMENT_BUDGET: u64 = 8_388_608;

/// Ours: how many bytes of collection elements one packet may hold in memory
/// (elements × `size_of` of the element type, counted when a collection's
/// length is read, before anything is allocated). The minimum-wire-size check
/// alone isn't enough: an empty item stack is one byte on the wire and dozens
/// in memory, so an 8 MiB packet of them would be hundreds of MiB. No vanilla
/// packet comes near this (the largest real ones, chunks and registries, hold
/// a few MiB).
pub const PACKET_MEMORY_BUDGET: u64 = 64 << 20;

/// Ours: how deep a type that contains itself on the wire (item stacks in
/// containers, slot displays in slot displays) may nest. The same value as
/// the network NBT depth.
pub const MAX_NESTING_DEPTH: u32 = 512;

/// Vanilla `NbtAccounter.MAX_STACK_DEPTH`, the depth of lists and compounds
/// in network NBT.
pub const NBT_MAX_DEPTH: u32 = 512;

/// Vanilla `FriendlyByteBuf.DEFAULT_NBT_QUOTA`, what `NbtAccounter` lets one
/// network NBT tag account for (its estimate of the tag's memory use).
pub const NBT_QUOTA: u64 = 2_097_152;

/// Which limit a [`BufReadError::Limit`] is about.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LimitKind {
    /// A string longer than its maximum (in bytes or in UTF-16 units).
    StringLength,
    /// A collection length prefix that is negative, above the collection's
    /// maximum, or larger than the bytes left could hold.
    CollectionLength,
    /// More zero-size elements in one packet than [`ELEMENT_BUDGET`].
    ElementBudget,
    /// Network NBT deeper than [`NBT_MAX_DEPTH`].
    NbtDepth,
    /// Network NBT that accounts for more than [`NBT_QUOTA`].
    NbtSize,
    /// A self-containing type nested deeper than [`MAX_NESTING_DEPTH`].
    NestingDepth,
    /// Collections in one packet that would hold more than
    /// [`PACKET_MEMORY_BUDGET`] bytes in memory.
    PacketMemory,
}

impl fmt::Display for LimitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            LimitKind::StringLength => "string length",
            LimitKind::CollectionLength => "collection length",
            LimitKind::ElementBudget => "element budget",
            LimitKind::NbtDepth => "NBT depth",
            LimitKind::NbtSize => "NBT size",
            LimitKind::NestingDepth => "nesting depth",
            LimitKind::PacketMemory => "packet memory",
        };
        f.write_str(s)
    }
}

thread_local! {
    static LIMIT_HIT: Cell<Option<LimitKind>> = const { Cell::new(None) };
    static ELEMENTS_LEFT: Cell<Option<u64>> = const { Cell::new(None) };
    static MEMORY_LEFT: Cell<Option<u64>> = const { Cell::new(None) };
    static NESTING: Cell<u32> = const { Cell::new(0) };
}

/// Make a [`BufReadError::Limit`] and remember its kind for
/// [`take_limit_hit`].
pub fn limit_error(kind: LimitKind, value: u64, max: u64) -> BufReadError {
    LIMIT_HIT.with(|c| c.set(Some(kind)));
    BufReadError::Limit { kind, value, max }
}

/// The kind of the last limit that tripped on this thread since the last
/// call, if any.
pub fn take_limit_hit() -> Option<LimitKind> {
    LIMIT_HIT.with(|c| c.take())
}

/// Run `f` (the decoding of one packet) with a fresh [`ELEMENT_BUDGET`] and
/// [`PACKET_MEMORY_BUDGET`]. Outside of this, neither is budgeted.
pub fn with_element_budget<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(Option<u64>, Option<u64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ELEMENTS_LEFT.with(|c| c.set(self.0));
            MEMORY_LEFT.with(|c| c.set(self.1));
        }
    }
    let _restore = Restore(
        ELEMENTS_LEFT.with(|c| c.replace(Some(ELEMENT_BUDGET))),
        MEMORY_LEFT.with(|c| c.replace(Some(PACKET_MEMORY_BUDGET))),
    );
    f()
}

/// Take the memory of `length` elements of `T` (`length × size_of::<T>()`)
/// from the current packet's [`PACKET_MEMORY_BUDGET`]. Call it once a
/// collection's length is known, before reading or allocating its elements.
pub fn charge_memory<T>(length: usize) -> Result<(), BufReadError> {
    let bytes = (length as u64).saturating_mul(mem::size_of::<T>() as u64);
    MEMORY_LEFT.with(|c| match c.get() {
        None => Ok(()),
        Some(left) if bytes <= left => {
            c.set(Some(left - bytes));
            Ok(())
        }
        Some(_) => Err(limit_error(
            LimitKind::PacketMemory,
            bytes,
            PACKET_MEMORY_BUDGET,
        )),
    })
}

/// Take `n` elements from the current packet's element budget.
pub fn charge_elements(n: u64) -> Result<(), BufReadError> {
    ELEMENTS_LEFT.with(|c| match c.get() {
        None => Ok(()),
        Some(left) if n <= left => {
            c.set(Some(left - n));
            Ok(())
        }
        Some(_) => Err(limit_error(LimitKind::ElementBudget, n, ELEMENT_BUDGET)),
    })
}

/// Held while a self-containing type is being read; see [`enter_nesting`].
#[must_use]
pub struct NestingGuard(());
impl Drop for NestingGuard {
    fn drop(&mut self) {
        NESTING.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

/// Call at the start of reading a type that can contain itself, and hold the
/// guard while reading it. Errors once more than [`MAX_NESTING_DEPTH`] are
/// open on this thread.
pub fn enter_nesting() -> Result<NestingGuard, BufReadError> {
    NESTING.with(|c| {
        let depth = c.get() + 1;
        if depth > MAX_NESTING_DEPTH {
            return Err(limit_error(
                LimitKind::NestingDepth,
                depth as u64,
                MAX_NESTING_DEPTH as u64,
            ));
        }
        c.set(depth);
        Ok(NestingGuard(()))
    })
}

fn remaining(buf: &Cursor<&[u8]>) -> usize {
    buf.get_ref().len().saturating_sub(buf.position() as usize)
}

/// Read a collection's VarInt length prefix.
///
/// The length must not be negative, and for element types with a non-zero
/// minimum encoded size it must fit in the bytes left (a length the data
/// can't hold is an error at once, not after reading up to it).
pub fn read_collection_len(
    buf: &mut Cursor<&[u8]>,
    min_element_size: usize,
) -> Result<usize, BufReadError> {
    let length = i32::azalea_read_var(buf)?;
    if length < 0 {
        return Err(limit_error(LimitKind::CollectionLength, length as u64, 0));
    }
    let length = length as usize;
    check_collection_len(buf, length, min_element_size)?;
    Ok(length)
}

/// Check a collection length (already read) against the bytes left, as
/// [`read_collection_len`] does.
pub fn check_collection_len(
    buf: &Cursor<&[u8]>,
    length: usize,
    min_element_size: usize,
) -> Result<(), BufReadError> {
    if min_element_size > 0 {
        let max = remaining(buf) / min_element_size;
        if length > max {
            return Err(limit_error(
                LimitKind::CollectionLength,
                length as u64,
                max as u64,
            ));
        }
    }
    Ok(())
}

/// How many elements of `T` to preallocate for a collection of `length`
/// elements: at most [`MAX_INITIAL_COLLECTION_SIZE`], at most what the bytes
/// left could hold, and at most [`MAX_INITIAL_COLLECTION_BYTES`].
pub fn initial_capacity<T>(buf: &Cursor<&[u8]>, length: usize, min_element_size: usize) -> usize {
    let by_bytes_left = remaining(buf) / min_element_size.max(1);
    let by_memory = MAX_INITIAL_COLLECTION_BYTES / mem::size_of::<T>().max(1);
    length
        .min(MAX_INITIAL_COLLECTION_SIZE)
        .min(by_bytes_left)
        .min(by_memory)
}

/// Charge one element of a collection whose element type may encode to zero
/// bytes against the packet's element budget.
#[inline]
pub fn charge_element_if_zero_size(min_element_size: usize) -> Result<(), BufReadError> {
    if min_element_size == 0 {
        charge_elements(1)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::{AzBuf, AzBufLimited, AzBufVar};

    fn varint(n: i32) -> Vec<u8> {
        let mut v = Vec::new();
        n.azalea_write_var(&mut v).unwrap();
        v
    }

    fn kind(e: BufReadError) -> LimitKind {
        match e {
            BufReadError::Limit { kind, .. } => kind,
            e => panic!("not a limit error: {e:?}"),
        }
    }

    fn string_bytes(s: &str) -> Vec<u8> {
        let mut v = varint(s.len() as i32);
        v.extend_from_slice(s.as_bytes());
        v
    }

    #[test]
    fn string_utf16_units_at_the_limit() {
        // 32767 UTF-16 units is ok, 32768 isn't
        let ok = "a".repeat(32767);
        let b = string_bytes(&ok);
        assert_eq!(String::azalea_read(&mut Cursor::new(&b[..])).unwrap(), ok);
        let b = string_bytes(&"a".repeat(32768));
        let e = String::azalea_read(&mut Cursor::new(&b[..])).unwrap_err();
        assert_eq!(kind(e), LimitKind::StringLength);
        assert_eq!(take_limit_hit(), Some(LimitKind::StringLength));
        assert_eq!(take_limit_hit(), None);
    }

    #[test]
    fn string_bytes_at_the_limit() {
        // 3 bytes per unit at most: 10923 three-byte chars are 32769 bytes
        // (over 3 * 10922 = 32766 for limit 10922, ok for 10923)
        let s = "\u{20ac}".repeat(10923);
        let b = string_bytes(&s);
        assert_eq!(
            String::azalea_read_limited(&mut Cursor::new(&b[..]), 10923).unwrap(),
            s
        );
        let e = String::azalea_read_limited(&mut Cursor::new(&b[..]), 10922).unwrap_err();
        assert_eq!(kind(e), LimitKind::StringLength);
        // a 4-byte char is 2 UTF-16 units
        let s = "\u{1f600}".repeat(16383);
        let b = string_bytes(&s);
        assert!(String::azalea_read(&mut Cursor::new(&b[..])).is_ok());
        let s = "\u{1f600}".repeat(16384);
        let b = string_bytes(&s);
        let e = String::azalea_read(&mut Cursor::new(&b[..])).unwrap_err();
        assert_eq!(kind(e), LimitKind::StringLength);
        // a byte length prefix over 3 * 32767 is an error before reading
        let b = varint(3 * 32767 + 1);
        let e = String::azalea_read(&mut Cursor::new(&b[..])).unwrap_err();
        assert_eq!(kind(e), LimitKind::StringLength);
        let b = varint(-1);
        let e = String::azalea_read(&mut Cursor::new(&b[..])).unwrap_err();
        assert_eq!(kind(e), LimitKind::StringLength);
    }

    #[test]
    fn collection_length_against_bytes_left() {
        // Vec<i32>: 4 bytes per element at least
        let mut b = varint(3);
        b.extend_from_slice(&[0; 12]);
        assert_eq!(
            Vec::<i32>::azalea_read(&mut Cursor::new(&b[..])).unwrap(),
            vec![0, 0, 0]
        );
        let mut b = varint(4);
        b.extend_from_slice(&[0; 12]);
        let e = Vec::<i32>::azalea_read(&mut Cursor::new(&b[..])).unwrap_err();
        assert_eq!(kind(e), LimitKind::CollectionLength);
        // the same for maps (i32 keys, u8 values: 5 bytes)
        let mut b = varint(2);
        b.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 2, 0]);
        assert_eq!(
            std::collections::HashMap::<i32, u8>::azalea_read(&mut Cursor::new(&b[..]))
                .unwrap()
                .len(),
            2
        );
        let mut b = varint(3);
        b.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 2, 0]);
        let e = std::collections::HashMap::<i32, u8>::azalea_read(&mut Cursor::new(&b[..]))
            .unwrap_err();
        assert_eq!(kind(e), LimitKind::CollectionLength);
        // bytes
        let mut b = varint(5);
        b.extend_from_slice(&[1; 5]);
        assert_eq!(
            Vec::<u8>::azalea_read(&mut Cursor::new(&b[..]))
                .unwrap()
                .len(),
            5
        );
        let mut b = varint(6);
        b.extend_from_slice(&[1; 5]);
        let e = Vec::<u8>::azalea_read(&mut Cursor::new(&b[..])).unwrap_err();
        assert_eq!(kind(e), LimitKind::CollectionLength);
        // a negative length
        let b = varint(-1);
        let e = Vec::<i32>::azalea_read(&mut Cursor::new(&b[..])).unwrap_err();
        assert_eq!(kind(e), LimitKind::CollectionLength);
        let e = Vec::<u8>::azalea_read(&mut Cursor::new(&b[..])).unwrap_err();
        assert_eq!(kind(e), LimitKind::CollectionLength);
        let e = Vec::<i32>::azalea_read_var(&mut Cursor::new(&b[..])).unwrap_err();
        assert_eq!(kind(e), LimitKind::CollectionLength);
    }

    #[test]
    fn limited_collection_at_the_limit() {
        let mut b = varint(16);
        b.extend_from_slice(&[7; 16]);
        assert_eq!(
            Vec::<u8>::azalea_read_limited(&mut Cursor::new(&b[..]), 16)
                .unwrap()
                .len(),
            16
        );
        let mut b = varint(17);
        b.extend_from_slice(&[7; 17]);
        let e = Vec::<u8>::azalea_read_limited(&mut Cursor::new(&b[..]), 16).unwrap_err();
        assert_eq!(kind(e), LimitKind::CollectionLength);
    }

    #[test]
    fn initial_allocation_is_capped() {
        // a length the data could hold, but not preallocated in full
        let buf = vec![0u8; 200_000];
        let cursor = Cursor::new(&buf[..]);
        assert_eq!(initial_capacity::<u8>(&cursor, 100_000, 1), 65536);
        assert_eq!(initial_capacity::<u8>(&cursor, 65536, 1), 65536);
        assert_eq!(initial_capacity::<u8>(&cursor, 65535, 1), 65535);
        // never more than the bytes left could hold
        assert_eq!(initial_capacity::<u8>(&cursor, 100_000, 4), 50_000);
        // nor more than 1 MiB
        assert_eq!(initial_capacity::<[u8; 64]>(&cursor, 100_000, 1), 16384);
        // zero-size types count as one byte each
        assert_eq!(
            initial_capacity::<()>(&Cursor::new(&[0u8; 10][..]), 100, 0),
            10
        );
    }

    #[test]
    fn zero_size_elements_use_the_element_budget() {
        // Vec<()> reads no bytes per element, so its length can't be checked
        // against the bytes left; the packet's element budget stops the loop
        let b = varint(ELEMENT_BUDGET as i32);
        let v = with_element_budget(|| Vec::<()>::azalea_read(&mut Cursor::new(&b[..])));
        assert_eq!(v.unwrap().len(), ELEMENT_BUDGET as usize);
        let b = varint(ELEMENT_BUDGET as i32 + 1);
        let e =
            with_element_budget(|| Vec::<()>::azalea_read(&mut Cursor::new(&b[..]))).unwrap_err();
        assert_eq!(kind(e), LimitKind::ElementBudget);
        // the budget is per packet: two halves fit, a third doesn't
        let half = varint((ELEMENT_BUDGET / 2) as i32);
        let r = with_element_budget(|| {
            Vec::<()>::azalea_read(&mut Cursor::new(&half[..]))?;
            Vec::<()>::azalea_read(&mut Cursor::new(&half[..]))?;
            Vec::<()>::azalea_read(&mut Cursor::new(&varint(1)[..]))
        });
        assert_eq!(kind(r.unwrap_err()), LimitKind::ElementBudget);
    }

    #[test]
    fn nesting_depth_at_the_limit() {
        fn nest(levels: u32) -> Result<(), BufReadError> {
            let _g = enter_nesting()?;
            if levels > 1 { nest(levels - 1) } else { Ok(()) }
        }
        assert!(nest(MAX_NESTING_DEPTH).is_ok());
        let e = nest(MAX_NESTING_DEPTH + 1).unwrap_err();
        assert_eq!(kind(e), LimitKind::NestingDepth);
        // the guards were all released
        assert!(nest(MAX_NESTING_DEPTH).is_ok());
    }

    #[test]
    fn min_encoded_sizes() {
        #[derive(crate::AzBuf)]
        struct S {
            _a: u8,
            _b: [i32; 2],
            #[var]
            _c: i32,
            _d: Vec<u64>,
            _e: Option<u8>,
        }
        #[derive(crate::AzBuf)]
        #[allow(dead_code)]
        enum E {
            A(u64),
            B,
        }
        assert_eq!(S::MIN_ENCODED_SIZE, 1 + 8 + 1 + 1 + 1);
        assert_eq!(<(u16, Option<u8>)>::MIN_ENCODED_SIZE, 3);
        assert_eq!(E::MIN_ENCODED_SIZE, 1);
        assert_eq!(<()>::MIN_ENCODED_SIZE, 0);
        assert_eq!(uuid::Uuid::MIN_ENCODED_SIZE, 16);
    }
}
