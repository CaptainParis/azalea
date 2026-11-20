mod profile;

use core::f64;
use std::{
    any::Any,
    fmt::{self, Debug, Display},
    io::{self, Cursor, Write},
    mem::ManuallyDrop,
};

use azalea_buf::{AzBuf, AzBufVar, BufReadError};
use azalea_chat::FormattedText;
use azalea_core::{
    attribute_modifier_operation::AttributeModifierOperation, codec_utils::*,
    filterable::Filterable, position::GlobalPos, sound::CustomSound,
};
use azalea_registry::{
    Holder, HolderSet,
    builtin::{
        Attribute, BlockKind, DataComponentKind, EntityKind, ItemKind, MobEffect, Potion,
        SoundEvent,
    },
    data::{BannerPatternKind, Enchantment, JukeboxSong, TrimMaterial, TrimPattern},
    identifier::Identifier,
};
use indexmap::IndexMap;
pub use profile::*;
use serde::{Serialize, ser::SerializeMap};
use simdnbt::owned::{Nbt, NbtCompound, NbtList, NbtTag};
use tracing::trace;

use crate::{ItemStack, item::consume_effect::ConsumeEffect};

pub trait DataComponentTrait:
    Send + Sync + Any + Clone + Serialize + Into<DataComponentUnion>
{
    const KIND: DataComponentKind;
}

pub trait EncodableDataComponent: Send + Sync + Any + Debug {
    fn encode(&self, buf: &mut Vec<u8>) -> io::Result<()>;
    // using the Clone trait makes it not be object-safe, so we have our own
    // clone function instead
    fn clone(&self) -> Box<dyn EncodableDataComponent>;
    // same thing here
    fn eq(&self, other: &dyn EncodableDataComponent) -> bool;
}

impl<T> EncodableDataComponent for T
where
    T: DataComponentTrait + Clone + AzBuf + PartialEq + Debug,
{
    fn encode(&self, buf: &mut Vec<u8>) -> io::Result<()> {
        self.azalea_write(buf)
    }
    fn clone(&self) -> Box<dyn EncodableDataComponent> {
        let cloned = self.clone();
        Box::new(cloned)
    }
    fn eq(&self, other: &dyn EncodableDataComponent) -> bool {
        let other_any: &dyn Any = other;
        match other_any.downcast_ref::<T>() {
            Some(other) => self == other,
            _ => false,
        }
    }
}

/// Read one component and wrap it in the union, in a function of its own:
/// generated `match`es over every kind would otherwise hold a stack slot for
/// each kind's value at once (in debug builds), and nested item stacks recurse
/// through them.
#[inline(never)]
fn read_component_as<T: AzBuf>(
    buf: &mut Cursor<&[u8]>,
    wrap: impl FnOnce(T) -> DataComponentUnion,
) -> Result<DataComponentUnion, BufReadError> {
    Ok(wrap(T::azalea_read(buf)?))
}

/// See [`read_component_as`].
#[inline(never)]
fn clone_component_as<T: Clone>(
    value: &ManuallyDrop<T>,
    wrap: impl FnOnce(T) -> DataComponentUnion,
) -> DataComponentUnion {
    wrap(T::clone(value))
}

