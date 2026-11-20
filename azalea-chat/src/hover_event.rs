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

#[cfg(feature = "simdnbt")]
impl simdnbt::Serialize for HoverEvent {
    fn to_compound(self) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.insert("action", self.action());
        match self {
            HoverEvent::ShowText { value } => {
                compound.insert("contents", value.to_compound());
            }
            HoverEvent::ShowItem { .. } => {}
            HoverEvent::ShowEntity { kind, id, name } => {
                let mut contents = NbtCompound::new();
                contents.insert("type", kind);
                contents.insert("id", id);
                if let Some(name) = name {
                    contents.insert("name", name.to_compound());
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
        if action != "show_text" {
            return Err(DeserializeError::MismatchedFieldType(action));
        }
        let contents = compound
            .get("contents")
            .ok_or(DeserializeError::MissingField)?;
        let value = FormattedText::from_nbt_tag(contents).ok_or(DeserializeError::MissingField)?;
        Ok(HoverEvent::ShowText {
            value: Box::new(value),
        })
    }
}
