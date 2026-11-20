//! The deepest input every decode limit allows is decoded, converted,
//! formatted (`Debug`), cloned and dropped on a thread with a bot thread's
//! 16 MB stack without overflowing it (WP2 limits table, last row), each in
//! well under a second.

use std::{
    io::Cursor,
    time::{Duration, Instant},
};

use azalea_buf::{AzBuf, AzBufVar};
use azalea_protocol::{
    packets::{
        Packet, ProtocolPacket,
        game::{
            ClientboundGamePacket, c_block_entity_data::ClientboundBlockEntityData,
            c_container_set_slot::ClientboundContainerSetSlot,
            c_place_ghost_recipe::ClientboundPlaceGhostRecipe,
            c_system_chat::ClientboundSystemChat,
        },
    },
    read::deserialize_packet,
};
use azalea_registry::builtin::{DataComponentKind, ItemKind};

const BOT_STACK: usize = 16 * 1024 * 1024;

fn on_bot_thread(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(BOT_STACK)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

fn varint(n: i32) -> Vec<u8> {
    let mut v = Vec::new();
    n.azalea_write_var(&mut v).unwrap();
    v
}

/// The packet's id and its fields up to (not including) the last one, which
/// the caller appends: write the packet with a cheap last field and cut that
/// field's bytes off.
fn head(packet: ClientboundGamePacket, last_field_len: usize) -> Vec<u8> {
    let mut v = varint(packet.id() as i32);
    packet.write(&mut v).unwrap();
    v.truncate(v.len() - last_field_len);
    v
}

/// Decode, format, clone and drop a packet, and check it took under 1 s.
fn exercise(packet: &[u8]) -> ClientboundGamePacket {
    let start = Instant::now();
    let p = deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(packet)).unwrap();
    let debug = format!("{p:?}");
    assert!(!debug.is_empty());
    let copy = p.clone();
    drop(copy);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    p
}

fn key(buf: &mut Vec<u8>, ty: u8, name: &str) {
    buf.push(ty);
    buf.extend_from_slice(&(name.len() as u16).to_be_bytes());
    buf.extend_from_slice(name.as_bytes());
}
fn string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u16).to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
}

/// A chat component as deep as network NBT allows (512 levels): 255
/// translate levels, each a compound and its one-element `with` list, then
/// a leaf with an `extra` list holding one text compound.
fn deepest_chat() -> Vec<u8> {
    let mut buf = vec![10u8];
    for _ in 0..255 {
        key(&mut buf, 8, "translate");
        string(&mut buf, "chat.type.text");
        key(&mut buf, 1, "bold");
        buf.push(1);
        key(&mut buf, 9, "with");
        buf.push(10);
        buf.extend_from_slice(&1i32.to_be_bytes());
    }
    // the leaf at depth 511, its extra list at 512 holding strings
    key(&mut buf, 8, "text");
    string(&mut buf, "leaf");
    key(&mut buf, 9, "extra");
    buf.push(8);
    buf.extend_from_slice(&1i32.to_be_bytes());
    string(&mut buf, "end");
    buf.extend(std::iter::repeat_n(0u8, 256));
    buf
}

/// A hover chain as deep as network NBT allows: each level's hover event
/// holds the next level.
fn deepest_hover_chain() -> Vec<u8> {
    let mut buf = vec![10u8];
    for _ in 0..255 {
        key(&mut buf, 8, "text");
        string(&mut buf, "x");
        key(&mut buf, 10, "hoverEvent");
        key(&mut buf, 8, "action");
        string(&mut buf, "show_text");
        key(&mut buf, 10, "contents");
    }
    // the innermost contents compound is at depth 511; one more level
    key(&mut buf, 9, "extra");
    buf.push(8);
    buf.extend_from_slice(&1i32.to_be_bytes());
    string(&mut buf, "end");
    key(&mut buf, 8, "text");
    string(&mut buf, "leaf");
    buf.push(0);
    buf.extend(std::iter::repeat_n(0u8, 255 * 2));
    buf
}

#[test]
fn deepest_chat_on_a_bot_thread() {
    on_bot_thread(|| {
        for chat in [deepest_chat(), deepest_hover_chain()] {
            let mut packet = head(
                ClientboundSystemChat {
                    content: "".into(),
                    overlay: false,
                }
                .into_variant(),
                // an empty string tag (8, 0, 0) and the bool
                4,
            );
            packet.extend(&chat);
            packet.push(0);
            let start = Instant::now();
            let ClientboundGamePacket::SystemChat(p) = exercise(&packet) else {
                panic!("not system chat");
            };
            // convert it every way a bot might
            let ansi = p.content.to_ansi();
            let html = p.content.to_html();
            let plain = p.content.to_string();
            assert!(ansi.len() + html.len() + plain.len() > 0);
            let json = serde_json::to_string(&p.content).unwrap();
            assert!(!json.is_empty());
            let mut out = Vec::new();
            p.content.azalea_write(&mut out).unwrap();
            drop(p);
            assert!(
                start.elapsed() < Duration::from_secs(1),
                "{:?}",
                start.elapsed()
            );
        }
    });
}