#[macro_export]
macro_rules! define_data_components {
    ( $( $x:ident ),* $(,)? ) => {
        /// A union of all data components.
        ///
        /// You probably don't want to use this directly. Consider [`DataComponentPatch`] instead.
        ///
        /// This type does not know its own value, and as such every function for it requires the
        /// `DataComponentKind` to be passed in. Passing the wrong `DataComponentKind` will result
        /// in undefined behavior. Also, all of the values are `ManuallyDrop`.
        ///
        /// [`DataComponentPatch`]: crate::DataComponentPatch
        #[allow(non_snake_case)]
        pub union DataComponentUnion {
            $( $x: ManuallyDrop<$x>, )*
        }
        impl DataComponentUnion {
            /// # Safety
            ///
            /// `kind` must be the correct value for this union.
            pub unsafe fn serialize_entry_as<S: SerializeMap>(
                &self,
                serializer: &mut S,
                kind: DataComponentKind,
            ) -> Result<(), S::Error> {
                match kind {
                    $( DataComponentKind::$x => { unsafe { serializer.serialize_entry(&kind, &*self.$x) } }, )*
                }
            }
            /// # Safety
            ///
            /// `kind` must be the correct value for this union.
            pub unsafe fn drop_as(&mut self, kind: DataComponentKind) {
                match kind {
                    $( DataComponentKind::$x => { unsafe { ManuallyDrop::drop(&mut self.$x) } }, )*
                }
            }
            /// # Safety
            ///
            /// `kind` must be the correct value for this union.
            pub unsafe fn as_kind(&self, kind: DataComponentKind) -> &dyn EncodableDataComponent {
                match kind {
                    $( DataComponentKind::$x => { unsafe { &**(&self.$x as &ManuallyDrop<dyn EncodableDataComponent>) } }, )*
                }
            }
            pub fn azalea_read_as(
                kind: DataComponentKind,
                buf: &mut Cursor<&[u8]>,
            ) -> Result<Self, BufReadError> {
                trace!("Reading data component {kind}");

                // each kind is read in its own function, so this match's
                // stack frame stays small (item stacks nest through it)
                match kind {
                    $( DataComponentKind::$x => {
                        read_component_as::<$x>(buf, |v| Self { $x: ManuallyDrop::new(v) })
                    }, )*
                }
            }
            /// # Safety
            ///
            /// `kind` must be the correct value for this union.
            pub unsafe fn azalea_write_as(
                &self,
                kind: DataComponentKind,
                buf: &mut impl std::io::Write,
            ) -> io::Result<()> {
                let mut value = Vec::new();
                match kind {
                    $( DataComponentKind::$x => unsafe { self.$x.encode(&mut value)? }, )*
                };
                buf.write_all(&value)?;

                Ok(())
            }
            /// # Safety
            ///
            /// `kind` must be the correct value for this union.
            pub unsafe fn clone_as(
                &self,
                kind: DataComponentKind,
            ) -> Self {
                match kind {
                    $( DataComponentKind::$x => {
                        clone_component_as::<$x>(unsafe { &self.$x }, |v| Self { $x: ManuallyDrop::new(v) })
                    }, )*
                }
            }
            /// # Safety
            ///
            /// `kind` must be the correct value for this union.
            pub unsafe fn eq_as(
                &self,
                other: &Self,
                kind: DataComponentKind,
            ) -> bool {
                match kind {
                    $( DataComponentKind::$x => unsafe { self.$x.eq(&other.$x) }, )*
                }
            }
        }
        $(
            impl From<$x> for DataComponentUnion {
                fn from(value: $x) -> Self {
                    DataComponentUnion { $x: ManuallyDrop::new(value) }
                }
            }
        )*

        $(
            impl DataComponentTrait for $x {
                const KIND: DataComponentKind = DataComponentKind::$x;
            }
        )*
    };
}

// if this is causing a compile-time error, look at DataComponents.java in the
// decompiled vanilla code to see how to implement new components

