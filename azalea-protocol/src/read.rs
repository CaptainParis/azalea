//! Read packets from a stream.
#![allow(clippy::redundant_field_names, reason = "Triggered by `Error` derive")]

use std::{
    backtrace::Backtrace,
    env,
    fmt::Debug,
    io,
    io::{Cursor, Read},
    sync::LazyLock,
};

use azalea_buf::{AzBufVar, BufReadError};
use azalea_core::decode_errors::{DecodeErrorKind, record_decode_error};
use azalea_crypto::Aes128CfbDec;
use flate2::read::ZlibDecoder;
use futures::StreamExt;
use futures_lite::future;
use thiserror::Error;
use tokio::io::AsyncRead;
use tokio_util::{
    bytes::Buf,
    codec::{BytesCodec, FramedRead},
};
use tracing::trace;

use crate::packets::ProtocolPacket;

#[derive(Debug, Error)]
pub enum ReadPacketError {
    #[error("Error reading packet {packet_name} (id {packet_id}): {source}")]
    Parse {
        packet_id: u32,
        packet_name: String,
        backtrace: Box<Backtrace>,
        source: BufReadError,
    },
    #[error("Unknown packet id {id} in state {state_name}")]
    UnknownPacketId { state_name: String, id: u32 },
    #[error("Couldn't read packet id")]
    ReadPacketId { source: BufReadError },
    #[error(transparent)]
    Decompress {
        #[from]
        #[backtrace]
        source: DecompressionError,
    },
    #[error(transparent)]
    FrameSplitter {
        #[from]
        #[backtrace]
        source: FrameSplitterError,
    },
    #[error("Leftover data after reading packet {packet_name}: {data:?}")]
    LeftoverData { data: Vec<u8>, packet_name: String },
    #[error(transparent)]
    IoError {
        #[from]
        #[backtrace]
        source: io::Error,
    },
    #[error("Connection closed")]
    ConnectionClosed,
}

#[derive(Debug, Error)]
pub enum FrameSplitterError {
    #[error("Couldn't read VarInt length for packet. The previous packet may have been corrupted")]
    LengthRead {
        #[from]
        source: BufReadError,
    },
    #[error("Io error")]
    Io {
        #[from]
        #[backtrace]
        source: io::Error,
    },
    /// The frame length prefix is wider than 3 bytes, so the frame would be
    /// longer than `max` (vanilla: "length wider than 21-bit").
    #[error("Frame length prefix wider than 21 bits (max {max} bytes, read {size} so far)")]
    BadLength { max: usize, size: usize },
    #[error("Connection reset by peer")]
    ConnectionReset,
    #[error("Connection closed")]
    ConnectionClosed,
}

/// The longest frame vanilla can receive: its length prefix is at most 3
/// VarInt bytes (`Varint21FrameDecoder`).
pub const MAX_FRAME_LENGTH: usize = (1 << 21) - 1;

/// Read a frame length prefix from the start of `data`, as
/// `Varint21FrameDecoder.copyVarint`: `Ok(None)` if more bytes are needed,
/// an error at once if the prefix is wider than 3 bytes. Returns the length
/// and the size of the prefix.
fn read_frame_length(data: &[u8]) -> Result<Option<(usize, usize)>, FrameSplitterError> {
    let mut length = 0usize;
    for (i, &byte) in data.iter().take(3).enumerate() {
        length |= ((byte & 0x7f) as usize) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(Some((length, i + 1)));
        }
    }
    if data.len() >= 3 {
        return Err(FrameSplitterError::BadLength {
            max: MAX_FRAME_LENGTH,
            size: length,
        });
    }
    Ok(None)
}

