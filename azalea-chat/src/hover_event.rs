use serde::{Serialize, Serializer, ser::SerializeMap};
#[cfg(feature = "simdnbt")]
use simdnbt::owned::NbtCompound;

use crate::FormattedText;

/// The `hoverEvent` of a [`Style`](crate::style::Style).
///
/// In 1.21.4 this is sent as `{action: "...", contents: ...}`.
#[derive(Clone, Debug, PartialEq)]
pub enum HoverEvent {
    ShowText {
        value: Box<FormattedText>,
    },
    /// An item tooltip, vanilla 1.21.4 `HoverEvent.ItemStackInfo`.
    ShowItem(ItemStackInfo),
    ShowEntity {
        /// The entity type, like `minecraft:pig`.
        kind: String,
        /// The entity's UUID as a hyphenated string.
        id: String,
        name: Option<Box<FormattedText>>,
    },
}

/// The contents of a `show_item` hover event: vanilla 1.21.4
/// `HoverEvent.ItemStackInfo.CODEC`, which is `ItemStack.CODEC` (`id`, `count`
/// in 1..=99 defaulting to 1, optional `components`) with the bare item id
/// (`ItemStack.SIMPLE_ITEM_CODEC`) as the alternative when reading.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemStackInfo {
    /// The item id, like `minecraft:diamond_sword`.
    pub id: String,
    pub count: i32,
    /// The item's `DataComponentPatch`, kept as it was received (azalea-chat
    /// can't depend on azalea-inventory's typed components). `None` when the
    /// patch is empty, which vanilla leaves out.
    pub components: Option<ItemComponents>,
}

/// A `DataComponentPatch` as received: keys like `minecraft:custom_name` (or
/// `!minecraft:...` for a removed component), values in that component's
/// codec form.
#[derive(Clone, Debug, PartialEq)]
pub enum ItemComponents {
    Json(serde_json::Map<String, serde_json::Value>),
    #[cfg(feature = "simdnbt")]
    Nbt(NbtCompound),
}

impl ItemStackInfo {
    pub fn new(id: impl Into<String>, count: i32) -> Self {
        Self {
            id: id.into(),
            count,
            components: None,
        }
    }

    /// `ExtraCodecs.intRange(1, 99).fieldOf("count").orElse(1)`: missing or
    /// out of range reads as 1.
    fn count_or_default(n: Option<i64>) -> i32 {
        match n {
            Some(n) if (1..=99).contains(&n) => n as i32,
            _ => 1,
        }
    }

    /// Parse the JSON form: a compound, or a bare item id.
    pub fn deserialize(json: &serde_json::Value) -> Option<Self> {
        if let Some(id) = json.as_str() {
            return Some(Self::new(id, 1));
        }
        let o = json.as_object()?;
        let id = o.get("id")?.as_str()?.to_owned();
        let count = Self::count_or_default(o.get("count").and_then(|c| c.as_i64()));
        let components = match o.get("components") {
            Some(c) => {
                let c = c.as_object()?;
                (!c.is_empty()).then(|| ItemComponents::Json(c.clone()))
            }
            None => None,
        };
        Some(Self {
            id,
            count,
            components,
        })
    }

    fn components_json(&self) -> Option<serde_json::Map<String, serde_json::Value>> {
        match self.components.as_ref()? {
            ItemComponents::Json(m) => Some(m.clone()),
            #[cfg(feature = "simdnbt")]
            ItemComponents::Nbt(c) => Some(nbt_compound_to_json(c)),
        }
    }
}

impl Serialize for ItemStackInfo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("count", &self.count)?;
        if let Some(c) = self.components_json() {
            map.serialize_entry("components", &c)?;
        }
        map.end()
    }
}

/// NBT to JSON as vanilla's `NbtOps` to `JsonOps` conversion does it:
/// numbers stay numbers (bytes too, there's no boolean in NBT), arrays become
/// lists. Used only to write components received as NBT into a JSON
/// component.
#[cfg(feature = "simdnbt")]
fn nbt_to_json(tag: &simdnbt::owned::NbtTag) -> serde_json::Value {
    use serde_json::{Value, json};
    use simdnbt::owned::NbtTag;
    match tag {
        NbtTag::Byte(v) => json!(v),
        NbtTag::Short(v) => json!(v),
        NbtTag::Int(v) => json!(v),
        NbtTag::Long(v) => json!(v),
        NbtTag::Float(v) => json!(v),
        NbtTag::Double(v) => json!(v),
        NbtTag::ByteArray(v) => json!(v.iter().map(|b| *b as i8).collect::<Vec<_>>()),
        NbtTag::String(s) => json!(s.to_str()),
        NbtTag::IntArray(v) => json!(v),
        NbtTag::LongArray(v) => json!(v),
        NbtTag::Compound(c) => Value::Object(nbt_compound_to_json(c)),
        NbtTag::List(l) => nbt_list_to_json(l),
    }
}