/// NBT 512 levels deep: nested compounds under key "a".
fn deepest_nbt() -> Vec<u8> {
    let mut v = vec![10u8];
    for _ in 0..511 {
        v.extend_from_slice(&[10, 0, 1, b'a']);
    }
    v.extend(std::iter::repeat_n(0, 512));
    v
}

#[test]
fn deepest_nbt_on_a_bot_thread() {
    on_bot_thread(|| {
        let mut packet = head(
            ClientboundBlockEntityData {
                pos: Default::default(),
                block_entity_type: azalea_registry::builtin::BlockEntityKind::Chest,
                tag: simdnbt::owned::Nbt::None,
            }
            .into_variant(),
            1,
        );
        packet.extend(deepest_nbt());
        exercise(&packet);
    });
}

/// 512 item stacks, each but the last holding the next in a container; the
/// innermost has a custom name as deep as network NBT allows and custom
/// data 512 levels deep.
fn deepest_items() -> Vec<u8> {
    let mut v = Vec::new();
    for _ in 0..511 {
        1i32.azalea_write_var(&mut v).unwrap();
        ItemKind::ShulkerBox.azalea_write(&mut v).unwrap();
        1u32.azalea_write_var(&mut v).unwrap();
        0u32.azalea_write_var(&mut v).unwrap();
        DataComponentKind::Container.azalea_write(&mut v).unwrap();
        1u32.azalea_write_var(&mut v).unwrap();
    }
    1i32.azalea_write_var(&mut v).unwrap();
    ItemKind::Stone.azalea_write(&mut v).unwrap();
    2u32.azalea_write_var(&mut v).unwrap();
    0u32.azalea_write_var(&mut v).unwrap();
    DataComponentKind::CustomName.azalea_write(&mut v).unwrap();
    v.extend(deepest_chat());
    DataComponentKind::CustomData.azalea_write(&mut v).unwrap();
    v.extend(deepest_nbt());
    v
}

#[test]
fn deepest_items_on_a_bot_thread() {
    on_bot_thread(|| {
        let mut packet = head(
            ClientboundContainerSetSlot {
                container_id: 0,
                state_id: 1,
                slot: 36,
                item_stack: azalea_inventory::ItemStack::Empty,
            }
            .into_variant(),
            1,
        );
        packet.extend(deepest_items());
        let ClientboundGamePacket::ContainerSetSlot(p) = exercise(&packet) else {
            panic!("not container_set_slot");
        };
        // and it writes back to bytes that decode to the same stack (the
        // chat inside re-encodes in its own key order)
        let mut out = Vec::new();
        p.item_stack.azalea_write(&mut out).unwrap();
        let back = azalea_inventory::ItemStack::azalea_read(&mut Cursor::new(&out[..])).unwrap();
        assert!(back == p.item_stack);
    });
}

/// 511 slot displays, each but the last a `with_remainder` whose input is
/// the next one, the innermost an item stack display whose item is the
/// 512th nesting level: slot displays and items share the nesting limit.
#[test]
fn deepest_recipe_on_a_bot_thread() {
    on_bot_thread(|| {
        let mut packet = head(
            ClientboundPlaceGhostRecipe {
                container_id: 1,
                recipe: azalea_protocol::common::recipe::RecipeDisplayData::Stonecutter(
                    azalea_protocol::common::recipe::StonecutterRecipeDisplay {
                        input: azalea_protocol::common::recipe::SlotDisplayData::Empty,
                        result: azalea_protocol::common::recipe::SlotDisplayData::Empty,
                        crafting_station: azalea_protocol::common::recipe::SlotDisplayData::Empty,
                    },
                ),
            }
            .into_variant(),
            3,
        );
        // the stonecutter display's input
        packet.extend(std::iter::repeat_n(6u8, 510));
        // the ItemStack slot display (3) at nesting depth 511, its item at
        // 512, which may not nest any further
        packet.push(3);
        1i32.azalea_write_var(&mut packet).unwrap();
        ItemKind::Stone.azalea_write(&mut packet).unwrap();
        packet.extend([0, 0]);
        // every with_remainder's remainder, then result and crafting station
        packet.extend(std::iter::repeat_n(0u8, 510 + 2));
        exercise(&packet);

        // one item level more is the 513th nesting level
        let mut deeper = packet.clone();
        let at = packet.len() - (510 + 2) - 2;
        deeper.splice(at..at + 2, {
            let mut v = vec![1, 0];
            DataComponentKind::Container.azalea_write(&mut v).unwrap();
            v.push(1);
            1i32.azalea_write_var(&mut v).unwrap();
            ItemKind::Stone.azalea_write(&mut v).unwrap();
            v.extend([0, 0]);
            v
        });
        assert!(
            deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(&deeper[..])).is_err()
        );
    });
}
