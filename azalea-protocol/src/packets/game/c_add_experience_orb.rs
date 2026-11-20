use azalea_buf::AzBuf;
use azalea_core::{entity_id::MinecraftEntityId, position::Vec3};
use azalea_protocol_macros::ClientboundGamePacket;

#[derive(AzBuf, ClientboundGamePacket, Clone, Debug, PartialEq)]
pub struct ClientboundAddExperienceOrb {
    #[var]
    pub id: MinecraftEntityId,
    pub pos: Vec3,
    pub value: u16,
}