#[cfg(feature = "simdnbt")]
fn nbt_compound_to_json(
    c: &simdnbt::owned::NbtCompound,
) -> serde_json::Map<String, serde_json::Value> {
    c.iter()
        .map(|(k, v)| (k.to_str().into_owned(), nbt_to_json(v)))
        .collect()
}

#[cfg(feature = "simdnbt")]
fn nbt_list_to_json(l: &simdnbt::owned::NbtList) -> serde_json::Value {
    use serde_json::{Value, json};
    use simdnbt::owned::NbtList;
    {
        Value::Array(match l {
            NbtList::Empty => vec![],
            NbtList::Byte(v) => v.iter().map(|x| json!(x)).collect(),
            NbtList::Short(v) => v.iter().map(|x| json!(x)).collect(),
            NbtList::Int(v) => v.iter().map(|x| json!(x)).collect(),
            NbtList::Long(v) => v.iter().map(|x| json!(x)).collect(),
            NbtList::Float(v) => v.iter().map(|x| json!(x)).collect(),
            NbtList::Double(v) => v.iter().map(|x| json!(x)).collect(),
            NbtList::ByteArray(v) => v
                .iter()
                .map(|x| json!(x.iter().map(|b| *b as i8).collect::<Vec<_>>()))
                .collect(),
            NbtList::String(v) => v.iter().map(|x| json!(x.to_str())).collect(),
            // without cloning each element into a tag first, which copied
            // every subtree once per level (quadratic in the depth)
            NbtList::List(v) => v.iter().map(nbt_list_to_json).collect(),
            NbtList::Compound(v) => v
                .iter()
                .map(|x| Value::Object(nbt_compound_to_json(x)))
                .collect(),
            NbtList::IntArray(v) => v.iter().map(|x| json!(x)).collect(),
            NbtList::LongArray(v) => v.iter().map(|x| json!(x)).collect(),
        })
    }
}

/// JSON to NBT, used only to write components received as JSON into an NBT
/// component. Vanilla would re-encode each component through its own codec,
/// which azalea-chat doesn't know, so this is a generic conversion: whole
/// numbers become ints (longs if they don't fit), other numbers doubles,
/// booleans bytes, and lists of mixed types lists of compounds with the value
/// under `""` (vanilla `NbtOps` list wrapping).
#[cfg(feature = "simdnbt")]
fn json_to_nbt(v: &serde_json::Value) -> simdnbt::owned::NbtTag {
    use serde_json::Value;
    use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
    match v {
        Value::Null => NbtTag::Compound(NbtCompound::new()),
        Value::Bool(b) => NbtTag::Byte(i8::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) if i32::try_from(i).is_ok() => NbtTag::Int(i as i32),
            Some(i) => NbtTag::Long(i),
            None => NbtTag::Double(n.as_f64().unwrap_or_default()),
        },
        Value::String(s) => NbtTag::String(s.as_str().into()),
        Value::Object(m) => {
            let mut c = NbtCompound::new();
            for (k, v) in m {
                c.insert(k.as_str(), json_to_nbt(v));
            }
            NbtTag::Compound(c)
        }
        Value::Array(a) => {
            let tags: Vec<NbtTag> = a.iter().map(json_to_nbt).collect();
            let list = match tags.first() {
                None => NbtList::Empty,
                Some(NbtTag::Int(_)) if tags.iter().all(|t| matches!(t, NbtTag::Int(_))) => {
                    NbtList::Int(tags.iter().filter_map(NbtTag::int).collect())
                }
                Some(NbtTag::String(_)) if tags.iter().all(|t| matches!(t, NbtTag::String(_))) => {
                    NbtList::String(
                        tags.into_iter()
                            .filter_map(|t| match t {
                                NbtTag::String(s) => Some(s),
                                _ => None,
                            })
                            .collect(),
                    )
                }
                Some(NbtTag::Compound(_))
                    if tags.iter().all(|t| matches!(t, NbtTag::Compound(_))) =>
                {
                    NbtList::Compound(
                        tags.into_iter()
                            .filter_map(|t| match t {
                                NbtTag::Compound(c) => Some(c),
                                _ => None,
                            })
                            .collect(),
                    )
                }
                _ => NbtList::Compound(
                    tags.into_iter()
                        .map(|t| {
                            let mut c = NbtCompound::new();
                            c.insert("", t);
                            c
                        })
                        .collect(),
                ),
            };
            NbtTag::List(list)
        }
    }
}

