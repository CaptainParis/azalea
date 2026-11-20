#![allow(clippy::redundant_field_names, reason = "Triggered by `Error` derive")]

mod extra;
mod primitives;

use std::{
    backtrace::Backtrace,
    io::{self, Cursor, Write},
};

use thiserror::Error;

/// A trait that's implemented on types that are used by the Minecraft protocol.
pub trait AzBuf
where
    Self: Sized,
{
    /// A lower bound on how many bytes [`Self::azalea_read`] consumes.
    ///
    /// Collections use it to reject a length prefix the bytes left can't
    /// hold. It must never be larger than the real minimum; 0 (the default)
    /// is always correct and makes collections of the type count against the
    /// packet's element budget instead
    /// ([`crate::limits::ELEMENT_BUDGET`]).
    const MIN_ENCODED_SIZE: usize = 0;

    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError>;
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()>;
}

/// Used for types that have an alternative variable-length encoding.
///
/// This mostly exists for varints.
pub trait AzBufVar
where
    Self: Sized,
{
    fn azalea_read_var(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError>;
    fn azalea_write_var(&self, buf: &mut impl Write) -> io::Result<()>;
}

/// Used for types that have some configurable limit.
///
/// For example, the implementation of this on `String` limits the maximum
/// length of the string.
///
/// This exists partially as an anti-abuse mechanism in Minecraft, so there is
/// no limited write function.
pub trait AzBufLimited
where
    Self: Sized,
{
    fn azalea_read_limited(buf: &mut Cursor<&[u8]>, limit: u32) -> Result<Self, BufReadError>;
}

#[derive(Debug, Error)]
pub enum BufReadError {
    #[error("Invalid VarInt")]
    InvalidVarInt,
    #[error("Invalid VarLong")]
    InvalidVarLong,
    #[error("Error reading bytes")]
    CouldNotReadBytes,
    /// A size or depth limit from [`crate::limits`] was exceeded. Make these
    /// with [`crate::limits::limit_error`].
    #[error("{kind} limit exceeded ({value} > {max})")]
    Limit {
        kind: crate::limits::LimitKind,
        value: u64,
        max: u64,
    },
    #[error("{source}")]
    Io {
        #[from]
        #[backtrace]
        source: io::Error,
    },
    #[error("Invalid UTF-8: {bytes:?} (lossy: {lossy:?})")]
    InvalidUtf8 {
        bytes: Vec<u8>,
        lossy: String,
        // backtrace: Backtrace,
    },
    #[error("Unexpected enum variant {id}")]
    UnexpectedEnumVariant { id: i32 },
    #[error("Unexpected enum variant {id}")]
    UnexpectedStringEnumVariant { id: String },
    #[error("Tried to read {attempted_read} bytes but there were only {actual_read}")]
    UnexpectedEof {
        attempted_read: usize,
        actual_read: usize,
        backtrace: Backtrace,
    },
    #[error("{0}")]
    Custom(String),
    #[cfg(feature = "serde_json")]
    #[error("{source}")]
    Deserialization {
        #[from]
        #[backtrace]
        source: serde_json::Error,
    },
    #[error("{source}")]
    Nbt {
        #[from]
        #[backtrace]
        source: simdnbt::Error,
    },
    #[error("{source}")]
    DeserializeNbt {
        #[from]
        #[backtrace]
        source: simdnbt::DeserializeError,
    },
}

pub(crate) fn read_bytes<'a>(
    buf: &'a mut Cursor<&[u8]>,
    length: usize,
) -> Result<&'a [u8], BufReadError> {
    let data = *buf.get_ref();
    let initial_position = (buf.position() as usize).min(data.len());
    // network-safe: initial_position is at most data.len() (min above)
    let Some(bytes) = data[initial_position..].get(..length) else {
        return Err(BufReadError::UnexpectedEof {
            attempted_read: length,
            actual_read: data.len() - initial_position,
            backtrace: Backtrace::capture(),
        });
    };
    buf.set_position((initial_position + length) as u64);
    Ok(bytes)
}

pub(crate) fn read_utf_with_len<'a>(
    buf: &'a mut Cursor<&[u8]>,
    max_length: u32,
) -> Result<&'a str, BufReadError> {
    use crate::limits::{LimitKind, limit_error};

    // Utf8String.read: at most 3 bytes per UTF-16 unit
    // (ByteBufUtil.utf8MaxBytes), then at most max_length UTF-16 units
    let max_bytes = max_length as u64 * 3;
    let length = i32::azalea_read_var(buf)?;
    if length < 0 {
        return Err(limit_error(LimitKind::StringLength, length as u64, 0));
    }
    if length as u64 > max_bytes {
        return Err(limit_error(
            LimitKind::StringLength,
            length as u64,
            max_bytes,
        ));
    }

    let buffer = read_bytes(buf, length as usize)?;
    let string = std::str::from_utf8(buffer).map_err(|_| BufReadError::InvalidUtf8 {
        bytes: buffer.to_vec(),
        lossy: String::from_utf8_lossy(buffer).to_string(),
        // backtrace: Backtrace::capture(),
    })?;
    // only count UTF-16 units when the byte length allows more than the
    // maximum (each unit is at least one byte)
    if string.len() > max_length as usize {
        let units = string.encode_utf16().count();
        if units > max_length as usize {
            return Err(limit_error(
                LimitKind::StringLength,
                units as u64,
                max_length as u64,
            ));
        }
    }

    Ok(string)
}

pub(crate) fn write_utf_with_len(
    buf: &mut impl Write,
    string: &str,
    max_len: u32,
) -> io::Result<()> {
    // Utf8String.write: the limit is in UTF-16 units (each is at least one
    // byte, so only count them when the byte length is over it). An error,
    // not a panic: strings we send can come from the server (item
    // components in container clicks, for example)
    if string.len() > max_len as usize {
        let units = string.encode_utf16().count();
        if units > max_len as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("String too big (was {units} characters, max {max_len})"),
            ));
        }
    }
    (string.len() as u32).azalea_write_var(buf)?;
    buf.write_all(string.as_bytes())
}
