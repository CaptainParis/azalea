//! The data sent to the client in the `ClientboundRegistryDataPacket`.
//!
//! This module contains the structures used to represent the registry
//! sent to the client upon login. This contains a lot of information about
//! the game, including the types of chat messages, dimensions, and
//! biomes.

pub mod block_predicate;
pub mod block_state_provider;
pub mod components;
pub mod dimension_type;
pub mod enchantment;
pub mod entity_effect;
pub mod float_provider;
pub mod value;

use std::{collections::HashMap, io::Cursor};

use azalea_registry::identifier::Identifier;
use indexmap::IndexMap;
use simdnbt::{DeserializeError, FromNbtTag, borrow, owned::NbtCompound};
use thiserror::Error;
use tracing::error;

use crate::decode_errors::{DecodeErrorKind, record_decode_error};

/// The base of the registry.
///
/// This is the registry that is sent to the client upon login.
///
/// Note that `azalea-client` stores registries per-world rather than
/// per-client like you might expect. This is an optimization for swarms to
/// reduce memory usage, since registries are expected to be the same for every
/// client in a world.
#[derive(Clone, Debug, Default)]
pub struct RegistryHolder {
    // if you add new fields here, don't forget to also update
    // `RegistryHolder::append`, `protocol_id_to_identifier`, and
    // `define_default_deserializes_to!` in `data_registry.rs`.
    #[rustfmt::skip] // allow empty line

    /// Attributes about the dimension.
    pub dimension_type: RegistryType<dimension_type::DimensionKindElement>,

    pub enchantment: RegistryType<enchantment::EnchantmentData>,

    /// Registries that we haven't implemented deserializable types for.
    ///
    /// You can still access these just fine, but they'll be NBT instead of
    /// nicer structs.
    pub extra: HashMap<Identifier, RegistryType<NbtCompound>>,
}

macro_rules! registry_holder {
    ($($registry:ident),* $(,)?) => {
        impl RegistryHolder {
            pub fn append(
                &mut self,
                id: Identifier,
                entries: Vec<(Identifier, Option<NbtCompound>)>,
            ) {

                if id.namespace() == "minecraft" {
                    match id.path() {
                        $(
                            stringify!($registry) => {
                                return self.$registry.append_nbt(id, entries);
                            }
                        )*
                        _ => {}
                    }
                }

                self.extra
                    .entry(id.clone())
                    .or_default()
                    .append_nbt(id, entries);
            }

            pub fn extend(&mut self, other: RegistryHolder) {
                $(
                    self.$registry = other.$registry;
                )*
                self.extra.extend(other.extra);
            }

            /// Convert a protocol ID for a registry key (like the protocol_id for
            /// something that implements `DataRegistry`) and convert it to its string
            /// name.
            pub fn protocol_id_to_identifier(
                &self,
                registry: Identifier,
                protocol_id: u32,
            ) -> Option<&Identifier> {
                let index = protocol_id as usize;

                if registry.namespace() == "minecraft" {
                    match registry.path() {
                        $(
                            stringify!($registry) => {
                                return self.$registry.map.get_index(index).map(|(k, _)| k);
                            }
                        )*
                        _ => {}
                    }
                }

                self.extra
                    .get(&registry)
                    .and_then(|r| r.map.get_index(index))
                    .map(|(k, _)| k)
            }
        }
    };
}

registry_holder!(dimension_type, enchantment);

fn nbt_to_serializable_type<T: simdnbt::Deserialize>(
    value: &NbtCompound,
) -> Result<T, NbtToSerializableTypeError> {
    // convert the value to T
    let mut nbt_bytes = Vec::new();
    value.write(&mut nbt_bytes);
    let nbt_borrow_compound = simdnbt::borrow::read_compound(&mut Cursor::new(&nbt_bytes))?;
    T::from_compound((&nbt_borrow_compound).into()).map_err(Into::into)
}