/// Read a length, then read that amount of bytes from the buffer. If
/// there's not enough data yet, `None` is returned; a length prefix wider
/// than 3 bytes is an error before anything more is buffered, so a frame is
/// never longer than [`MAX_FRAME_LENGTH`].
fn parse_frame(buffer: &mut Cursor<Vec<u8>>) -> Result<Option<Box<[u8]>>, FrameSplitterError> {
    let start = (buffer.position() as usize).min(buffer.get_ref().len());
    // network-safe: start is at most the length (min above)
    let available = &buffer.get_ref()[start..];
    let Some((length, prefix_length)) = read_frame_length(available)? else {
        return Ok(None);
    };
    let Some(data) = available.get(prefix_length..prefix_length + length) else {
        // the rest of the frame hasn't arrived yet
        return Ok(None);
    };
    let data: Box<[u8]> = data.into();
    buffer.advance(prefix_length + length);

    if buffer.position() == buffer.get_ref().len() as u64 {
        // reset the inner vec once we've reached the end of the buffer so we
        // don't keep leaking memory
        buffer.get_mut().clear();

        // we just cap the capacity to 64KB instead of resetting it to save some
        // allocations.
        // and the reason we bother capping it at all is to avoid wasting memory
        // if we get a big packet once and then never again.
        buffer.get_mut().shrink_to(1024 * 64);

        buffer.set_position(0);
    }

    Ok(Some(data))
}

fn frame_splitter(buffer: &mut Cursor<Vec<u8>>) -> Result<Option<Box<[u8]>>, FrameSplitterError> {
    // https://tokio.rs/tokio/tutorial/framing
    parse_frame(buffer)
}

pub fn deserialize_packet<P: ProtocolPacket + Debug>(
    stream: &mut Cursor<&[u8]>,
) -> Result<P, Box<ReadPacketError>> {
    azalea_buf::limits::take_limit_hit();
    let res = azalea_buf::limits::with_element_budget(|| {
        // Packet ID
        u32::azalea_read_var(stream)
            .map_err(|e| Box::new(ReadPacketError::ReadPacketId { source: e }))
            .and_then(|packet_id| P::read(packet_id, stream))
    });
    // a limit that tripped is counted as its own kind, even if a reader on
    // the way up replaced the error (or, if it recovered, the packet still
    // lost data)
    let limit = azalea_buf::limits::take_limit_hit();
    if let Err(err) = &res {
        // decode error hooks: unknown packet id, and packet id/body parse
        // failures
        record_decode_error(match (limit, &**err) {
            (Some(limit), _) => limit.into(),
            (None, ReadPacketError::UnknownPacketId { .. }) => DecodeErrorKind::UnknownPacketId,
            (None, _) => DecodeErrorKind::PacketBody,
        });
    } else if let Some(limit) = limit {
        record_decode_error(limit.into());
    }
    res
}

// this is always true in multiplayer, false in singleplayer
static VALIDATE_DECOMPRESSED: bool = true;

pub static MAXIMUM_UNCOMPRESSED_LENGTH: u32 = 8_388_608;

#[derive(Debug, Error)]
pub enum DecompressionError {
    #[error("Couldn't read VarInt length for data")]
    LengthReadError {
        #[from]
        source: BufReadError,
    },
    #[error("Io error")]
    Io {
        #[from]
        #[backtrace]
        source: io::Error,
    },
    #[error("Badly compressed packet - size of {size} is below server threshold of {threshold}")]
    BelowCompressionThreshold { size: u32, threshold: u32 },
    #[error(
        "Badly compressed packet - size of {size} is larger than protocol maximum of {maximum}"
    )]
    AboveCompressionThreshold { size: u32, maximum: u32 },
    #[error(
        "Badly compressed packet - actual length of uncompressed payload {actual} does not match declared size {declared}"
    )]
    SizeMismatch { actual: usize, declared: u32 },
}

impl DecompressionError {
    /// The decode error counter kind for this error.
    pub fn kind(&self) -> DecodeErrorKind {
        match self {
            DecompressionError::AboveCompressionThreshold { .. }
            | DecompressionError::SizeMismatch { .. } => DecodeErrorKind::DecompressedSize,
            _ => DecodeErrorKind::Frame,
        }
    }
}

