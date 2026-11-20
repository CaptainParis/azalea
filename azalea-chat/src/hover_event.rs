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
    // TODO
    ShowItem {
        // item: ItemStack,
    },
    ShowEntity {
        /// The entity type, like `minecraft:pig`.
        kind: String,
        /// The entity's UUID as a hyphenated string.
        id: String,
        name: Option<Box<FormattedText>>,
    },
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

    /// Parse a hover event from JSON. Only `show_text` is supported.
    pub fn deserialize(json: &serde_json::Value) -> Option<Self> {
        if json.get("action")?.as_str()? != "show_text" {
            return None;
        }
        let value = serde_json::from_value(json.get("contents")?.clone()).ok()?;
        Some(HoverEvent::ShowText {
            value: Box::new(value),
        })
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
            HoverEvent::ShowItem {} => {}
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
            HoverEvent::ShowItem { .. } => {}
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