#[derive(Debug, Error)]
enum NbtToSerializableTypeError {
    #[error(transparent)]
    NbtError(#[from] simdnbt::Error),
    #[error(transparent)]
    DeserializeError(#[from] simdnbt::DeserializeError),
}

/// A collection of values for a certain type of registry data.
///
/// The protocol id of an entry is its index in `map`, so every entry the
/// server sent has a place here, in the order it was sent. An entry whose
/// data couldn't be parsed is kept as `None` (a placeholder, counted as a
/// [`DecodeErrorKind::Registry`] decode error): leaving it out would give
/// every later entry the protocol id of the one before it.
#[derive(Clone, Debug)]
pub struct RegistryType<T: simdnbt::Deserialize> {
    pub map: IndexMap<Identifier, Option<T>>,
}

impl<T: simdnbt::Deserialize> RegistryType<T> {
    /// The parsed entry with this key, or `None` if there's no such entry or
    /// it's a placeholder for data that couldn't be parsed.
    pub fn get(&self, key: &Identifier) -> Option<&T> {
        self.map.get(key).and_then(Option::as_ref)
    }

    /// The key and parsed entry at this protocol id, or `None` if there's no
    /// such entry or it's a placeholder for data that couldn't be parsed.
    pub fn get_index(&self, protocol_id: usize) -> Option<(&Identifier, &T)> {
        self.map
            .get_index(protocol_id)
            .and_then(|(k, v)| v.as_ref().map(|v| (k, v)))
    }
}

impl<T: simdnbt::Deserialize> Default for RegistryType<T> {
    fn default() -> Self {
        Self {
            map: IndexMap::new(),
        }
    }
}

/// Bounds a registry entry must be within before it's used, as vanilla's
/// codec for it checks them. An entry outside them is kept (so later
/// entries keep their protocol ids) but never resolved by
/// [`RegistryDeserializesTo::get_for_registry`].
pub trait RegistryEntryBounds {
    /// Why the entry is outside its bounds, or `None` if it's within them.
    fn out_of_bounds(&self) -> Option<String> {
        None
    }
    /// The decode error counter kind for an entry outside its bounds.
    fn out_of_bounds_kind() -> DecodeErrorKind {
        DecodeErrorKind::Registry
    }
}
impl RegistryEntryBounds for NbtCompound {}
impl RegistryEntryBounds for enchantment::EnchantmentData {}

impl<T: simdnbt::Deserialize + RegistryEntryBounds> RegistryType<T> {
    fn append_nbt(&mut self, id: Identifier, entries: Vec<(Identifier, Option<NbtCompound>)>) {
        let map = &mut self.map;
        for (key, value) in entries {
            if let Some(value) = value {
                match nbt_to_serializable_type::<T>(&value) {
                    Ok(value) => {
                        if let Some(reason) = value.out_of_bounds() {
                            error!("Rejecting {id} entry {key}: {reason}");
                            // decode error hook: the entry is kept for its
                            // protocol id but never resolved
                            record_decode_error(T::out_of_bounds_kind());
                        }
                        map.insert(key, Some(value));
                    }
                    Err(err) => {
                        error!("Error deserializing {id} entry {key}: {err:?}\n{value:?}");
                        // decode error hook: an entry that fails to parse is
                        // kept as a placeholder, so the entries after it keep
                        // their protocol ids
                        record_decode_error(DecodeErrorKind::Registry);
                        map.insert(key, None);
                    }
                }
            } else {
                map.shift_remove(&key);
            }
        }
    }
}

pub trait RegistryDeserializesTo: simdnbt::Deserialize {
    fn get_for_registry<'a>(
        registries: &'a RegistryHolder,
        registry_name: &'static str,
        protocol_id: u32,
    ) -> Option<(&'a Identifier, &'a Self)>;
}

impl RegistryDeserializesTo for NbtCompound {
    fn get_for_registry<'a>(
        registries: &'a RegistryHolder,
        registry_name: &'static str,
        protocol_id: u32,
    ) -> Option<(&'a Identifier, &'a Self)> {
        registries
            .extra
            .get(&Identifier::new(registry_name))?
            .get_index(protocol_id as usize)
    }
}
impl RegistryDeserializesTo for dimension_type::DimensionKindElement {
    fn get_for_registry<'a>(
        registries: &'a RegistryHolder,
        registry_name: &'static str,
        protocol_id: u32,
    ) -> Option<(&'a Identifier, &'a Self)> {
        // it would return an entry from the wrong registry
        debug_assert!(
            registry_name == "dimension_type",
            "called RegistryDeserializesTo::get_for_registry with the wrong registry: {registry_name}"
        );
        registries
            .dimension_type
            .get_index(protocol_id as usize)
            // a dimension type outside vanilla's bounds was counted when it
            // was received, and nothing may be sized from it
            .filter(|(_, d)| d.out_of_bounds().is_none())
    }
}
impl RegistryDeserializesTo for enchantment::EnchantmentData {
    fn get_for_registry<'a>(
        registries: &'a RegistryHolder,
        registry_name: &'static str,
        protocol_id: u32,
    ) -> Option<(&'a Identifier, &'a Self)> {
        // it would return an entry from the wrong registry
        debug_assert!(
            registry_name == "enchantment",
            "called RegistryDeserializesTo::get_for_registry with the wrong registry: {registry_name}"
        );
        registries.enchantment.get_index(protocol_id as usize)
    }
}