/// Get the decompressed bytes from a packet.
///
/// The stream must have already been decrypted before passing it to this
/// function.
///
/// As vanilla's `CompressionDecoder`: the declared size must be at least the
/// threshold and at most [`MAXIMUM_UNCOMPRESSED_LENGTH`], and the inflated
/// data must be exactly the declared size. Inflating stops one byte past the
/// declared size, so a zip bomb never produces more than that.
pub fn compression_decoder(
    stream: &mut Cursor<&[u8]>,
    compression_threshold: u32,
) -> Result<Box<[u8]>, DecompressionError> {
    // Data Length
    let n = u32::azalea_read_var(stream)?;
    if n == 0 {
        // no data size, no compression
        let start = (stream.position() as usize).min(stream.get_ref().len());
        // network-safe: start is at most the length (min above)
        let buf = stream.get_ref()[start..].to_vec().into_boxed_slice();
        stream.set_position(stream.get_ref().len() as u64);
        return Ok(buf);
    }

    if VALIDATE_DECOMPRESSED {
        if n < compression_threshold {
            return Err(DecompressionError::BelowCompressionThreshold {
                size: n,
                threshold: compression_threshold,
            });
        }
        if n > MAXIMUM_UNCOMPRESSED_LENGTH {
            return Err(DecompressionError::AboveCompressionThreshold {
                size: n,
                maximum: MAXIMUM_UNCOMPRESSED_LENGTH,
            });
        }
    }

    // preallocate at most 1 MiB; a larger packet grows the buffer as it's
    // actually inflated
    let mut decoded_buf = Vec::with_capacity((n as usize).min(1 << 20));

    let decoder = ZlibDecoder::new(stream);
    decoder.take(n as u64 + 1).read_to_end(&mut decoded_buf)?;
    if decoded_buf.len() != n as usize {
        return Err(DecompressionError::SizeMismatch {
            actual: decoded_buf.len(),
            declared: n,
        });
    }

    Ok(decoded_buf.into_boxed_slice())
}

/// Read a single packet from a stream.
///
/// The buffer is required because servers may send multiple packets in the
/// same frame, so we need to store the packet data that's left to read.
///
/// The current protocol state must be passed as a generic.
///
/// For the non-waiting version, see [`try_read_packet`].
pub async fn read_packet<P: ProtocolPacket + Debug, R>(
    stream: &mut R,
    buffer: &mut Cursor<Vec<u8>>,
    compression_threshold: Option<u32>,
    cipher: &mut Option<Aes128CfbDec>,
) -> Result<P, Box<ReadPacketError>>
where
    R: AsyncRead + Unpin + Send + Sync,
{
    let raw_packet = read_raw_packet(stream, buffer, compression_threshold, cipher).await?;
    let packet = deserialize_packet(&mut Cursor::new(&raw_packet))?;
    Ok(packet)
}

/// Try to read a single packet from a stream. Returns None if we haven't
/// received a full packet yet.
pub fn try_read_packet<P: ProtocolPacket + Debug, R>(
    stream: &mut R,
    buffer: &mut Cursor<Vec<u8>>,
    compression_threshold: Option<u32>,
    cipher: &mut Option<Aes128CfbDec>,
) -> Result<Option<P>, Box<ReadPacketError>>
where
    R: AsyncRead + Unpin + Send + Sync,
{
    let Some(raw_packet) = try_read_raw_packet(stream, buffer, compression_threshold, cipher)?
    else {
        return Ok(None);
    };
    let packet = deserialize_packet(&mut Cursor::new(&raw_packet))?;
    Ok(Some(packet))
}

