//! The body of every fuzz target.

use std::{fmt::Debug, io::Cursor};

use azalea_block::BlockState;
use azalea_buf::AzBuf;
use azalea_chat::FormattedText;
use azalea_client::{
    connection::RawConnection,
    test_utils::simulation::{
        Simulation, default_login_packet, make_basic_add_entity, make_basic_empty_chunk,
    },
};
use azalea_core::position::{ChunkPos, ChunkSectionBlockPos};
use azalea_protocol::{
    packets::{
        ConnectionProtocol, ProtocolPacket,
        config::{ClientboundConfigPacket, ClientboundFinishConfiguration},
        game::ClientboundGamePacket,
    },
    read::{
        MAX_FRAME_LENGTH, MAXIMUM_UNCOMPRESSED_LENGTH, deserialize_packet,
        read_raw_packet_from_buffer,
    },
    write::serialize_packet,
};
use azalea_registry::{builtin::EntityKind, data::Biome};
use azalea_world::{Chunk, Section, palette::PalettedContainer};

use crate::input::{PLAY_MARKER, packet_list};

/// Decode one packet of type `P` (the upstream per-state targets).
pub fn decode<P: ProtocolPacket + Debug>(data: &[u8]) {
    if let Ok(p) = deserialize_packet::<P>(&mut Cursor::new(data)) {
        // a bot logs packets
        let _ = format!("{p:?}");
    }
}

/// Decode, encode, decode the result, encode again; the two encodings must
/// be the same bytes (a byte comparison, so NaN and map order don't count).
pub fn roundtrip<P: ProtocolPacket + Debug>(data: &[u8]) {
    let Ok(first) = deserialize_packet::<P>(&mut Cursor::new(data)) else {
        return;
    };
    let bytes1 = serialize_packet(&first)
        .unwrap_or_else(|e| panic!("a decoded packet doesn't encode: {e}\n{first:?}"));
    let second = deserialize_packet::<P>(&mut Cursor::new(&bytes1))
        .unwrap_or_else(|e| panic!("our own encoding doesn't decode: {e}\n{first:?}\n{bytes1:?}"));
    let bytes2 = serialize_packet(&second)
        .unwrap_or_else(|e| panic!("a decoded packet doesn't encode: {e}\n{second:?}"));
    assert!(
        bytes1 == bytes2,
        "encode(decode(encode(p))) != encode(p)\n{first:?}\n{second:?}"
    );
}

/// The frame splitter, decompression and their limits: the stream after the
/// first two bytes is fed in chunks; byte 0 picks the compression threshold,
/// byte 1 the chunk size. Every frame is decoded as a clientbound game packet.
pub fn frame(data: &[u8]) {
    if data.len() < 2 {
        return;
    }
    let threshold = match data[0] % 3 {
        0 => None,
        1 => Some(256),
        _ => Some(0),
    };
    let step = (data[1] as usize % 64) + 1;
    let mut buffer = Cursor::new(Vec::new());
    for chunk in data[2..].chunks(step) {
        buffer.get_mut().extend_from_slice(chunk);
        loop {
            match read_raw_packet_from_buffer::<Cursor<Vec<u8>>>(&mut buffer, threshold) {
                Ok(Some(packet)) => {
                    assert!(packet.len() <= MAXIMUM_UNCOMPRESSED_LENGTH as usize);
                    decode::<ClientboundGamePacket>(&packet);
                }
                Ok(None) => break,
                // the stream is lost; a client closes it
                Err(_) => return,
            }
        }
        // nothing buffered beyond one frame that hasn't fully arrived
        let unread =
            buffer.get_ref().len() - (buffer.position() as usize).min(buffer.get_ref().len());
        assert!(
            unread <= MAX_FRAME_LENGTH + 3,
            "{unread} bytes buffered without a frame"
        );
    }
}

