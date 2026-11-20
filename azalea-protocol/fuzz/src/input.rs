//! Input formats of the structured targets.

/// A packet list: each packet (id and body, as after decompression) is
/// prefixed with its length as a u16, little endian. A truncated last entry is
/// cut to what's there.
pub fn packet_list(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut rest = data;
    while rest.len() >= 2 {
        let len = u16::from_le_bytes([rest[0], rest[1]]) as usize;
        rest = &rest[2..];
        let len = len.min(rest.len());
        let (packet, tail) = rest.split_at(len);
        out.push(packet);
        rest = tail;
    }
    out
}

/// The inverse of [`packet_list`] (packets over 65 535 bytes are skipped).
pub fn write_packet_list<'a>(packets: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    for p in packets {
        let Ok(len) = u16::try_from(p.len()) else {
            continue;
        };
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(p);
    }
    out
}

/// In `config_to_play` inputs, this entry ends the configuration packets;
/// what follows are play packets.
pub const PLAY_MARKER: &[u8] = b"\xffPLAY";
