//! Limits for NBT read from the network, as vanilla's `NbtAccounter`.
//!
//! Vanilla 1.21.4 reads every network NBT tag (`FriendlyByteBuf.readNbt`,
//! `ByteBufCodecs.TAG`/`COMPOUND_TAG`, and the codec-based ones such as chat
//! components) with `NbtAccounter.create(2097152)`: lists and compounds may
//! nest 512 deep, and the tag's estimated memory use (fixed costs per tag
//! type, see [`check_network_nbt`]) may be at most 2 097 152 bytes.
//!
//! [`check_network_nbt`] walks the encoded tag without recursion and without
//! allocating more than the depth of the tag, and returns an error if it
//! breaks one of those rules or isn't well-formed. NBT is only handed to
//! simdnbt after that, so simdnbt never sees a length the data can't hold.

use std::{backtrace::Backtrace, collections::HashSet, io::Cursor};

use crate::{
    BufReadError,
    limits::{LimitKind, NBT_MAX_DEPTH, NBT_QUOTA, limit_error},
};

const END: u8 = 0;
const BYTE: u8 = 1;
const SHORT: u8 = 2;
const INT: u8 = 3;
const LONG: u8 = 4;
const FLOAT: u8 = 5;
const DOUBLE: u8 = 6;
const BYTE_ARRAY: u8 = 7;
const STRING: u8 = 8;
const LIST: u8 = 9;
const COMPOUND: u8 = 10;
const INT_ARRAY: u8 = 11;
const LONG_ARRAY: u8 = 12;

enum Frame<'a> {
    Compound { keys: HashSet<&'a [u8]> },
    List { element_type: u8, remaining: u32 },
}

struct Checker<'a> {
    data: &'a [u8],
    pos: usize,
    usage: u64,
    stack: Vec<Frame<'a>>,
}