/// Network NBT: the limit checker, then simdnbt's owned and borrow readers.
/// A tag that's read must write back to bytes that read the same, and both
/// readers must read the same tag (and the same number of bytes) from it.
pub fn nbt_network(data: &[u8]) {
    let mut cursor = Cursor::new(data);
    let Ok(tag) = simdnbt::owned::NbtTag::azalea_read(&mut cursor) else {
        return;
    };
    let owned_end = cursor.position();
    let _ = format!("{tag:?}");
    let mut bytes1 = Vec::new();
    tag.azalea_write(&mut bytes1).unwrap();
    let again = simdnbt::owned::NbtTag::azalea_read(&mut Cursor::new(&bytes1[..]))
        .unwrap_or_else(|e| panic!("our own NBT doesn't read back: {e}\n{tag:?}"));
    let mut bytes2 = Vec::new();
    again.azalea_write(&mut bytes2).unwrap();
    assert!(bytes1 == bytes2, "NBT write isn't stable");

    // the borrow reader on the same input (the owned read checked it): it
    // must read it too, to the same tag (compared as written bytes, so NaN
    // floats compare equal) and end where the owned reader ended
    let mut cursor = Cursor::new(data);
    let borrowed = simdnbt::borrow::read_tag(&mut cursor).unwrap_or_else(|e| {
        panic!("the owned reader read this NBT, the borrow reader didn't: {e}\n{tag:?}")
    });
    assert_eq!(
        cursor.position(),
        owned_end,
        "the borrow reader read a different number of bytes"
    );
    let from_borrowed = borrowed.as_tag().to_owned();
    let mut bytes3 = Vec::new();
    from_borrowed.azalea_write(&mut bytes3).unwrap();
    assert!(
        bytes1 == bytes3,
        "owned and borrowed reads differ\nowned: {tag:?}\nborrowed: {from_borrowed:?}"
    );
}

/// A chat component, as network NBT (or JSON when the input starts like
/// JSON), converted every way a bot might.
pub fn chat_component(data: &[u8]) {
    let text = if matches!(data.first(), Some(b'{' | b'[' | b'"')) {
        let Ok(text) = serde_json::from_slice::<FormattedText>(data) else {
            return;
        };
        text
    } else {
        let Ok(text) = FormattedText::azalea_read(&mut Cursor::new(data)) else {
            return;
        };
        text
    };
    let _ = text.to_string();
    let _ = text.to_ansi();
    let _ = text.to_html();
    let _ = format!("{text:?}");
    let json = serde_json::to_string(&text).unwrap();
    let _ = serde_json::from_str::<FormattedText>(&json);
    let mut bytes1 = Vec::new();
    text.azalea_write(&mut bytes1).unwrap();
    let again = FormattedText::azalea_read(&mut Cursor::new(&bytes1[..]))
        .unwrap_or_else(|e| panic!("our own chat NBT doesn't read back: {e}\n{text:?}"));
    let mut bytes2 = Vec::new();
    again.azalea_write(&mut bytes2).unwrap();
    assert!(
        bytes1 == bytes2,
        "chat NBT write isn't stable\n{text:?}\n{again:?}"
    );
}

/// A paletted container: byte 0 picks block states or biomes and the
/// registry size; every value is read, then it's written back and read
/// again.
pub fn paletted_container(data: &[u8]) {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let registry_size = match mode >> 1 {
        0 => None,
        n => Some(n as usize),
    };
    if mode & 1 == 0 {
        container::<BlockState>(rest, registry_size);
    } else {
        container::<Biome>(rest, registry_size);
    }
}

fn container<S>(data: &[u8], registry_size: Option<usize>)
where
    S: azalea_world::palette::PalletedContainerKind + 'static,
{
    let Ok(c) =
        PalettedContainer::<S>::read_with_registry_size(&mut Cursor::new(data), registry_size)
    else {
        return;
    };
    let n = if std::any::TypeId::of::<S>() == std::any::TypeId::of::<BlockState>() {
        4096
    } else {
        64
    };
    for i in 0..n {
        let _ = c.get_at_index(i);
    }
    let mut bytes1 = Vec::new();
    c.write(&mut bytes1).unwrap();
    let again = PalettedContainer::<S>::read_with_registry_size(
        &mut Cursor::new(&bytes1[..]),
        registry_size,
    )
    .unwrap_or_else(|e| panic!("our own container doesn't read back: {e}"));
    let mut bytes2 = Vec::new();
    again.write(&mut bytes2).unwrap();
    assert!(bytes1 == bytes2, "container write isn't stable");
}

/// Chunk sections as in a chunk packet: byte 0 picks the dimension height,
/// byte 1 the biome registry size; then every block of the chunk is read.
pub fn chunk_section(data: &[u8]) {
    if data.len() < 2 {
        return;
    }
    // vanilla's bounds: 16..=4064 in steps of 16; keep it small enough to
    // stay fast (and use the overworld most of the time)
    let height = match data[0] {
        0..=127 => 384,
        n => (u32::from(n) % 32 + 1) * 16,
    };
    let biome_registry_size = (data[1] != 0).then_some(data[1] as usize);
    let mut cursor = Cursor::new(&data[2..]);
    if let Ok(section) =
        Section::read_with_biome_registry_size(&mut cursor.clone(), biome_registry_size)
    {
        for i in 0..4096u16 {
            let pos =
                ChunkSectionBlockPos::new((i & 15) as u8, (i >> 8) as u8, ((i >> 4) & 15) as u8);
            let _ = section.get_block_state(pos);
        }
        let mut out = Vec::new();
        section.azalea_write(&mut out).unwrap();
    }
    let Ok(chunk) = Chunk::read_with_dimension(&mut cursor, height, -64, &[], biome_registry_size)
    else {
        return;
    };
    for y in (-64..(-64 + height as i32)).step_by(7) {
        for xz in 0..16 {
            let _ = chunk.get_block_state(
                &azalea_core::position::ChunkBlockPos::new(xz, y, 15 - xz),
                -64,
            );
        }
    }
}