#[cfg(feature = "simdnbt")]
impl simdnbt::Serialize for ItemStackInfo {
    fn to_compound(self) -> NbtCompound {
        let mut c = NbtCompound::new();
        c.insert("id", self.id);
        c.insert("count", self.count);
        match self.components {
            Some(ItemComponents::Nbt(n)) => c.insert("components", n),
            Some(ItemComponents::Json(m)) => {
                c.insert("components", json_to_nbt(&serde_json::Value::Object(m)))
            }
            None => {}
        }
        c
    }
}

#[cfg(feature = "simdnbt")]
impl ItemStackInfo {
    /// Parse the NBT form: a compound, or a bare item id string.
    pub fn from_nbt_tag(tag: simdnbt::borrow::NbtTag) -> Option<Self> {
        if let Some(id) = tag.string() {
            return Some(Self::new(id.to_str().into_owned(), 1));
        }
        let c = tag.compound()?;
        let id = c.string("id")?.to_str().into_owned();
        // NbtOps reads any numeric tag as a number
        let count = c.get("count").and_then(|t| {
            t.int()
                .map(i64::from)
                .or_else(|| t.byte().map(i64::from))
                .or_else(|| t.short().map(i64::from))
                .or_else(|| t.long())
        });
        let count = Self::count_or_default(count);
        let components = match c.get("components") {
            Some(t) => {
                let comp = t.compound()?.to_owned();
                (!comp.is_empty()).then_some(ItemComponents::Nbt(comp))
            }
            None => None,
        };
        Some(Self {
            id,
            count,
            components,
        })
    }
}

impl HoverEvent {
    /// The name of the action, like `show_text`.
    pub fn action(&self) -> &'static str {
        match self {
            HoverEvent::ShowText { .. } => "show_text",
            HoverEvent::ShowItem { .. } => "show_item",
            HoverEvent::ShowEntity { .. } => "show_entity",
        }
    }

    /// Parse a hover event from JSON. `show_text` and `show_item` are
    /// supported.
    pub fn deserialize(json: &serde_json::Value) -> Option<Self> {
        let contents = json.get("contents")?;
        match json.get("action")?.as_str()? {
            "show_text" => {
                let value = serde_json::from_value(contents.clone()).ok()?;
                Some(HoverEvent::ShowText {
                    value: Box::new(value),
                })
            }
            "show_item" => Some(HoverEvent::ShowItem(ItemStackInfo::deserialize(contents)?)),
            _ => None,
        }
    }
}

#[derive(Serialize)]
struct EntityContents<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a FormattedText>,
}

impl Serialize for HoverEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("action", self.action())?;
        match self {
            HoverEvent::ShowText { value } => map.serialize_entry("contents", value)?,
            HoverEvent::ShowItem(item) => map.serialize_entry("contents", item)?,
            HoverEvent::ShowEntity { kind, id, name } => map.serialize_entry(
                "contents",
                &EntityContents {
                    kind,
                    id,
                    name: name.as_deref(),
                },
            )?,
        }
        map.end()
    }
}

/// A UUID as vanilla's `UUIDUtil.CODEC` writes it: four big-endian ints.
#[cfg(feature = "simdnbt")]
fn uuid_to_int_array(id: &str) -> Option<Vec<i32>> {
    let hex: String = id.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    let n = u128::from_str_radix(&hex, 16).ok()?;
    Some((0..4).map(|i| (n >> (96 - 32 * i)) as u32 as i32).collect())
}