impl<'a> Checker<'a> {
    fn eof(&self, wanted: usize) -> BufReadError {
        BufReadError::UnexpectedEof {
            attempted_read: wanted,
            actual_read: self.data.len() - self.pos,
            backtrace: Backtrace::capture(),
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], BufReadError> {
        let Some(bytes) = self.data[self.pos..].get(..n) else {
            return Err(self.eof(n));
        };
        self.pos += n;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, BufReadError> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, BufReadError> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// `NbtAccounter.accountBytes`
    fn account(&mut self, bytes: u64) -> Result<(), BufReadError> {
        let usage = self.usage.saturating_add(bytes);
        if usage > NBT_QUOTA {
            return Err(limit_error(LimitKind::NbtSize, usage, NBT_QUOTA));
        }
        self.usage = usage;
        Ok(())
    }

    /// `NbtAccounter.pushDepth`
    fn push(&mut self, frame: Frame<'a>) -> Result<(), BufReadError> {
        if self.stack.len() as u32 >= NBT_MAX_DEPTH {
            return Err(limit_error(
                LimitKind::NbtDepth,
                self.stack.len() as u64 + 1,
                NBT_MAX_DEPTH as u64,
            ));
        }
        self.stack.push(frame);
        Ok(())
    }

    /// A modified UTF-8 string (`DataInput.readUTF`); returns its bytes and
    /// its length in UTF-16 units (every byte that isn't a continuation byte
    /// starts one unit, as modified UTF-8 writes supplementary characters as
    /// two 3-byte surrogates).
    fn string(&mut self) -> Result<(&'a [u8], u64), BufReadError> {
        let len = self.take(2)?;
        let len = u16::from_be_bytes([len[0], len[1]]) as usize;
        let bytes = self.take(len)?;
        let units = bytes.iter().filter(|&&b| b & 0xc0 != 0x80).count() as u64;
        Ok((bytes, units))
    }

    fn array(&mut self, element_size: u64) -> Result<(), BufReadError> {
        self.account(24)?;
        let n = self.i32()?;
        if n < 0 {
            return Err(BufReadError::Custom(format!(
                "negative NBT array length {n}"
            )));
        }
        self.account(element_size * n as u64)?;
        self.take(n as usize * element_size as usize)?;
        Ok(())
    }

    /// Read the payload of a tag of the given type; lists and compounds are
    /// pushed and their contents read by [`Self::run`].
    fn payload(&mut self, tag_type: u8) -> Result<(), BufReadError> {
        match tag_type {
            BYTE => {
                self.account(9)?;
                self.take(1)?;
            }
            SHORT => {
                self.account(10)?;
                self.take(2)?;
            }
            INT => {
                self.account(12)?;
                self.take(4)?;
            }
            LONG => {
                self.account(16)?;
                self.take(8)?;
            }
            FLOAT => {
                self.account(12)?;
                self.take(4)?;
            }
            DOUBLE => {
                self.account(16)?;
                self.take(8)?;
            }
            BYTE_ARRAY => self.array(1)?,
            INT_ARRAY => self.array(4)?,
            LONG_ARRAY => self.array(8)?,
            STRING => {
                self.account(36)?;
                let (_, units) = self.string()?;
                self.account(2 * units)?;
            }
            LIST => {
                self.push(Frame::List {
                    element_type: END,
                    remaining: 0,
                })?;
                self.account(37)?;
                let element_type = self.u8()?;
                let n = self.i32()?;
                if element_type == END && n > 0 {
                    return Err(BufReadError::Custom("Missing type on ListTag".to_owned()));
                }
                if n < 0 {
                    return Err(BufReadError::Custom(format!(
                        "negative NBT list length {n}"
                    )));
                }
                self.account(4 * n as u64)?;
                if n > 0 && !(BYTE..=LONG_ARRAY).contains(&element_type) {
                    return Err(BufReadError::Custom(format!(
                        "invalid NBT list element type {element_type}"
                    )));
                }
                if let Some(Frame::List {
                    element_type: t,
                    remaining,
                }) = self.stack.last_mut()
                {
                    *t = element_type;
                    *remaining = n as u32;
                }
            }
            COMPOUND => {
                self.push(Frame::Compound {
                    keys: HashSet::new(),
                })?;
                self.account(48)?;
            }
            _ => {
                return Err(BufReadError::Custom(format!(
                    "invalid NBT tag type {tag_type}"
                )));
            }
        }
        Ok(())
    }

    /// Read until every list and compound that was opened is closed.
    fn run(&mut self) -> Result<(), BufReadError> {
        loop {
            let next = match self.stack.last_mut() {
                None => return Ok(()),
                Some(Frame::List {
                    element_type,
                    remaining,
                }) => {
                    if *remaining == 0 {
                        None
                    } else {
                        *remaining -= 1;
                        Some(*element_type)
                    }
                }
                Some(Frame::Compound { .. }) => {
                    let tag_type = self.u8()?;
                    if tag_type == END {
                        None
                    } else {
                        self.account(28)?;
                        let (key, units) = self.string()?;
                        self.account(2 * units)?;
                        let Some(Frame::Compound { keys }) = self.stack.last_mut() else {
                            // network-safe: the top of the stack was a compound
                            // two lines above and nothing was pushed since
                            unreachable!()
                        };
                        // a key seen before replaces its value and isn't
                        // accounted again
                        if keys.insert(key) {
                            self.account(36)?;
                        }
                        Some(tag_type)
                    }
                }
            };
            match next {
                // the list or compound on top is done (`popDepth`)
                None => {
                    self.stack.pop();
                }
                Some(tag_type) => self.payload(tag_type)?,
            }
        }
    }
}

/// Check the network NBT tag at the cursor (a tag type byte, then the
/// unnamed payload; type 0 is an empty tag) against vanilla's network NBT
/// limits. Doesn't move the cursor.
///
/// Errors are [`BufReadError::Limit`] with [`LimitKind::NbtDepth`] or
/// [`LimitKind::NbtSize`] for the limits, and other errors for data that
/// isn't well-formed or ends early.
pub fn check_network_nbt(buf: &Cursor<&[u8]>) -> Result<(), BufReadError> {
    let data = *buf.get_ref();
    let pos = (buf.position() as usize).min(data.len());
    let mut checker = Checker {
        data,
        pos,
        usage: 0,
        stack: Vec::new(),
    };
    let tag_type = checker.u8()?;
    if tag_type == END {
        return Ok(());
    }
    checker.payload(tag_type)?;
    checker.run()
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    /// `depth` nested lists of lists (the innermost one empty).
    pub fn nested_lists(depth: usize) -> Vec<u8> {
        let mut v = vec![LIST];
        for _ in 0..depth - 1 {
            v.push(LIST);
            v.extend_from_slice(&1i32.to_be_bytes());
        }
        v.push(END);
        v.extend_from_slice(&0i32.to_be_bytes());
        v
    }

    /// `depth` nested compounds, each holding the next under key "a".
    pub fn nested_compounds(depth: usize) -> Vec<u8> {
        let mut v = vec![COMPOUND];
        for _ in 0..depth - 1 {
            v.extend_from_slice(&[COMPOUND, 0, 1, b'a']);
        }
        v.extend(std::iter::repeat_n(END, depth));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::{tests_support::*, *};

    fn kind(r: Result<(), BufReadError>) -> Option<LimitKind> {
        match r {
            Err(BufReadError::Limit { kind, .. }) => Some(kind),
            Err(e) => panic!("not a limit error: {e}"),
            Ok(()) => None,
        }
    }

    fn check(data: &[u8]) -> Result<(), BufReadError> {
        check_network_nbt(&Cursor::new(data))
    }

    #[test]
    fn depth_at_the_limit() {
        assert_eq!(kind(check(&nested_lists(512))), None);
        assert_eq!(kind(check(&nested_lists(513))), Some(LimitKind::NbtDepth));
        assert_eq!(kind(check(&nested_compounds(512))), None);
        assert_eq!(
            kind(check(&nested_compounds(513))),
            Some(LimitKind::NbtDepth)
        );
    }

    /// A root byte array whose accounted size is `24 + n`.
    fn byte_array(n: usize) -> Vec<u8> {
        let mut v = vec![BYTE_ARRAY];
        v.extend_from_slice(&(n as i32).to_be_bytes());
        v.extend(std::iter::repeat_n(0, n));
        v
    }

    #[test]
    fn size_at_the_limit() {
        let max = NBT_QUOTA as usize - 24;
        assert_eq!(kind(check(&byte_array(max))), None);
        assert_eq!(kind(check(&byte_array(max + 1))), Some(LimitKind::NbtSize));
        // a length far beyond the data is a size error before any read
        let mut v = vec![LONG_ARRAY];
        v.extend_from_slice(&i32::MAX.to_be_bytes());
        assert_eq!(kind(check(&v)), Some(LimitKind::NbtSize));
    }

    #[test]
    fn accounting_matches_vanilla() {
        // a compound {"ab": "é"}: 48 + key 28 + 2*2 + string 36 + 2*1 + entry
        // 36
        let mut v = vec![COMPOUND, STRING, 0, 2, b'a', b'b', 0, 2, 0xc3, 0xa9, END];
        let mut c = Checker {
            data: &v,
            pos: 0,
            usage: 0,
            stack: Vec::new(),
        };
        c.u8().unwrap();
        c.payload(COMPOUND).unwrap();
        c.run().unwrap();
        assert_eq!(c.usage, 48 + 28 + 4 + 36 + 2 + 36);
        // the same key twice is accounted once as an entry
        v.splice(10..10, [STRING, 0, 2, b'a', b'b', 0, 0]);
        let mut c = Checker {
            data: &v,
            pos: 0,
            usage: 0,
            stack: Vec::new(),
        };
        c.u8().unwrap();
        c.payload(COMPOUND).unwrap();
        c.run().unwrap();
        assert_eq!(c.usage, 48 + 28 + 4 + 36 + 2 + 36 + 28 + 4 + 36);
        // a list of 3 ints: 37 + 4*3 + 12*3
        let mut v = vec![LIST, INT];
        v.extend_from_slice(&3i32.to_be_bytes());
        v.extend_from_slice(&[0; 12]);
        let mut c = Checker {
            data: &v,
            pos: 0,
            usage: 0,
            stack: Vec::new(),
        };
        c.u8().unwrap();
        c.payload(LIST).unwrap();
        c.run().unwrap();
        assert_eq!(c.usage, 37 + 12 + 36);
    }

    #[test]
    fn malformed() {
        // truncated
        assert!(check(&[COMPOUND, BYTE, 0, 1]).is_err());
        assert!(check(&byte_array(10)[..8]).is_err());
        // list of end with elements
        let mut v = vec![LIST, END];
        v.extend_from_slice(&1i32.to_be_bytes());
        assert!(check(&v).is_err());
        // negative lengths
        let mut v = vec![LIST, INT];
        v.extend_from_slice(&(-1i32).to_be_bytes());
        assert!(check(&v).is_err());
        let mut v = vec![INT_ARRAY];
        v.extend_from_slice(&(-1i32).to_be_bytes());
        assert!(check(&v).is_err());
        // unknown tag type
        assert!(check(&[13]).is_err());
        // empty tag and empty compound are fine
        assert!(check(&[END]).is_ok());
        assert!(check(&[COMPOUND, END]).is_ok());
    }

    #[test]
    fn wide_input_is_linear() {
        // 2 MB of NBT at the quota: a list of 43 690 empty compounds
        let n = (NBT_QUOTA as usize - 37) / (4 + 48);
        let mut v = vec![LIST, COMPOUND];
        v.extend_from_slice(&(n as i32).to_be_bytes());
        v.extend(std::iter::repeat_n(END, n));
        let start = std::time::Instant::now();
        assert!(check(&v).is_ok());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}

#[cfg(test)]
mod reader_tests {
    use std::io::Cursor;

    use super::tests_support::*;
    use crate::{AzBuf, BufReadError, limits::LimitKind};

    #[test]
    fn owned_readers_take_the_deepest_allowed_nbt() {
        for data in [nested_lists(512), nested_compounds(512)] {
            let mut cursor = Cursor::new(&data[..]);
            simdnbt::owned::NbtTag::azalea_read(&mut cursor).unwrap();
            assert_eq!(cursor.position() as usize, data.len());
        }
        for data in [nested_lists(513), nested_compounds(513)] {
            let e = simdnbt::owned::NbtTag::azalea_read(&mut Cursor::new(&data[..])).unwrap_err();
            assert!(matches!(
                e,
                BufReadError::Limit {
                    kind: LimitKind::NbtDepth,
                    ..
                }
            ));
        }
        let data = nested_compounds(512);
        simdnbt::owned::NbtCompound::azalea_read(&mut Cursor::new(&data[..])).unwrap();
        simdnbt::owned::Nbt::azalea_read(&mut Cursor::new(&data[..])).unwrap();
    }

    #[test]
    fn borrow_reader_takes_the_deepest_allowed_nbt() {
        for data in [nested_lists(512), nested_compounds(512)] {
            let mut cursor = Cursor::new(&data[..]);
            super::check_network_nbt(&cursor).unwrap();
            simdnbt::borrow::read_optional_tag(&mut cursor).unwrap();
            assert_eq!(cursor.position() as usize, data.len());
        }
    }
}
