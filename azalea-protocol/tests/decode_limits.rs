//! Each decode limit is counted in the decode error counter under its own
//! kind when a packet trips it (WP2 limits table).

use std::io::Cursor;

use azalea_buf::AzBufVar;
use azalea_protocol::{
    decode_errors,
    packets::{
        Packet, ProtocolPacket,
        game::{
            ClientboundGamePacket, c_remove_entities::ClientboundRemoveEntities,
            c_system_chat::ClientboundSystemChat,
        },
        status::ClientboundStatusPacket,
    },
    read::deserialize_packet,
};

fn varint(n: i32) -> Vec<u8> {
    let mut v = Vec::new();
    n.azalea_write_var(&mut v).unwrap();
    v
}

fn game_id(p: ClientboundGamePacket) -> Vec<u8> {
    varint(p.id() as i32)
}

#[test]
fn string_length_is_counted() {
    // status response (id 0): a JSON string over 3 * 32767 bytes
    let mut packet = varint(0);
    packet.extend(varint(3 * 32767 + 1));
    let before = decode_errors();
    assert!(deserialize_packet::<ClientboundStatusPacket>(&mut Cursor::new(&packet[..])).is_err());
    let after = decode_errors();
    assert!(after.string_length > before.string_length);
}

#[test]
fn collection_length_is_counted() {
    let id = game_id(ClientboundRemoveEntities { entity_ids: vec![] }.into_variant());
    // 3 VarInt entity ids announced, 2 bytes left
    let mut packet = id.clone();
    packet.extend(varint(3));
    packet.extend([1, 2]);
    let before = decode_errors();
    assert!(deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(&packet[..])).is_err());
    let after = decode_errors();
    assert!(after.collection_length > before.collection_length);
    // the boundary: 2 ids in 2 bytes are fine
    let mut packet = id;
    packet.extend(varint(2));
    packet.extend([1, 2]);
    assert!(deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(&packet[..])).is_ok());
}

/// Network NBT: `depth` nested compounds under key "a".
fn nested_compounds(depth: usize) -> Vec<u8> {
    let mut v = vec![10u8];
    for _ in 0..depth - 1 {
        v.extend_from_slice(&[10, 0, 1, b'a']);
    }
    v.extend(std::iter::repeat_n(0, depth));
    v
}

#[test]
fn nbt_depth_and_size_are_counted() {
    let id = game_id(
        ClientboundSystemChat {
            content: "".into(),
            overlay: false,
        }
        .into_variant(),
    );
    let mut packet = id.clone();
    packet.extend(nested_compounds(513));
    packet.push(0);
    let before = decode_errors();
    assert!(deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(&packet[..])).is_err());
    let after = decode_errors();
    assert!(after.nbt_depth > before.nbt_depth);
    assert_eq!(after.packet_body, before.packet_body);

    // a string tag accounted at 36 + 2 * 65535 bytes, in a list of 17: over
    // the 2 097 152 quota
    let mut packet = id;
    packet.extend([9, 8]);
    packet.extend(17i32.to_be_bytes());
    for _ in 0..17 {
        packet.extend(0xffffu16.to_be_bytes());
        packet.extend(std::iter::repeat_n(b'a', 0xffff));
    }
    packet.push(0);
    let before = decode_errors();
    assert!(deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(&packet[..])).is_err());
    let after = decode_errors();
    assert!(after.nbt_size > before.nbt_size);
}
