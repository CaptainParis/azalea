use azalea_buf::AzBuf;
use azalea_core::{entity_id::MinecraftEntityId, position::Vec3f32};
use azalea_protocol_macros::ServerboundGamePacket;

#[derive(AzBuf, Clone, Debug, PartialEq, ServerboundGamePacket)]
pub struct ServerboundInteract {
    #[var]
    pub entity_id: MinecraftEntityId,
    pub action: ActionType,
    /// Whether the player is sneaking.
    pub using_secondary_action: bool,
}

/// What the player is doing to the entity. Attacking is also done with this
/// packet in 1.21.4.
#[derive(AzBuf, Clone, Copy, Debug, PartialEq)]
pub enum ActionType {
    Interact {
        hand: InteractionHand,
    },
    Attack,
    InteractAt {
        /// The position on the entity's hitbox, relative to the entity.
        location: Vec3f32,
        hand: InteractionHand,
    },
}

#[derive(AzBuf, Clone, Copy, Debug, Default, PartialEq)]
pub enum InteractionHand {
    #[default]
    MainHand = 0,
    OffHand = 1,
}
