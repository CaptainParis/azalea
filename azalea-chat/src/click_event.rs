use std::fmt::Display;

use serde::{Serialize, Serializer};
#[cfg(feature = "simdnbt")]
use simdnbt::{DeserializeError, owned::NbtCompound};

#[cfg(feature = "simdnbt")]
use crate::get_in_compound;

/// Serializes a value as a string, since every click event value is a string
/// in 1.21.4 (even page numbers).
fn serialize_as_string<T: Display, S: Serializer>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

macro_rules! define_click_event_struct {
    (
        $(
            $action_name:ident : $action_variant:ident { $field:ident : $type:ty }
        ),*
        $(,)?
    ) => {
        /// The `clickEvent` of a [`Style`](crate::style::Style).
        ///
        /// In 1.21.4 every action is sent as `{action: "...", value: "..."}`.
        #[derive(Clone, Debug, PartialEq, Serialize)]
        #[serde(rename_all = "snake_case", tag = "action")]
        pub enum ClickEvent {
            $(
                $action_variant {
                    #[serde(rename = "value", serialize_with = "serialize_as_string")]
                    $field: $type
                }
            ),*
        }

        impl ClickEvent {
            /// The name of the action, like `open_url`.
            pub fn action(&self) -> &'static str {
                match self {
                    $( Self::$action_variant { .. } => stringify!($action_name), )*
                }
            }

            /// The value of the event as it's sent over the network.
            pub fn value(&self) -> String {
                match self {
                    $( Self::$action_variant { $field } => $field.to_string(), )*
                }
            }

            /// Parse a click event from its action name and value.
            ///
            /// Returns `None` if the action is unknown or the value is invalid
            /// for it.
            pub fn from_action_value(action: &str, value: &str) -> Option<Self> {
                Some(match action {
                    $(
                        stringify!($action_name) => Self::$action_variant {
                            $field: value.parse().ok()?
                        },
                    )*
                    _ => return None,
                })
            }
        }
    }
}

define_click_event_struct! {
    open_url: OpenUrl { url: String },
    open_file: OpenFile { path: String },
    run_command: RunCommand { command: String },
    suggest_command: SuggestCommand { command: String },
    change_page: ChangePage { page: i32 },
    copy_to_clipboard: CopyToClipboard { value: String },
}

impl ClickEvent {
    pub fn deserialize(json: &serde_json::Value) -> Option<Self> {
        let action = json.get("action")?.as_str()?;
        let value = json.get("value")?.as_str()?;
        Self::from_action_value(action, value)
    }
}

#[cfg(feature = "simdnbt")]
impl simdnbt::Serialize for ClickEvent {
    fn to_compound(self) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.insert("action", self.action());
        compound.insert("value", self.value());
        compound
    }
}

#[cfg(feature = "simdnbt")]
impl simdnbt::Deserialize for ClickEvent {
    fn from_compound(compound: simdnbt::borrow::NbtCompound) -> Result<Self, DeserializeError> {
        let action = get_in_compound::<String>(&compound, "action")?;
        let value = get_in_compound::<String>(&compound, "value")?;
        Self::from_action_value(&action, &value)
            .ok_or(DeserializeError::MismatchedFieldType(action))
    }
}