pub fn get_in_compound<T: FromNbtTag>(
    compound: &borrow::NbtCompound,
    key: &str,
) -> Result<T, DeserializeError> {
    let value = compound.get(key).ok_or(DeserializeError::MissingField)?;
    T::from_nbt_tag(value).ok_or(DeserializeError::MissingField)
}

#[cfg(test)]
mod tests {
    use simdnbt::owned::NbtCompound;

    use super::*;
    use crate::decode_errors::decode_errors;

    fn dimension(height: i32, min_y: i32, logical_height: i32) -> NbtCompound {
        let mut c = NbtCompound::new();
        c.insert("height", height);
        c.insert("min_y", min_y);
        c.insert("logical_height", logical_height);
        c
    }

    // strict registries need every dimension field
    #[cfg(not(feature = "strict_registry"))]
    #[test]
    fn out_of_bounds_dimension_type_is_counted_and_never_resolved() {
        let mut registries = RegistryHolder::default();
        let before = decode_errors();
        registries.append(
            Identifier::new("minecraft:dimension_type"),
            vec![
                (Identifier::new("a:ok"), Some(dimension(384, -64, 384))),
                // 4080 is over the 4064 maximum
                (Identifier::new("a:tall"), Some(dimension(4080, -2032, 0))),
                (Identifier::new("a:also_ok"), Some(dimension(256, 0, 128))),
            ],
        );
        assert!(decode_errors().dimension_type > before.dimension_type);
        let get = |id| {
            <dimension_type::DimensionKindElement as RegistryDeserializesTo>::get_for_registry(
                &registries,
                "dimension_type",
                id,
            )
            .map(|(k, _)| k.to_string())
        };
        assert_eq!(get(0).as_deref(), Some("a:ok"));
        assert_eq!(get(1), None);
        // the entry after the rejected one keeps its protocol id
        assert_eq!(get(2).as_deref(), Some("a:also_ok"));
    }

    // a dimension type without `min_y` fails to parse in either mode
    #[cfg(not(feature = "strict_registry"))]
    #[test]
    fn unparsable_entry_keeps_a_placeholder_so_later_ids_stay_aligned() {
        let mut broken = NbtCompound::new();
        broken.insert("height", 384);
        let mut registries = RegistryHolder::default();
        let before = decode_errors();
        registries.append(
            Identifier::new("minecraft:dimension_type"),
            vec![
                (Identifier::new("a:ok"), Some(dimension(384, -64, 384))),
                (Identifier::new("a:broken"), Some(broken)),
                (Identifier::new("a:also_ok"), Some(dimension(256, 0, 128))),
            ],
        );
        assert!(decode_errors().registry > before.registry);
        assert_eq!(registries.dimension_type.map.len(), 3);
        let get = |id| {
            <dimension_type::DimensionKindElement as RegistryDeserializesTo>::get_for_registry(
                &registries,
                "dimension_type",
                id,
            )
            .map(|(k, d)| (k.to_string(), d.height))
        };
        assert_eq!(get(0), Some(("a:ok".to_string(), 384)));
        // the placeholder has its id (and key) but never resolves
        assert_eq!(get(1), None);
        assert_eq!(
            registries
                .protocol_id_to_identifier(Identifier::new("minecraft:dimension_type"), 1)
                .map(|k| k.to_string())
                .as_deref(),
            Some("a:broken")
        );
        // without the placeholder this would be None and id 1 would be
        // a:also_ok
        assert_eq!(get(2), Some(("a:also_ok".to_string(), 256)));
    }
}