/// How many packets a handler sequence may hold (a bigger input is cut).
pub const MAX_SEQUENCE: usize = 256;

thread_local! {
    /// One simulation app per fuzzing process (libFuzzer runs inputs on one
    /// thread): building an app and its runtime costs far more than an
    /// input, so each input [`Simulation::reset`]s it instead.
    static SIMULATION: std::cell::RefCell<Option<Simulation>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` on the process's simulation, reset to a fresh client in
/// `protocol` (built on first use).
fn with_simulation(protocol: ConnectionProtocol, f: impl FnOnce(&mut Simulation)) {
    SIMULATION.with(|cell| {
        let mut slot = cell.borrow_mut();
        match slot.as_mut() {
            Some(simulation) => simulation.reset(protocol),
            None => *slot = Some(Simulation::new(protocol)),
        }
        if let Some(simulation) = slot.as_mut() {
            f(simulation);
        }
    });
}

/// Azalea's networkless client in play, fed a list of clientbound game
/// packets (see [`packet_list`]); it ticks after every 8 packets.
pub fn handler_sequence(data: &[u8]) {
    with_simulation(ConnectionProtocol::Game, |simulation| {
        handler_sequence_on(simulation, data)
    });
}

fn handler_sequence_on(simulation: &mut Simulation, data: &[u8]) {
    simulation.receive_packet(default_login_packet());
    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), (384 + 64) / 16));
    simulation.receive_packet(make_basic_add_entity(
        EntityKind::Zombie,
        7,
        (0.5, 70., 0.5),
    ));
    simulation.tick();
    feed(simulation, &packet_list(data));
}

/// Whether the client still has its connection (the bad packet policy
/// closes it for some packets; a closed client gets no more packets).
fn connected(simulation: &Simulation) -> bool {
    simulation.has_component::<RawConnection>()
}

fn feed(simulation: &mut Simulation, packets: &[&[u8]]) {
    for (i, p) in packets.iter().take(MAX_SEQUENCE).enumerate() {
        if !connected(simulation) {
            break;
        }
        simulation.receive_raw_packet(*p);
        if i % 8 == 7 {
            simulation.tick();
        }
    }
    simulation.tick();
    simulation.tick();
}

/// A fuzzed configuration phase (registry data, tags, ...) then seed play
/// packets: login, a chunk, an entity, so registry-derived values (dimension
/// heights, registry sizes) reach chunk and entity code. Play packets after a
/// [`PLAY_MARKER`] entry are fed too.
pub fn config_to_play(data: &[u8]) {
    with_simulation(ConnectionProtocol::Configuration, |simulation| {
        config_to_play_on(simulation, data)
    });
}

fn config_to_play_on(simulation: &mut Simulation, data: &[u8]) {
    let packets = packet_list(data);
    let split = packets
        .iter()
        .position(|p| *p == PLAY_MARKER)
        .unwrap_or(packets.len());
    let (config, play) = packets.split_at(split);
    for p in config.iter().take(MAX_SEQUENCE) {
        if !connected(simulation) {
            return;
        }
        // a decodable finish would switch states early; it's sent below
        if let Ok(ClientboundConfigPacket::FinishConfiguration(_)) =
            deserialize_packet::<ClientboundConfigPacket>(&mut Cursor::new(*p))
        {
            continue;
        }
        simulation.receive_raw_packet(*p);
    }
    simulation.tick();
    if !connected(simulation) {
        return;
    }
    simulation.receive_packet(ClientboundFinishConfiguration);
    simulation.tick();
    if !connected(simulation) {
        return;
    }
    simulation.receive_packet(default_login_packet());
    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), (384 + 64) / 16));
    simulation.receive_packet(make_basic_add_entity(
        EntityKind::Zombie,
        7,
        (0.5, 70., 0.5),
    ));
    simulation.tick();
    let play: Vec<&[u8]> = play.iter().skip(1).copied().collect();
    feed(simulation, &play);
}
