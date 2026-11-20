use azalea_auth::game_profile::{GameProfileProperties, SerializableProfileProperties};
use azalea_buf::AzBuf;
use azalea_core::codec_utils::*;
use serde::{Serialize, Serializer};
use uuid::Uuid;

/// A player profile that may be partial (the server fills in the rest).
#[derive(AzBuf, Clone, Debug, Default, PartialEq, Serialize)]
#[doc(alias = "ResolvableProfile")]
pub struct Profile {
    #[limit(16)]
    #[serde(skip_serializing_if = "is_default")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    pub id: Option<Uuid>,
    #[serde(serialize_with = "serialize_properties")]
    #[serde(skip_serializing_if = "is_default")]
    pub properties: GameProfileProperties,
}
fn serialize_properties<S: Serializer>(
    properties: &GameProfileProperties,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let serializable = SerializableProfileProperties::from(properties.clone());
    serializable.serialize(serializer)
}