pub async fn read_raw_packet<R>(
    stream: &mut R,
    buffer: &mut Cursor<Vec<u8>>,
    compression_threshold: Option<u32>,
    // this has to be a &mut Option<T> instead of an Option<&mut T> because
    // otherwise the borrow checker complains about the cipher being moved
    cipher: &mut Option<Aes128CfbDec>,
) -> Result<Box<[u8]>, Box<ReadPacketError>>
where
    R: AsyncRead + Unpin + Send + Sync,
{
    loop {
        if let Some(buf) = read_raw_packet_from_buffer::<R>(buffer, compression_threshold)? {
            // we got a full packet!!
            return Ok(buf);
        };

        let bytes = read_and_decrypt_frame(stream, cipher).await?;
        buffer.get_mut().extend_from_slice(&bytes);
    }
}
/// Read a packet from the stream, then if necessary decrypt it, decompress
/// it, and split it.
pub fn try_read_raw_packet<R>(
    stream: &mut R,
    buffer: &mut Cursor<Vec<u8>>,
    compression_threshold: Option<u32>,
    cipher: &mut Option<Aes128CfbDec>,
) -> Result<Option<Box<[u8]>>, Box<ReadPacketError>>
where
    R: AsyncRead + Unpin + Send + Sync,
{
    loop {
        if let Some(buf) = read_raw_packet_from_buffer::<R>(buffer, compression_threshold)? {
            // we got a full packet!!
            return Ok(Some(buf));
        };
        let Some(bytes) = try_read_and_decrypt_frame(stream, cipher)? else {
            // no data received
            return Ok(None);
        };
        // we got some data, so add it to the buffer and try again
        buffer.get_mut().extend_from_slice(&bytes);
    }
}

async fn read_and_decrypt_frame<R>(
    stream: &mut R,
    cipher: &mut Option<Aes128CfbDec>,
) -> Result<Box<[u8]>, Box<ReadPacketError>>
where
    R: AsyncRead + Unpin + Send + Sync,
{
    let mut framed = FramedRead::new(stream, BytesCodec::new());

    let Some(message) = framed.next().await else {
        return Err(Box::new(ReadPacketError::ConnectionClosed));
    };
    let bytes = message.map_err(ReadPacketError::from)?;

    let mut bytes = bytes.to_vec().into_boxed_slice();

    // decrypt if necessary
    if let Some(cipher) = cipher {
        azalea_crypto::decrypt_packet(cipher, &mut bytes);
    }

    Ok(bytes)
}
fn try_read_and_decrypt_frame<R>(
    stream: &mut R,
    cipher: &mut Option<Aes128CfbDec>,
) -> Result<Option<Box<[u8]>>, Box<ReadPacketError>>
where
    R: AsyncRead + Unpin + Send + Sync,
{
    let mut framed = FramedRead::new(stream, BytesCodec::new());

    let Some(message) = future::block_on(future::poll_once(framed.next())) else {
        // nothing yet
        return Ok(None);
    };
    let Some(message) = message else {
        return Err(Box::new(ReadPacketError::ConnectionClosed));
    };
    let bytes = message.map_err(ReadPacketError::from)?;
    let mut bytes = bytes.to_vec().into_boxed_slice();

    // decrypt if necessary
    if let Some(cipher) = cipher {
        azalea_crypto::decrypt_packet(cipher, &mut bytes);
    }

    Ok(Some(bytes))
}

