//! Hand-written deep seeds: every recursive type at the deepest its limit
//! allows (WP2 section 5). Each is also a unit test (`tests/deep_seeds.rs`,
//! 1 s cap on a 16 MB bot thread), and `examples/write_seeds.rs` writes them
//! into a corpus directory.

use azalea_buf::{AzBuf, AzBufVar};
use azalea_protocol::packets::{
    Packet, ProtocolPacket,
    game::{
        ClientboundGamePacket,
        c_block_entity_data::ClientboundBlockEntityData,
        c_commands::{BrigadierNodeStub, ClientboundCommands, NodeType},
        c_container_set_slot::ClientboundContainerSetSlot,
        c_place_ghost_recipe::ClientboundPlaceGhostRecipe,
        c_system_chat::ClientboundSystemChat,
    },
};
use azalea_registry::builtin::{DataComponentKind, ItemKind};

use crate::input::write_packet_list;

pub struct Seed {
    /// The fuzz target whose corpus it belongs to.
    pub target: &'static str,
    pub name: &'static str,
    pub data: Vec<u8>,
}

const NBT_COMPOUND: u8 = 10;
const NBT_LIST: u8 = 9;
const NBT_STRING: u8 = 8;
const NBT_BYTE: u8 = 1;

fn varint(n: i32) -> Vec<u8> {
    let mut v = Vec::new();
    n.azalea_write_var(&mut v).unwrap();
    v
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

/// A packet's id and fields up to (not including) its last field, which the
/// caller appends: the packet is written with a cheap last field whose
/// `last_field_len` bytes are cut off.
fn head(packet: ClientboundGamePacket, last_field_len: usize) -> Vec<u8> {
    let mut v = varint(packet.id() as i32);
    packet.write(&mut v).unwrap();
    v.truncate(v.len() - last_field_len);
    v
}

/// Chat as deep as network NBT allows (512): 255 `translate` levels, each a
/// compound and its one-element `with` list, then a leaf with an `extra`
/// list.
pub fn chat_translate_with() -> Vec<u8> {
    let mut buf = vec![NBT_COMPOUND];
    for _ in 0..255 {
        key(&mut buf, NBT_STRING, "translate");
        string(&mut buf, "chat.type.text");
        key(&mut buf, NBT_BYTE, "bold");
        buf.push(1);
        key(&mut buf, NBT_LIST, "with");
        buf.push(NBT_COMPOUND);
        buf.extend_from_slice(&1i32.to_be_bytes());
    }
    key(&mut buf, NBT_STRING, "text");
    string(&mut buf, "leaf");
    key(&mut buf, NBT_LIST, "extra");
    buf.push(NBT_STRING);
    buf.extend_from_slice(&1i32.to_be_bytes());
    string(&mut buf, "end");
    buf.extend(std::iter::repeat_n(0u8, 256));
    buf
}

/// Chat 512 deep through `extra` alone: 255 levels of a styled text compound
/// whose `extra` list holds the next one.
pub fn chat_extra() -> Vec<u8> {
    let mut buf = vec![NBT_COMPOUND];
    for i in 0..255 {
        key(&mut buf, NBT_STRING, "text");
        string(&mut buf, "x");
        key(&mut buf, NBT_STRING, "color");
        string(&mut buf, if i % 2 == 0 { "red" } else { "#00ff00" });
        key(&mut buf, NBT_LIST, "extra");
        buf.push(NBT_COMPOUND);
        buf.extend_from_slice(&1i32.to_be_bytes());
    }
    key(&mut buf, NBT_STRING, "text");
    string(&mut buf, "leaf");
    key(&mut buf, NBT_LIST, "extra");
    buf.push(NBT_STRING);
    buf.extend_from_slice(&2i32.to_be_bytes());
    string(&mut buf, "a");
    string(&mut buf, "b");
    buf.extend(std::iter::repeat_n(0u8, 256));
    buf
}

/// A hover chain as deep as network NBT allows: each level's `show_text`
/// hover event holds the next level.
pub fn chat_hover() -> Vec<u8> {
    let mut buf = vec![NBT_COMPOUND];
    for _ in 0..255 {
        key(&mut buf, NBT_STRING, "text");
        string(&mut buf, "x");
        key(&mut buf, NBT_COMPOUND, "hoverEvent");
        key(&mut buf, NBT_STRING, "action");
        string(&mut buf, "show_text");
        key(&mut buf, NBT_COMPOUND, "contents");
    }
    key(&mut buf, NBT_LIST, "extra");
    buf.push(NBT_STRING);
    buf.extend_from_slice(&1i32.to_be_bytes());
    string(&mut buf, "end");
    key(&mut buf, NBT_STRING, "text");
    string(&mut buf, "leaf");
    buf.push(0);
    buf.extend(std::iter::repeat_n(0u8, 255 * 2));
    buf
}

/// NBT compounds nested 512 deep (key "a").
pub fn nbt_compounds() -> Vec<u8> {
    let mut v = vec![NBT_COMPOUND];
    for _ in 0..511 {
        v.extend_from_slice(&[NBT_COMPOUND, 0, 1, b'a']);
    }
    v.extend(std::iter::repeat_n(0, 512));
    v
}

/// NBT lists of lists nested 512 deep.
pub fn nbt_lists() -> Vec<u8> {
    let mut v = vec![NBT_LIST];
    for _ in 0..511 {
        v.push(NBT_LIST);
        v.extend_from_slice(&1i32.to_be_bytes());
    }
    v.push(0);
    v.extend_from_slice(&0i32.to_be_bytes());
    v
}

/// NBT alternating lists and compounds, 512 deep.
pub fn nbt_mixed() -> Vec<u8> {
    // compound { "l": [ compound { "l": [ ... ] } ] }
    let mut v = vec![NBT_COMPOUND];
    for _ in 0..255 {
        key(&mut v, NBT_LIST, "l");
        v.push(NBT_COMPOUND);
        v.extend_from_slice(&1i32.to_be_bytes());
    }
    key(&mut v, NBT_STRING, "s");
    string(&mut v, "leaf");
    v.extend(std::iter::repeat_n(0, 256));
    v
}

/// 512 item stacks, each but the last holding the next in a component of
/// `kind` (`container` or `bundle_contents`); the innermost has a custom
/// name as deep as chat goes and custom data 512 levels deep.
pub fn nested_items(kind: DataComponentKind) -> Vec<u8> {
    let mut v = Vec::new();
    for _ in 0..511 {
        1i32.azalea_write_var(&mut v).unwrap();
        ItemKind::ShulkerBox.azalea_write(&mut v).unwrap();
        1u32.azalea_write_var(&mut v).unwrap();
        0u32.azalea_write_var(&mut v).unwrap();
        kind.azalea_write(&mut v).unwrap();
        1u32.azalea_write_var(&mut v).unwrap();
    }
    1i32.azalea_write_var(&mut v).unwrap();
    ItemKind::Stone.azalea_write(&mut v).unwrap();
    2u32.azalea_write_var(&mut v).unwrap();
    0u32.azalea_write_var(&mut v).unwrap();
    DataComponentKind::CustomName.azalea_write(&mut v).unwrap();
    v.extend(chat_translate_with());
    DataComponentKind::CustomData.azalea_write(&mut v).unwrap();
    v.extend(nbt_compounds());
    v
}

/// A written book whose pages (raw and filtered) are chat as deep as it
/// goes, inside a shulker box.
pub fn written_book() -> Vec<u8> {
    let mut v = Vec::new();
    // the shulker box holding the book
    1i32.azalea_write_var(&mut v).unwrap();
    ItemKind::ShulkerBox.azalea_write(&mut v).unwrap();
    1u32.azalea_write_var(&mut v).unwrap();
    0u32.azalea_write_var(&mut v).unwrap();
    DataComponentKind::Container.azalea_write(&mut v).unwrap();
    1u32.azalea_write_var(&mut v).unwrap();
    // the book
    1i32.azalea_write_var(&mut v).unwrap();
    ItemKind::WrittenBook.azalea_write(&mut v).unwrap();
    1u32.azalea_write_var(&mut v).unwrap();
    0u32.azalea_write_var(&mut v).unwrap();
    DataComponentKind::WrittenBookContent
        .azalea_write(&mut v)
        .unwrap();
    // title: raw, then a filtered one
    "deep".to_owned().azalea_write(&mut v).unwrap();
    v.push(1);
    "deep!".to_owned().azalea_write(&mut v).unwrap();
    "author".to_owned().azalea_write(&mut v).unwrap();
    0i32.azalea_write_var(&mut v).unwrap();
    // pages
    let pages = 9u32;
    pages.azalea_write_var(&mut v).unwrap();
    for i in 0..pages {
        let page = match i % 3 {
            0 => chat_translate_with(),
            1 => chat_extra(),
            _ => chat_hover(),
        };
        v.extend(&page);
        v.push(1);
        v.extend(&page);
    }
    // resolved
    v.push(1);
    v
}

fn set_slot(item: Vec<u8>) -> Vec<u8> {
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
    packet.extend(item);
    packet
}

fn system_chat(chat: Vec<u8>) -> Vec<u8> {
    let mut packet = head(
        ClientboundSystemChat {
            content: "".into(),
            overlay: false,
        }
        .into_variant(),
        // an empty string tag and the bool
        4,
    );
    packet.extend(chat);
    packet.push(0);
    packet
}

fn block_entity_data(nbt: Vec<u8>) -> Vec<u8> {
    let mut packet = head(
        ClientboundBlockEntityData {
            pos: Default::default(),
            block_entity_type: azalea_registry::builtin::BlockEntityKind::Chest,
            tag: simdnbt::owned::Nbt::None,
        }
        .into_variant(),
        1,
    );
    packet.extend(nbt);
    packet
}

/// 511 `with_remainder` slot displays, each holding the next, the innermost
/// an item stack display: slot displays and items share the nesting limit.
pub fn recipe_slot_displays() -> Vec<u8> {
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
    packet.extend(std::iter::repeat_n(6u8, 510));
    packet.push(3);
    1i32.azalea_write_var(&mut packet).unwrap();
    ItemKind::Stone.azalea_write(&mut packet).unwrap();
    packet.extend([0, 0]);
    packet.extend(std::iter::repeat_n(0u8, 510 + 2));
    packet
}

/// A brigadier command tree that's one long chain (`n` literal nodes, each
/// the only child of the one before) whose last node redirects to itself,
/// plus an argument node with a redirect back to the root.
pub fn command_chain(n: u32) -> Vec<u8> {
    let mut entries = vec![BrigadierNodeStub {
        is_executable: false,
        children: vec![1],
        redirect_node: None,
        node_type: NodeType::Root,
    }];
    for i in 1..n {
        entries.push(BrigadierNodeStub {
            is_executable: true,
            children: if i + 1 < n { vec![i + 1] } else { vec![] },
            redirect_node: if i + 1 == n {
                Some(i)
            } else if i == 1 {
                Some(0)
            } else {
                None
            },
            node_type: NodeType::Literal {
                name: "a".to_owned(),
            },
        });
    }
    let packet = ClientboundCommands {
        entries,
        root_index: 0,
    }
    .into_variant();
    let mut bytes = varint(packet.id() as i32);
    packet.write(&mut bytes).unwrap();
    bytes
}

/// Every deep seed, by target.
pub fn all() -> Vec<Seed> {
    let mut out = Vec::new();
    let chats = [
        ("deep_chat_translate_with", chat_translate_with()),
        ("deep_chat_extra", chat_extra()),
        ("deep_chat_hover", chat_hover()),
    ];
    let nbts = [
        ("deep_nbt_compounds", nbt_compounds()),
        ("deep_nbt_lists", nbt_lists()),
        ("deep_nbt_mixed", nbt_mixed()),
    ];
    let mut packets: Vec<(&'static str, Vec<u8>)> = vec![
        (
            "deep_items_container",
            set_slot(nested_items(DataComponentKind::Container)),
        ),
        (
            "deep_items_bundle",
            set_slot(nested_items(DataComponentKind::BundleContents)),
        ),
        ("deep_written_book", set_slot(written_book())),
        ("deep_recipe_slot_displays", recipe_slot_displays()),
        ("deep_command_chain", command_chain(50_000)),
    ];
    for (name, chat) in &chats {
        out.push(Seed {
            target: "chat_component",
            name,
            data: chat.clone(),
        });
        packets.push((name, system_chat(chat.clone())));
    }
    for (name, nbt) in &nbts {
        out.push(Seed {
            target: "nbt_network",
            name,
            data: nbt.clone(),
        });
        // a block entity's tag is a compound
        if nbt[0] == NBT_COMPOUND {
            packets.push((name, block_entity_data(nbt.clone())));
        }
    }
    for (name, packet) in &packets {
        for target in ["clientbound_game", "roundtrip_game_clientbound"] {
            out.push(Seed {
                target,
                name,
                data: packet.clone(),
            });
        }
        if packet.len() <= u16::MAX as usize {
            out.push(Seed {
                target: "handler_sequence",
                name,
                data: write_packet_list([&packet[..]]),
            });
        }
    }
    out
}
