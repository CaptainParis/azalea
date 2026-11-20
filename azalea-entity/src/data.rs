//! Define some types needed for entity metadata.

// TODO: this is here because of a bug in enum_as_inner. remove when it's fixed
#![allow(clippy::double_parens)]

use std::io::{self, Cursor, Write};

use azalea_buf::{AzBuf, AzBufVar, BufReadError};
use azalea_chat::FormattedText;
use azalea_core::{
    direction::Direction,
    position::{BlockPos, GlobalPos, Vec3f32},
};
use azalea_inventory::{ItemStack, components::PaintingVariantData};
use azalea_registry::{
    Holder, HolderSet,
    builtin::{VillagerKind, VillagerProfession},
    data::Biome,
    identifier::Identifier,
};
use derive_more::Deref;
use enum_as_inner::EnumAsInner;
use uuid::Uuid;

use crate::particle::Particle;

#[derive(Clone, Debug, Deref, PartialEq)]
pub struct EntityMetadataItems(pub Vec<EntityDataItem>);

#[derive(Clone, Debug, PartialEq)]
pub struct EntityDataItem {
    // we can't identify what the index is for here because we don't know the
    // entity type
    pub index: u8,
    pub value: EntityDataValue,
}

impl AzBuf for EntityMetadataItems {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let mut metadata = Vec::new();
        loop {
            let id = u8::azalea_read(buf)?;
            if id == 0xff {
                break;
            }
            let value = EntityDataValue::azalea_read(buf)?;
            metadata.push(EntityDataItem { index: id, value });
        }
        Ok(EntityMetadataItems(metadata))
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        for item in &self.0 {
            item.index.azalea_write(buf)?;
            item.value.azalea_write(buf)?;
        }
        0xffu8.azalea_write(buf)?;
        Ok(())
    }
}

// Note: This enum is partially generated and parsed by
// codegen/lib/code/entity.py
#[derive(AzBuf, Clone, Debug, EnumAsInner, PartialEq)]
pub enum EntityDataValue {
    // Order and set are those of 1.21.4's EntityDataSerializers (protocol 769).
    Byte(u8),
    Int(#[var] i32),
    Long(#[var] i64),
    Float(f32),
    String(Box<str>),
    FormattedText(Box<FormattedText>),
    OptionalFormattedText(Option<Box<FormattedText>>),
    ItemStack(ItemStack),
    Boolean(bool),
    Rotations(Rotations),
    BlockPos(BlockPos),
    OptionalBlockPos(Option<BlockPos>),
    Direction(Direction),
    OptionalUuid(Option<Uuid>),
    BlockState(azalea_block::BlockState),
    /// If this is air, that means it's absent,
    OptionalBlockState(azalea_block::BlockState),
    CompoundTag(Box<simdnbt::owned::NbtCompound>),
    Particle(Particle),
    Particles(Box<[Particle]>),
    VillagerData(VillagerData),
    // 0 for absent; 1 + actual value otherwise. Used for entity IDs.
    OptionalUnsignedInt(OptionalUnsignedInt),
    Pose(Pose),
    CatVariant(azalea_registry::builtin::CatVariant),
    WolfVariant(WolfVariantHolder),
    FrogVariant(azalea_registry::builtin::FrogVariant),
    OptionalGlobalPos(Option<Box<GlobalPos>>),
    PaintingVariant(PaintingVariantHolder),
    SnifferState(SnifferStateKind),
    ArmadilloState(ArmadilloStateKind),
    Vector3(Vec3f32),
    Quaternion(Quaternion),
}

const _: () = assert!(size_of::<EntityDataValue>() == 24);

#[derive(Clone, Debug, PartialEq, Default)]
pub struct OptionalUnsignedInt(pub Option<u32>);

#[derive(AzBuf, Clone, Debug, PartialEq, Default)]
pub struct Quaternion {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

// mojang just calls this ArmadilloState but i added "Kind" since otherwise it
// collides with a name in metadata.rs
#[derive(AzBuf, Clone, Copy, Debug, Default, PartialEq)]
pub enum ArmadilloStateKind {
    #[default]
    Idle,
    Rolling,
    Scared,
    Unrolling,
}

/// A wolf variant: a registry id, or an inline [`WolfVariantData`].
///
/// Vanilla encodes this with `ByteBufCodecs.holder`, so a registry id is sent
/// as `id + 1` and 0 means an inline value follows.
pub type WolfVariantHolder = Holder<azalea_registry::data::WolfVariant, Box<WolfVariantData>>;
/// A painting variant: a registry id, or an inline [`PaintingVariantData`].
///
/// Encoded like [`WolfVariantHolder`].
pub type PaintingVariantHolder =
    Holder<azalea_registry::data::PaintingVariant, Box<PaintingVariantData>>;

/// An inline wolf variant, used when the variant isn't a registry reference.
#[derive(AzBuf, Clone, Debug, PartialEq)]
pub struct WolfVariantData {
    pub wild_texture: Identifier,
    pub tame_texture: Identifier,
    pub angry_texture: Identifier,
    pub biomes: HolderSet<Biome, Identifier>,
}

impl AzBuf for OptionalUnsignedInt {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let val = u32::azalea_read_var(buf)?;
        Ok(OptionalUnsignedInt(if val == 0 {
            None
        } else {
            Some(val - 1)
        }))
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        match self.0 {
            Some(val) => (val + 1).azalea_write_var(buf),
            None => 0u32.azalea_write_var(buf),
        }
    }
}

/// A set of x, y, and z rotations. This is used for armor stands.
#[derive(AzBuf, Clone, Debug, Default, PartialEq)]
pub struct Rotations {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[cfg_attr(feature = "bevy_ecs", derive(bevy_ecs::component::Component))]
#[derive(AzBuf, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Pose {
    #[default]
    Standing = 0,
    FallFlying,
    Sleeping,
    Swimming,
    SpinAttack,
    Crouching,
    LongJumping,
    Dying,
    Croaking,
    UsingTongue,
    Sitting,
    Roaring,
    Sniffing,
    Emerging,
    Digging,
    Sliding,
    Shooting,
    Inhaling,
}

#[derive(AzBuf, Clone, Debug, PartialEq)]
pub struct VillagerData {
    pub kind: VillagerKind,
    pub profession: VillagerProfession,
    #[var]
    pub level: u32,
}

#[derive(AzBuf, Clone, Copy, Debug, Default, PartialEq)]
pub enum SnifferStateKind {
    #[default]
    Idling,
    FeelingHappy,
    Scenting,
    Sniffing,
    Searching,
    Digging,
    Rising,
}

#[derive(AzBuf, Clone, Copy, Debug, Default, PartialEq)]
pub enum CopperGolemStateKind {
    #[default]
    Idle,
    GettingItem,
    GettingNoItem,
    DroppingItem,
    DroppingNoItem,
}
#[derive(AzBuf, Clone, Copy, Debug, Default, PartialEq)]
pub enum WeatheringCopperStateKind {
    #[default]
    Unaffected,
    Exposed,
    Weathered,
    Oxidized,
}

#[derive(AzBuf, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HumanoidArm {
    Left = 0,
    #[default]
    Right = 1,
}