pub fn read_raw_packet_from_buffer<R>(
    buffer: &mut Cursor<Vec<u8>>,
    compression_threshold: Option<u32>,
) -> Result<Option<Box<[u8]>>, Box<ReadPacketError>>
where
    R: AsyncRead + Unpin + Send + Sync,
{
    // decode error hook: frame decode failures (bad length prefix or bad
    // compression)
    let Some(mut buf) = frame_splitter(buffer).map_err(|e| {
        record_decode_error(match e {
            FrameSplitterError::BadLength { .. } => DecodeErrorKind::FrameLength,
            _ => DecodeErrorKind::Frame,
        });
        ReadPacketError::from(e)
    })?
    else {
        // no full packet yet :(
        return Ok(None);
    };

    if let Some(compression_threshold) = compression_threshold {
        // network-safe: a full-range slice
        buf = compression_decoder(&mut Cursor::new(&buf[..]), compression_threshold).map_err(
            |e| {
                record_decode_error(e.kind());
                ReadPacketError::from(e)
            },
        )?;
    }

    if tracing::enabled!(tracing::Level::TRACE) {
        static DO_NOT_CUT_OFF_PACKET_LOGS: LazyLock<bool> = LazyLock::new(|| {
            env::var("AZALEA_DO_NOT_CUT_OFF_PACKET_LOGS")
                .map(|s| s == "1" || s == "true")
                .unwrap_or(false)
        });

        let buf_string: String = {
            if !*DO_NOT_CUT_OFF_PACKET_LOGS && buf.len() > 500 {
                // network-safe: only when buf.len() > 500
                let cut_off_buf = &buf[..500];
                format!("{cut_off_buf:?}...")
            } else {
                format!("{buf:?}")
            }
        };
        trace!("Reading packet with bytes: {buf_string}");
    };

    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use azalea_buf::AzBuf as _;

    use crate::{packets::game::ClientboundGamePacket, read::deserialize_packet};

    #[test]
    fn fuzzed_1() {
        // oom: checks for unbounded TagMap
        let _ = deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(
            [132, 1, 255, 255, 255, 255, 255].as_slice(),
        ));
    }
    #[test]
    fn fuzzed_2() {
        // oom: also checks for unbounded TagMap
        let _ = deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(
            [132, 1, 75, 0, 255, 255, 255, 255, 24, 0].as_slice(),
        ));
    }
    #[test]
    fn fuzzed_3() {
        // panic: integer overflow in HolderSet::azalea_read
        let _ = deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(
            [
                94, 44, 157, 38, 61, 37, 37, 37, 37, 37, 37, 65, 128, 128, 1, 1, 255, 252, 128,
                128, 128, 128, 128, 128, 128, 40, 0,
            ]
            .as_slice(),
        ));
    }
    #[test]
    fn fuzzed_4() {
        // memory leak in DataComponentPatch
        let _ = deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(
            [94, 94, 70, 52, 0, 6, 0].as_slice(),
        ));
    }
    #[test]
    fn fuzzed_5() {
        // also a memory leak in DataComponentPatch
        let _ = deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(
            [94, 94, 70, 52, 0, 6, 0, 6, 0].as_slice(),
        ));
    }
    #[test]
    fn fuzzed_6() {
        // memory leak in simdnbt
        let _ = simdnbt::owned::Nbt::azalea_read(&mut Cursor::new([10, 10, 0, 0, 0].as_slice()));
    }

    #[test]
    fn decode_errors_are_counted() {
        use crate::decode_errors::decode_errors;

        // other tests in this crate decode bad data too and the counters are
        // global, so only check that ours went up
        let before = decode_errors();
        // 200 isn't a clientbound game packet id in 1.21.4 (there are 131)
        let res =
            deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new([0xc8, 0x01].as_slice()));
        assert!(res.is_err());
        let after = decode_errors();
        assert!(after.unknown_packet_id > before.unknown_packet_id);

        // keep alive (0x27) with a truncated u64 id
        let res = deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(
            [0x27, 1, 2, 3].as_slice(),
        ));
        assert!(res.is_err());
        assert!(decode_errors().packet_body > after.packet_body);

        // a compressed frame that claims an uncompressed size below the
        // compression threshold
        let before = decode_errors();
        let mut buffer = Cursor::new(vec![2, 1, 0]);
        let res = super::read_raw_packet_from_buffer::<tokio::io::Empty>(&mut buffer, Some(256));
        assert!(res.is_err());
        assert!(decode_errors().frame > before.frame);
    }

    fn varint(n: u32) -> Vec<u8> {
        use azalea_buf::AzBufVar;
        let mut v = Vec::new();
        n.azalea_write_var(&mut v).unwrap();
        v
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::bufread::ZlibEncoder::new(data, flate2::Compression::best())
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    #[test]
    fn frame_length_prefix_is_checked_before_buffering() {
        use super::{MAX_FRAME_LENGTH, parse_frame};

        // a 3-byte prefix with the continuation bit on its last byte is a
        // frame error at once, with nothing more buffered
        let before = decode_errors();
        let mut buffer = Cursor::new(vec![0x80, 0x80, 0x80]);
        let res = super::read_raw_packet_from_buffer::<tokio::io::Empty>(&mut buffer, None);
        assert!(res.is_err());
        assert!(decode_errors().frame_length > before.frame_length);
        // two prefix bytes so far: need more
        assert!(
            parse_frame(&mut Cursor::new(vec![0x80, 0x80]))
                .unwrap()
                .is_none()
        );
        // the longest frame (2^21 - 1 bytes, prefix ff ff 7f) is fine once
        // it's all there, and waits until then
        let mut data = vec![0xff, 0xff, 0x7f];
        data.extend(std::iter::repeat_n(7u8, MAX_FRAME_LENGTH - 1));
        assert!(
            parse_frame(&mut Cursor::new(data.clone()))
                .unwrap()
                .is_none()
        );
        data.push(7);
        let frame = parse_frame(&mut Cursor::new(data)).unwrap().unwrap();
        assert_eq!(frame.len(), MAX_FRAME_LENGTH);
    }

    use crate::decode_errors::decode_errors;

    #[test]
    fn decompressed_size_at_the_limit() {
        use super::{DecompressionError, MAXIMUM_UNCOMPRESSED_LENGTH, compression_decoder};
        let max = MAXIMUM_UNCOMPRESSED_LENGTH as usize;

        // exactly the maximum
        let mut frame = varint(max as u32);
        frame.extend(zlib(&vec![0; max]));
        let out = compression_decoder(&mut Cursor::new(&frame[..]), 256).unwrap();
        assert_eq!(out.len(), max);

        // declaring one byte more is an error before inflating
        let mut frame = varint(max as u32 + 1);
        frame.extend(zlib(&vec![0; max + 1]));
        let e = compression_decoder(&mut Cursor::new(&frame[..]), 256).unwrap_err();
        assert!(matches!(
            e,
            DecompressionError::AboveCompressionThreshold { .. }
        ));

        // less data than declared
        let mut frame = varint(1000);
        frame.extend(zlib(&[1; 999]));
        let e = compression_decoder(&mut Cursor::new(&frame[..]), 256).unwrap_err();
        assert!(matches!(
            e,
            DecompressionError::SizeMismatch {
                actual: 999,
                declared: 1000
            }
        ));
    }

    #[test]
    fn zip_bomb_is_inflated_only_to_the_declared_size_plus_one() {
        use super::{DecompressionError, compression_decoder};

        // 64 MiB of zeros in about 64 KiB of zlib, declared as 300 bytes
        let bomb = zlib(&vec![0; 64 << 20]);
        assert!(bomb.len() < 128 << 10);
        let mut frame = varint(300);
        frame.extend(&bomb);
        let start = std::time::Instant::now();
        let e = compression_decoder(&mut Cursor::new(&frame[..]), 256).unwrap_err();
        // inflating stopped one byte past the declared size
        assert!(
            matches!(
                e,
                DecompressionError::SizeMismatch {
                    actual: 301,
                    declared: 300
                }
            ),
            "{e:?}"
        );
        assert!(start.elapsed() < std::time::Duration::from_millis(100));

        // and it's counted as a decompressed size error
        let before = decode_errors();
        let mut framed = varint(frame.len() as u32);
        framed.extend(&frame);
        let res = super::read_raw_packet_from_buffer::<tokio::io::Empty>(
            &mut Cursor::new(framed),
            Some(256),
        );
        assert!(res.is_err());
        assert!(decode_errors().decompressed_size > before.decompressed_size);
    }
}