// note that this statement is updated by genitemcomponents.py
define_data_components!(
    CustomData,
    MaxStackSize,
    MaxDamage,
    Damage,
    Unbreakable,
    CustomName,
    ItemName,
    ItemModel,
    Lore,
    Rarity,
    Enchantments,
    CanPlaceOn,
    CanBreak,
    AttributeModifiers,
    CustomModelData,
    HideAdditionalTooltip,
    HideTooltip,
    RepairCost,
    CreativeSlotLock,
    EnchantmentGlintOverride,
    IntangibleProjectile,
    Food,
    Consumable,
    UseRemainder,
    UseCooldown,
    DamageResistant,
    Tool,
    Enchantable,
    Equippable,
    Repairable,
    Glider,
    TooltipStyle,
    DeathProtection,
    StoredEnchantments,
    DyedColor,
    MapColor,
    MapId,
    MapDecorations,
    MapPostProcessing,
    ChargedProjectiles,
    BundleContents,
    PotionContents,
    SuspiciousStewEffects,
    WritableBookContent,
    WrittenBookContent,
    Trim,
    DebugStickState,
    EntityData,
    BucketEntityData,
    BlockEntityData,
    Instrument,
    OminousBottleAmplifier,
    JukeboxPlayable,
    Recipes,
    LodestoneTracker,
    FireworkExplosion,
    Fireworks,
    Profile,
    NoteBlockSound,
    BannerPatterns,
    BaseColor,
    PotDecorations,
    Container,
    BlockState,
    Bees,
    Lock,
    ContainerLoot,
);

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CustomData {
    pub nbt: Nbt,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct MaxStackSize {
    #[var]
    pub count: i32,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct MaxDamage {
    #[var]
    pub amount: i32,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Damage {
    #[var]
    pub amount: i32,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct Unbreakable {
    #[serde(skip_serializing_if = "is_true")]
    pub show_in_tooltip: bool,
}
impl Unbreakable {
    pub const fn new() -> Self {
        Self {
            show_in_tooltip: true,
        }
    }
}
impl Default for Unbreakable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CustomName {
    pub name: FormattedText,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ItemName {
    pub name: FormattedText,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Lore {
    #[limit(256)]
    pub lines: Vec<FormattedText>,
    // vanilla also has styled_lines here but it doesn't appear to be used for the protocol
}

#[derive(AzBuf, Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
}

#[derive(AzBuf, Clone, PartialEq, Serialize, Debug)]
pub struct Enchantments {
    /// Enchantment levels here are 1-indexed, level 0 does not exist.
    #[var]
    pub levels: IndexMap<Enchantment, i32>,
    #[serde(skip_serializing_if = "is_true")]
    pub show_in_tooltip: bool,
}
impl Enchantments {
    pub fn new() -> Self {
        Self {
            levels: IndexMap::new(),
            show_in_tooltip: true,
        }
    }
}
impl Default for Enchantments {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum BlockStateValueMatcher {
    Exact {
        value: String,
    },
    Range {
        min: Option<String>,
        max: Option<String>,
    },
}
// vanilla encodes this with `ByteBufCodecs.either`, so it's a bool that's true
// for `Exact` (the left side) and false for `Range`.
impl AzBuf for BlockStateValueMatcher {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        if bool::azalea_read(buf)? {
            Ok(Self::Exact {
                value: String::azalea_read(buf)?,
            })
        } else {
            Ok(Self::Range {
                min: Option::<String>::azalea_read(buf)?,
                max: Option::<String>::azalea_read(buf)?,
            })
        }
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        match self {
            Self::Exact { value } => {
                true.azalea_write(buf)?;
                value.azalea_write(buf)
            }
            Self::Range { min, max } => {
                false.azalea_write(buf)?;
                min.azalea_write(buf)?;
                max.azalea_write(buf)
            }
        }
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct BlockStatePropertyMatcher {
    pub name: String,
    pub value_matcher: BlockStateValueMatcher,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct BlockPredicate {
    #[serde(skip_serializing_if = "is_default")]
    pub blocks: Option<HolderSet<BlockKind, Identifier>>,
    #[serde(skip_serializing_if = "is_default")]
    pub properties: Option<Vec<BlockStatePropertyMatcher>>,
    #[serde(skip_serializing_if = "is_default")]
    pub nbt: Option<NbtCompound>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct AdventureModePredicate {
    pub predicates: Vec<BlockPredicate>,
    #[serde(skip_serializing_if = "is_true")]
    pub show_in_tooltip: bool,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CanPlaceOn {
    pub predicate: AdventureModePredicate,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CanBreak {
    pub predicate: AdventureModePredicate,
}

#[derive(AzBuf, Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentSlotGroup {
    Any,
    Mainhand,
    Offhand,
    Hand,
    Feet,
    Legs,
    Chest,
    Head,
    Armor,
    Body,
}

// this is duplicated in azalea-entity, BUT the one there has a different
// protocol format (and we can't use it anyways because it would cause a
// circular dependency)
#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct AttributeModifier {
    pub id: Identifier,
    pub amount: f64,
    pub operation: AttributeModifierOperation,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct AttributeModifiersEntry {
    #[serde(rename = "type")]
    pub kind: Attribute,
    #[serde(flatten)]
    pub modifier: AttributeModifier,
    pub slot: EquipmentSlotGroup,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct AttributeModifiers {
    pub modifiers: Vec<AttributeModifiersEntry>,
    #[serde(skip_serializing_if = "is_true")]
    pub show_in_tooltip: bool,
}
impl AttributeModifiers {
    pub const fn new() -> Self {
        Self {
            modifiers: Vec::new(),
            show_in_tooltip: true,
        }
    }
}
impl Default for AttributeModifiers {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct CustomModelData {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub floats: Vec<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub strings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<i32>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RepairCost {
    #[var]
    pub cost: u32,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct HideAdditionalTooltip;

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct HideTooltip;

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct CreativeSlotLock;

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct EnchantmentGlintOverride {
    pub show_glint: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IntangibleProjectile;
// vanilla has no stream codec for this component, so it's sent as the NBT
// encoding of `Unit.CODEC` (an empty compound).
impl AzBuf for IntangibleProjectile {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        NbtTag::azalea_read(buf)?;
        Ok(Self)
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        NbtCompound::new().azalea_write(buf)
    }
}

/// Effect details hold the hidden effect's details, so their depth is
/// bounded.
#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[recursive]
pub struct MobEffectDetails {
    #[var]
    #[serde(skip_serializing_if = "is_default")]
    pub amplifier: i32,
    #[var]
    #[serde(skip_serializing_if = "is_default")]
    pub duration: i32,
    #[serde(skip_serializing_if = "is_default")]
    pub ambient: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub show_particles: bool,
    pub show_icon: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub hidden_effect: Option<Box<MobEffectDetails>>,
}
impl MobEffectDetails {
    pub const fn new() -> Self {
        MobEffectDetails {
            amplifier: 0,
            duration: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            hidden_effect: None,
        }
    }
}
impl Default for MobEffectDetails {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct MobEffectInstance {
    pub id: MobEffect,
    #[serde(flatten)]
    pub details: MobEffectDetails,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct PossibleEffect {
    pub effect: MobEffectInstance,
    pub probability: f32,
}

#[derive(AzBuf, Clone, Debug, Default, PartialEq, Serialize)]
pub struct Food {
    #[var]
    pub nutrition: i32,
    pub saturation: f32,
    #[serde(skip_serializing_if = "is_default")]
    pub can_always_eat: bool,
}

impl Food {
    pub const fn new() -> Self {
        Food {
            nutrition: 0,
            saturation: 0.,
            can_always_eat: false,
        }
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct ToolRule {
    pub blocks: HolderSet<BlockKind, Identifier>,
    #[serde(skip_serializing_if = "is_default")]
    pub speed: Option<f32>,
    #[serde(skip_serializing_if = "is_default")]
    pub correct_for_drops: Option<bool>,
}
impl ToolRule {
    pub const fn new() -> Self {
        ToolRule {
            blocks: HolderSet::Direct { contents: vec![] },
            speed: None,
            correct_for_drops: None,
        }
    }
}
impl Default for ToolRule {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct Tool {
    #[serde(serialize_with = "flatten_array")]
    pub rules: Vec<ToolRule>,
    #[serde(skip_serializing_if = "is_default")]
    pub default_mining_speed: f32,
    #[var]
    #[serde(skip_serializing_if = "is_default")]
    pub damage_per_block: i32,
}

impl Tool {
    pub const fn new() -> Self {
        Tool {
            rules: vec![],
            default_mining_speed: 1.,
            damage_per_block: 1,
        }
    }
}
impl Default for Tool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct StoredEnchantments {
    #[var]
    #[serde(rename = "levels")]
    pub enchantments: IndexMap<Enchantment, i32>,
    #[serde(skip_serializing_if = "is_true")]
    pub show_in_tooltip: bool,
}
impl StoredEnchantments {
    pub fn new() -> Self {
        Self {
            enchantments: IndexMap::new(),
            show_in_tooltip: true,
        }
    }
}
impl Default for StoredEnchantments {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct DyedColor {
    pub rgb: i32,
    #[serde(skip_serializing_if = "is_true")]
    pub show_in_tooltip: bool,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct MapColor {
    pub color: i32,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct MapId {
    #[var]
    pub id: i32,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct MapDecorations {
    pub decorations: NbtCompound,
}

#[derive(AzBuf, Clone, Copy, Debug, PartialEq, Serialize)]
pub enum MapPostProcessing {
    Lock,
    Scale,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ChargedProjectiles {
    pub items: Vec<ItemStack>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BundleContents {
    pub items: Vec<ItemStack>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct PotionContents {
    #[serde(skip_serializing_if = "is_default")]
    pub potion: Option<Potion>,
    #[serde(skip_serializing_if = "is_default")]
    pub custom_color: Option<i32>,
    #[serde(skip_serializing_if = "is_default")]
    pub custom_effects: Vec<MobEffectInstance>,
    #[serde(skip_serializing_if = "is_default")]
    pub custom_name: Option<String>,
}

impl PotionContents {
    pub const fn new() -> Self {
        PotionContents {
            potion: None,
            custom_color: None,
            custom_effects: vec![],
            custom_name: None,
        }
    }
}
impl Default for PotionContents {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct SuspiciousStewEffect {
    #[serde(rename = "id")]
    pub effect: MobEffect,
    #[var]
    #[serde(skip_serializing_if = "is_default")]
    pub duration: i32,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SuspiciousStewEffects {
    pub effects: Vec<SuspiciousStewEffect>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct WritableBookContent {
    pub pages: Vec<Filterable<String>>,
}

#[derive(AzBuf, Clone, PartialEq, Serialize, Debug)]
pub struct WrittenBookContent {
    #[limit(32)]
    pub title: Filterable<String>,
    pub author: String,
    #[var]
    #[serde(skip_serializing_if = "is_default")]
    pub generation: i32,
    #[serde(skip_serializing_if = "is_default")]
    pub pages: Vec<Filterable<FormattedText>>,
    #[serde(skip_serializing_if = "is_default")]
    pub resolved: bool,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct Trim {
    pub material: Holder<TrimMaterial, DirectTrimMaterial>,
    pub pattern: Holder<TrimPattern, DirectTrimPattern>,
    #[serde(skip_serializing_if = "is_true")]
    pub show_in_tooltip: bool,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DebugStickState {
    pub properties: NbtCompound,
}

/// The NBT data of the entity that this item spawns. The entity's type is the
/// `id` tag in the NBT.
#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct EntityData {
    pub data: NbtCompound,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BucketEntityData {
    pub entity: NbtCompound,
}

/// The NBT data of the block entity that this item places. The block entity's
/// type is the `id` tag in the NBT.
#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BlockEntityData {
    pub data: NbtCompound,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Instrument {
    pub value: Holder<azalea_registry::data::Instrument, InstrumentData>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct InstrumentData {
    pub sound_event: Holder<SoundEvent, azalea_core::sound::CustomSound>,
    pub use_duration: f32,
    pub range: f32,
    pub description: FormattedText,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct OminousBottleAmplifier {
    #[var]
    pub amplifier: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Recipes {
    pub recipes: Vec<Identifier>,
}
// vanilla has no stream codec for this component, so it's sent as NBT (a list
// of strings).
impl AzBuf for Recipes {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let tag = NbtTag::azalea_read(buf)?;
        let mut recipes = Vec::new();
        if let NbtTag::List(list) = tag
            && let Some(strings) = list.strings()
        {
            for s in strings {
                recipes.push(Identifier::new(s.to_str()));
            }
        }
        Ok(Self { recipes })
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        let list = NbtList::String(self.recipes.iter().map(|r| r.to_string().into()).collect());
        NbtTag::List(list).azalea_write(buf)
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct LodestoneTracker {
    #[serde(skip_serializing_if = "is_default")]
    pub target: Option<GlobalPos>,
    #[serde(skip_serializing_if = "is_true")]
    pub tracked: bool,
}

#[derive(AzBuf, Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FireworkExplosionShape {
    SmallBall,
    LargeBall,
    Star,
    Creeper,
    Burst,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct FireworkExplosion {
    pub shape: FireworkExplosionShape,
    #[serde(skip_serializing_if = "is_default")]
    pub colors: Vec<i32>,
    #[serde(skip_serializing_if = "is_default")]
    pub fade_colors: Vec<i32>,
    #[serde(skip_serializing_if = "is_default")]
    pub has_trail: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub has_twinkle: bool,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct Fireworks {
    #[var]
    #[serde(skip_serializing_if = "is_default")]
    pub flight_duration: i32,
    #[limit(256)]
    pub explosions: Vec<FireworkExplosion>,
}

impl Fireworks {
    pub const fn new() -> Self {
        Fireworks {
            flight_duration: 0,
            explosions: vec![],
        }
    }
}
impl Default for Fireworks {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NoteBlockSound {
    pub sound: Identifier,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct BannerPatternLayer {
    pub pattern: Holder<BannerPatternKind, BannerPatternData>,
    pub color: DyeColor,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct BannerPatternData {
    pub asset_id: Identifier,
    pub translation_key: String,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BannerPatterns {
    pub patterns: Vec<BannerPatternLayer>,
}

#[derive(AzBuf, Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DyeColor {
    White,
    Orange,
    Magenta,
    LightBlue,
    Yellow,
    Lime,
    Pink,
    Gray,
    LightGray,
    Cyan,
    Purple,
    Blue,
    Brown,
    Green,
    Red,
    Black,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BaseColor {
    pub color: DyeColor,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct PotDecorations {
    #[limit(4)]
    pub items: Vec<ItemKind>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Container {
    #[limit(256)]
    pub items: Vec<ItemStack>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BlockState {
    pub properties: IndexMap<String, String>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct BeehiveOccupant {
    #[serde(skip_serializing_if = "is_default")]
    pub entity_data: NbtCompound,
    #[var]
    pub ticks_in_hive: i32,
    #[var]
    pub min_ticks_in_hive: i32,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Bees {
    pub occupants: Vec<BeehiveOccupant>,
}

/// An item predicate that the key item must match.
///
/// Vanilla has no stream codec for this component, so it's sent as NBT.
#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Lock {
    pub predicate: NbtCompound,
}

/// Vanilla has no stream codec for this component, so it's sent as NBT (a
/// compound with `loot_table` and `seed`).
#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ContainerLoot {
    pub loot_table: NbtCompound,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct JukeboxPlayable {
    pub song: JukeboxSongHolder,
    #[serde(skip_serializing_if = "is_true")]
    pub show_in_tooltip: bool,
}

/// Vanilla's `EitherHolder<JukeboxSong>`: either a holder for the song, or the
/// key of a song that's resolved later.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum JukeboxSongHolder {
    Holder(Holder<JukeboxSong, JukeboxSongData>),
    Key(Identifier),
}
// `ByteBufCodecs.either`: true for the holder (left), false for the key.
impl AzBuf for JukeboxSongHolder {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        if bool::azalea_read(buf)? {
            Ok(Self::Holder(AzBuf::azalea_read(buf)?))
        } else {
            Ok(Self::Key(Identifier::azalea_read(buf)?))
        }
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        match self {
            Self::Holder(holder) => {
                true.azalea_write(buf)?;
                holder.azalea_write(buf)
            }
            Self::Key(key) => {
                false.azalea_write(buf)?;
                key.azalea_write(buf)
            }
        }
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct JukeboxSongData {
    pub sound_event: Holder<SoundEvent, CustomSound>,
    pub description: FormattedText,
    pub length_in_seconds: f32,
    #[var]
    pub comparator_output: i32,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct Consumable {
    #[serde(skip_serializing_if = "is_default")]
    pub consume_seconds: f32,
    #[serde(skip_serializing_if = "is_default")]
    pub animation: ItemUseAnimation,
    #[serde(skip_serializing_if = "is_default_eat_sound")]
    pub sound: azalea_registry::Holder<SoundEvent, CustomSound>,
    #[serde(skip_serializing_if = "is_default")]
    pub has_consume_particles: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub on_consume_effects: Vec<ConsumeEffect>,
}
fn is_default_eat_sound(sound: &azalea_registry::Holder<SoundEvent, CustomSound>) -> bool {
    matches!(
        sound,
        azalea_registry::Holder::Reference(SoundEvent::EntityGenericEat)
    )
}

impl Consumable {
    pub const fn new() -> Self {
        Self {
            consume_seconds: 1.6,
            animation: ItemUseAnimation::Eat,
            sound: azalea_registry::Holder::Reference(SoundEvent::EntityGenericEat),
            has_consume_particles: true,
            on_consume_effects: Vec::new(),
        }
    }
}
impl Default for Consumable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(AzBuf, Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemUseAnimation {
    #[default]
    None,
    Eat,
    Drink,
    BlockKind,
    Bow,
    Spear,
    Crossbow,
    Spyglass,
    TootHorn,
    Brush,
    Bundle,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UseRemainder {
    pub convert_into: ItemStack,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct UseCooldown {
    pub seconds: f32,
    #[serde(skip_serializing_if = "is_default")]
    pub cooldown_group: Option<Identifier>,
}

impl UseCooldown {
    pub const fn new() -> Self {
        Self {
            seconds: 0.,
            cooldown_group: None,
        }
    }
}
impl Default for UseCooldown {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct Enchantable {
    #[var]
    pub value: u32,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct Repairable {
    pub items: HolderSet<ItemKind, Identifier>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ItemModel {
    pub resource_location: Identifier,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct DamageResistant {
    /// The damage type tag that this item is resistant to.
    pub types: Identifier,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Equippable {
    pub slot: EquipmentSlot,
    #[serde(skip_serializing_if = "is_default_equip_sound")]
    pub equip_sound: Holder<SoundEvent, CustomSound>,
    #[serde(skip_serializing_if = "is_default")]
    pub asset_id: Option<Identifier>,
    #[serde(skip_serializing_if = "is_default")]
    pub camera_overlay: Option<Identifier>,
    #[serde(skip_serializing_if = "is_default")]
    pub allowed_entities: Option<HolderSet<EntityKind, Identifier>>,
    #[serde(skip_serializing_if = "is_true")]
    pub dispensable: bool,
    #[serde(skip_serializing_if = "is_true")]
    pub swappable: bool,
    #[serde(skip_serializing_if = "is_true")]
    pub damage_on_hurt: bool,
}
fn is_default_equip_sound(sound: &Holder<SoundEvent, CustomSound>) -> bool {
    matches!(sound, Holder::Reference(SoundEvent::ItemArmorEquipGeneric))
}
// written by hand because the slot uses vanilla's `EquipmentSlot.id` here,
// which isn't the same as the ordinal (see `EquipmentSlot::component_id`).
impl AzBuf for Equippable {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let slot_id = u32::azalea_read_var(buf)?;
        // vanilla's ByIdMap uses OutOfBoundsStrategy.ZERO
        let slot = EquipmentSlot::from_component_id(slot_id).unwrap_or(EquipmentSlot::Mainhand);
        Ok(Self {
            slot,
            equip_sound: AzBuf::azalea_read(buf)?,
            asset_id: AzBuf::azalea_read(buf)?,
            camera_overlay: AzBuf::azalea_read(buf)?,
            allowed_entities: AzBuf::azalea_read(buf)?,
            dispensable: AzBuf::azalea_read(buf)?,
            swappable: AzBuf::azalea_read(buf)?,
            damage_on_hurt: AzBuf::azalea_read(buf)?,
        })
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        self.slot.component_id().azalea_write_var(buf)?;
        self.equip_sound.azalea_write(buf)?;
        self.asset_id.azalea_write(buf)?;
        self.camera_overlay.azalea_write(buf)?;
        self.allowed_entities.azalea_write(buf)?;
        self.dispensable.azalea_write(buf)?;
        self.swappable.azalea_write(buf)?;
        self.damage_on_hurt.azalea_write(buf)
    }
}

impl Equippable {
    pub const fn new() -> Self {
        Self {
            slot: EquipmentSlot::Body,
            equip_sound: Holder::Reference(SoundEvent::ItemArmorEquipGeneric),
            asset_id: None,
            camera_overlay: None,
            allowed_entities: None,
            dispensable: true,
            swappable: true,
            damage_on_hurt: true,
        }
    }
}
impl Default for Equippable {
    fn default() -> Self {
        Self::new()
    }
}

/// An enum that represents inventory slots that can hold items.
///
/// Its protocol encoding is the ordinal, which is what `set_equipment` uses.
/// Data components use [`Self::component_id`] instead.
#[derive(AzBuf, Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentSlot {
    Mainhand,
    Offhand,
    Feet,
    Legs,
    Chest,
    Head,
    /// This is for animal armor, use [`Self::Chest`] for the chestplate slot.
    Body,
}
impl EquipmentSlot {
    #[must_use]
    pub fn from_byte(byte: u8) -> Option<Self> {
        let value = match byte {
            0 => Self::Mainhand,
            1 => Self::Offhand,
            2 => Self::Feet,
            3 => Self::Legs,
            4 => Self::Chest,
            5 => Self::Head,
            6 => Self::Body,
            _ => return None,
        };
        Some(value)
    }
    /// The id that vanilla uses for this slot in data components
    /// (`EquipmentSlot.id`), which differs from the ordinal for the offhand
    /// and armor slots.
    pub fn component_id(self) -> u32 {
        match self {
            Self::Mainhand => 0,
            Self::Feet => 1,
            Self::Legs => 2,
            Self::Chest => 3,
            Self::Head => 4,
            Self::Offhand => 5,
            Self::Body => 6,
        }
    }
    /// The inverse of [`Self::component_id`].
    pub fn from_component_id(id: u32) -> Option<Self> {
        Some(match id {
            0 => Self::Mainhand,
            1 => Self::Feet,
            2 => Self::Legs,
            3 => Self::Chest,
            4 => Self::Head,
            5 => Self::Offhand,
            6 => Self::Body,
            _ => return None,
        })
    }
    pub fn values() -> [Self; 7] {
        [
            Self::Mainhand,
            Self::Offhand,
            Self::Feet,
            Self::Legs,
            Self::Chest,
            Self::Head,
            Self::Body,
        ]
    }
    /// Get the display name for the equipment slot, like "mainhand".
    pub fn name(self) -> &'static str {
        match self {
            Self::Mainhand => "mainhand",
            Self::Offhand => "offhand",
            Self::Feet => "feet",
            Self::Legs => "legs",
            Self::Chest => "chest",
            Self::Head => "head",
            Self::Body => "body",
        }
    }
}
impl Display for EquipmentSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct Glider;

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct TooltipStyle {
    pub resource_location: Identifier,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct DeathProtection {
    pub death_effects: Vec<ConsumeEffect>,
}

#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct PaintingVariantData {
    #[var]
    pub width: i32,
    #[var]
    pub height: i32,
    pub asset_id: Identifier,
    #[serde(skip_serializing_if = "is_default")]
    pub title: Option<FormattedText>,
    #[serde(skip_serializing_if = "is_default")]
    pub author: Option<FormattedText>,
}

/// An inline trim material, used when the material isn't a registry
/// reference.
#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct DirectTrimMaterial {
    pub asset_name: String,
    pub ingredient: ItemKind,
    /// A map of equipment asset ids to asset names.
    #[serde(skip_serializing_if = "is_default")]
    pub override_armor_assets: IndexMap<Identifier, String>,
    pub description: FormattedText,
}

/// An inline trim pattern, used when the pattern isn't a registry reference.
#[derive(AzBuf, Clone, Debug, PartialEq, Serialize)]
pub struct DirectTrimPattern {
    pub asset_id: Identifier,
    pub template_item: ItemKind,
    pub description: FormattedText,
    #[serde(skip_serializing_if = "is_default")]
    pub decal: bool,
}