#[cfg(feature = "simdnbt")]
fn int_array_to_uuid(ints: &[i32]) -> Option<String> {
    let [a, b, c, d] = ints else { return None };
    let n = (u128::from(*a as u32) << 96)
        | (u128::from(*b as u32) << 64)
        | (u128::from(*c as u32) << 32)
        | u128::from(*d as u32);
    let h = format!("{n:032x}");
    Some(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}

#[cfg(feature = "simdnbt")]
impl simdnbt::Serialize for HoverEvent {
    fn to_compound(self) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.insert("action", self.action());
        match self {
            HoverEvent::ShowText { value } => {
                compound.insert("contents", value.to_vanilla_nbt_tag());
            }
            HoverEvent::ShowItem(item) => {
                compound.insert("contents", simdnbt::Serialize::to_compound(item));
            }
            HoverEvent::ShowEntity { kind, id, name } => {
                // vanilla 1.21.4 HoverEvent.EntityTooltipInfo.CODEC: type, id
                // (UUIDUtil.LENIENT_CODEC, written as an int array), name
                let mut contents = NbtCompound::new();
                contents.insert("type", kind);
                match uuid_to_int_array(&id) {
                    Some(ints) => contents.insert("id", simdnbt::owned::NbtTag::IntArray(ints)),
                    None => contents.insert("id", id),
                }
                if let Some(name) = name {
                    contents.insert("name", name.to_vanilla_nbt_tag());
                }
                compound.insert("contents", contents);
            }
        }
        compound
    }
}

