//! Wire format tests for data components, checked by hand against the
//! 1.21.4 stream codecs.

use std::io::Cursor;

use azalea_buf::{AzBuf, AzBufVar};
use azalea_core::attribute_modifier_operation::AttributeModifierOperation;
use azalea_inventory::{
    ItemStack,
    components::{
        AdventureModePredicate, AttributeModifier, AttributeModifiers, AttributeModifiersEntry,
        BlockPredicate, BlockStatePropertyMatcher, BlockStateValueMatcher, DyedColor, Enchantments,
        EquipmentSlot, EquipmentSlotGroup, Equippable, IntangibleProjectile, JukeboxPlayable,
        JukeboxSongHolder, Recipes, Unbreakable,
    },
};
use azalea_registry::{
    DataRegistry, Holder,
    builtin::{Attribute, BlockKind, ItemKind, SoundEvent},
    data::{Enchantment, JukeboxSong},
};

fn encode<T: AzBuf>(value: &T) -> Vec<u8> {
    let mut buf = Vec::new();
    value.azalea_write(&mut buf).unwrap();
    buf
}

fn roundtrip<T: AzBuf + PartialEq + std::fmt::Debug>(value: &T) {
    let buf = encode(value);
    let mut cursor = Cursor::new(buf.as_slice());
    let decoded = T::azalea_read(&mut cursor).unwrap();
    assert_eq!(&decoded, value);
    assert_eq!(cursor.position() as usize, buf.len(), "trailing bytes");
}

#[test]
fn test_unbreakable_has_show_in_tooltip() {
    assert_eq!(encode(&Unbreakable::new()), [1]);
    roundtrip(&Unbreakable {
        show_in_tooltip: false,
    });
}

#[test]
fn test_dyed_color() {
    let c = DyedColor {
        rgb: 0x123456,
        show_in_tooltip: false,
    };
    assert_eq!(encode(&c), [0x00, 0x12, 0x34, 0x56, 0]);
}

#[test]
fn test_enchantments() {
    let mut c = Enchantments::new();
    c.levels.insert(Enchantment::new_raw(3), 5);
    // map size, enchantment id, level, show_in_tooltip
    assert_eq!(encode(&c), [1, 3, 5, 1]);
    roundtrip(&c);
}

#[test]
fn test_block_state_value_matcher_either() {
    // ByteBufCodecs.either writes true for the left side (exact)
    let exact = BlockStateValueMatcher::Exact {
        value: "a".to_owned(),
    };
    assert_eq!(encode(&exact), [1, 1, b'a']);
    let range = BlockStateValueMatcher::Range {
        min: None,
        max: Some("b".to_owned()),
    };
    assert_eq!(encode(&range), [0, 0, 1, 1, b'b']);
    roundtrip(&exact);
    roundtrip(&range);
}

#[test]
fn test_adventure_mode_predicate() {
    let c = AdventureModePredicate {
        predicates: vec![BlockPredicate {
            blocks: Some(vec![BlockKind::Stone].into()),
            properties: Some(vec![BlockStatePropertyMatcher {
                name: "x".to_owned(),
                value_matcher: BlockStateValueMatcher::Exact {
                    value: "y".to_owned(),
                },
            }]),
            nbt: None,
        }],
        show_in_tooltip: true,
    };
    let stone = BlockKind::Stone as u8;
    assert_eq!(
        encode(&c),
        // list len, Some(holder set of 1: stone), Some([x = exact y]), None,
        // show_in_tooltip
        [1, 1, 2, stone, 1, 1, 1, b'x', 1, 1, b'y', 0, 1]
    );
    roundtrip(&c);
}

#[test]
fn test_attribute_modifiers() {
    let c = AttributeModifiers {
        modifiers: vec![AttributeModifiersEntry {
            kind: Attribute::Scale,
            modifier: AttributeModifier {
                id: "a:b".into(),
                amount: 4.0,
                operation: AttributeModifierOperation::AddMultipliedBase,
            },
            slot: EquipmentSlotGroup::Hand,
        }],
        show_in_tooltip: false,
    };
    let mut expected = vec![1, Attribute::Scale as u8, 3, b'a', b':', b'b'];
    expected.extend(4.0f64.to_be_bytes());
    // operation, slot group, show_in_tooltip
    expected.extend([1, 3, 0]);
    assert_eq!(encode(&c), expected);
    roundtrip(&c);
}

#[test]
fn test_equippable_uses_slot_ids() {
    let c = Equippable {
        slot: EquipmentSlot::Offhand,
        ..Equippable::new()
    };
    let buf = encode(&c);
    // EquipmentSlot.OFFHAND has id 5 (its ordinal is 1)
    assert_eq!(buf[0], 5);
    // equip_sound is a holder, so it's the registry id + 1
    let mut sound = Vec::new();
    (SoundEvent::ItemArmorEquipGeneric as u32 + 1)
        .azalea_write_var(&mut sound)
        .unwrap();
    assert_eq!(&buf[1..1 + sound.len()], &sound);
    roundtrip(&c);

    for slot in EquipmentSlot::values() {
        roundtrip(&Equippable {
            slot,
            ..Equippable::new()
        });
    }
}

#[test]
fn test_jukebox_playable_either_holder() {
    let c = JukeboxPlayable {
        song: JukeboxSongHolder::Holder(Holder::Reference(JukeboxSong::new_raw(2))),
        show_in_tooltip: true,
    };
    assert_eq!(encode(&c), [1, 3, 1]);
    roundtrip(&c);
    let c = JukeboxPlayable {
        song: JukeboxSongHolder::Key("a:b".into()),
        show_in_tooltip: false,
    };
    assert_eq!(encode(&c), [0, 3, b'a', b':', b'b', 0]);
    roundtrip(&c);
}

#[test]
fn test_nbt_only_components() {
    // empty compound, as Unit.CODEC encodes it
    assert_eq!(encode(&IntangibleProjectile), [0x0a, 0x00]);
    roundtrip(&IntangibleProjectile);

    let c = Recipes {
        recipes: vec!["minecraft:stick".into()],
    };
    roundtrip(&c);
    // list tag of strings
    assert_eq!(&encode(&c)[..2], [0x09, 0x08]);
}

#[test]
fn test_item_stack_with_components() {
    let item = ItemStack::from(ItemKind::LeatherHelmet)
        .with_component(DyedColor {
            rgb: 1,
            show_in_tooltip: true,
        })
        .with_component::<Unbreakable>(None);
    roundtrip(&item);
}

#[test]
fn test_default_components() {
    use azalea_inventory::components::{Food, MaxDamage, MaxStackSize};

    let sword = ItemStack::from(ItemKind::DiamondSword);
    assert_eq!(sword.get_component::<MaxDamage>().unwrap().amount, 1561);
    assert_eq!(sword.get_component::<MaxStackSize>().unwrap().count, 1);
    let apple = ItemStack::from(ItemKind::Apple);
    assert_eq!(apple.get_component::<Food>().unwrap().nutrition, 4);
    assert!(
        ItemStack::from(ItemKind::Stone)
            .get_component::<Food>()
            .is_none()
    );
}
