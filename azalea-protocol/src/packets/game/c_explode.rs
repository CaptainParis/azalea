use azalea_buf::AzBuf;
use azalea_core::{position::Vec3, sound::CustomSound};
use azalea_entity::particle::Particle;
use azalea_protocol_macros::ClientboundGamePacket;
use azalea_registry::{Holder, builtin::SoundEvent};

#[derive(AzBuf, ClientboundGamePacket, Clone, Debug, PartialEq)]
pub struct ClientboundExplode {
    pub center: Vec3,
    pub player_knockback: Option<Vec3>,
    pub explosion_particle: Particle,
    pub explosion_sound: Holder<SoundEvent, CustomSound>,
}