#[cfg(feature = "simdnbt")]
impl simdnbt::Deserialize for HoverEvent {
    fn from_compound(
        compound: simdnbt::borrow::NbtCompound,
    ) -> Result<Self, simdnbt::DeserializeError> {
        use simdnbt::{DeserializeError, FromNbtTag};

        let action = crate::get_in_compound::<String>(&compound, "action")?;
        let contents = compound
            .get("contents")
            .ok_or(DeserializeError::MissingField)?;
        match action.as_str() {
            "show_text" => {
                let value =
                    FormattedText::from_nbt_tag(contents).ok_or(DeserializeError::MissingField)?;
                Ok(HoverEvent::ShowText {
                    value: Box::new(value),
                })
            }
            "show_item" => Ok(HoverEvent::ShowItem(
                ItemStackInfo::from_nbt_tag(contents).ok_or(DeserializeError::MissingField)?,
            )),
            "show_entity" => {
                let contents = contents.compound().ok_or(DeserializeError::MissingField)?;
                let kind = crate::get_in_compound::<String>(&contents, "type")?;
                let id_tag = contents.get("id").ok_or(DeserializeError::MissingField)?;
                // UUIDUtil.LENIENT_CODEC: an int array, or a hyphenated string
                let id = match id_tag.int_array() {
                    Some(ints) => int_array_to_uuid(&ints).ok_or(DeserializeError::MissingField)?,
                    None => id_tag
                        .string()
                        .ok_or(DeserializeError::MissingField)?
                        .to_string(),
                };
                let name = match contents.get("name") {
                    Some(tag) => Some(Box::new(
                        FormattedText::from_nbt_tag(tag).ok_or(DeserializeError::MissingField)?,
                    )),
                    None => None,
                };
                Ok(HoverEvent::ShowEntity { kind, id, name })
            }
            _ => Err(DeserializeError::MismatchedFieldType(action)),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_show_item_round_trips() {
        let j = json!({
            "text": "[Excalibur]",
            "hoverEvent": {
                "action": "show_item",
                "contents": {
                    "id": "minecraft:diamond_sword",
                    "count": 1,
                    "components": {
                        "minecraft:custom_name": "\"Excalibur\"",
                        "minecraft:damage": 5
                    }
                }
            }
        });
        let c: FormattedText = serde_json::from_value(j.clone()).unwrap();
        let FormattedText::Text(t) = &c else {
            panic!("{c:?}")
        };
        let Some(HoverEvent::ShowItem(item)) = &t.base.style.hover_event else {
            panic!("hover dropped: {c:?}")
        };
        assert_eq!(item.id, "minecraft:diamond_sword");
        assert_eq!(item.count, 1);
        assert!(matches!(&item.components, Some(ItemComponents::Json(m)) if m.len() == 2));
        assert_eq!(serde_json::to_value(&c).unwrap(), j);
    }

    #[test]
    fn json_show_item_simple_and_default_count() {
        // ItemStack.SIMPLE_ITEM_CODEC: a bare item id
        let simple = json!({"action": "show_item", "contents": "minecraft:stone"});
        assert_eq!(
            HoverEvent::deserialize(&simple),
            Some(HoverEvent::ShowItem(ItemStackInfo::new(
                "minecraft:stone",
                1
            )))
        );
        // count: intRange(1, 99).orElse(1)
        for count in [json!(null), json!(0), json!(100)] {
            let mut contents = json!({"id": "minecraft:stone"});
            if !count.is_null() {
                contents["count"] = count;
            }
            let h = json!({"action": "show_item", "contents": contents});
            assert_eq!(
                HoverEvent::deserialize(&h),
                Some(HoverEvent::ShowItem(ItemStackInfo::new(
                    "minecraft:stone",
                    1
                )))
            );
        }
        // vanilla always writes count, and leaves out an empty patch
        assert_eq!(
            serde_json::to_value(HoverEvent::ShowItem(ItemStackInfo::new(
                "minecraft:stone",
                1
            )))
            .unwrap(),
            json!({"action": "show_item", "contents": {"id": "minecraft:stone", "count": 1}})
        );
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    #[test]
    fn nbt_show_item_keeps_contents() {
        use std::io::Cursor;

        use azalea_buf::AzBuf;
        use simdnbt::owned::{NbtCompound, NbtTag};

        // the hover event as Paper 1.21.4 writes it through
        // HoverEvent.ItemStackInfo.CODEC
        let mut components = NbtCompound::new();
        components.insert("minecraft:custom_name", "\"Excalibur\"");
        components.insert("minecraft:damage", NbtTag::Int(5));
        components.insert("!minecraft:attribute_modifiers", NbtCompound::new());
        let mut contents = NbtCompound::new();
        contents.insert("id", "minecraft:diamond_sword");
        contents.insert("count", NbtTag::Int(1));
        contents.insert("components", components.clone());
        let mut hover = NbtCompound::new();
        hover.insert("action", "show_item");
        hover.insert("contents", contents);
        let mut root = NbtCompound::new();
        root.insert("text", "[Excalibur]");
        root.insert("hoverEvent", hover);
        let mut wire = vec![10];
        root.write(&mut wire);

        let c = FormattedText::azalea_read(&mut Cursor::new(&wire[..])).unwrap();
        let FormattedText::Text(t) = &c else {
            panic!("{c:?}")
        };
        let Some(HoverEvent::ShowItem(item)) = &t.base.style.hover_event else {
            panic!("hover dropped: {c:?}")
        };
        assert_eq!(item.id, "minecraft:diamond_sword");
        assert_eq!(item.count, 1);
        assert_eq!(item.components, Some(ItemComponents::Nbt(components)));

        // re-encoded, it reads back the same, and the same keys and value
        // bytes go out (key order aside)
        let mut out = Vec::new();
        c.azalea_write(&mut out).unwrap();
        assert_eq!(out.len(), wire.len());
        let back = FormattedText::azalea_read(&mut Cursor::new(&out[..])).unwrap();
        assert_eq!(back, c);
    }

    #[cfg(feature = "simdnbt")]
    #[test]
    fn nbt_components_to_json_is_linear() {
        use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                // 500 nested lists over 100 000 ints: converting cloned each
                // level's whole subtree
                let mut list = NbtList::Int((0..100_000).collect());
                for _ in 0..500 {
                    list = NbtList::List(vec![list]);
                }
                let mut components = NbtCompound::new();
                components.insert("minecraft:custom_data", NbtTag::List(list));
                let info = ItemStackInfo {
                    id: "minecraft:stone".to_owned(),
                    count: 1,
                    components: Some(ItemComponents::Nbt(components)),
                };
                let start = std::time::Instant::now();
                let json = serde_json::to_string(&info).unwrap();
                let elapsed = start.elapsed();
                assert!(json.contains("99999"));
                assert!(elapsed < std::time::Duration::from_secs(1), "{elapsed:?}");
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[cfg(all(feature = "azalea-buf", feature = "simdnbt"))]
    #[test]
    fn nbt_show_item_simple_form() {
        use std::io::Cursor;

        use azalea_buf::AzBuf;
        use simdnbt::owned::NbtCompound;

        let mut hover = NbtCompound::new();
        hover.insert("action", "show_item");
        hover.insert("contents", "minecraft:stone");
        let mut root = NbtCompound::new();
        root.insert("text", "x");
        root.insert("hoverEvent", hover);
        let mut wire = vec![10];
        root.write(&mut wire);
        let c = FormattedText::azalea_read(&mut Cursor::new(&wire[..])).unwrap();
        let FormattedText::Text(t) = &c else {
            panic!("{c:?}")
        };
        assert_eq!(
            t.base.style.hover_event,
            Some(HoverEvent::ShowItem(ItemStackInfo::new(
                "minecraft:stone",
                1
            )))
        );
    }
}
