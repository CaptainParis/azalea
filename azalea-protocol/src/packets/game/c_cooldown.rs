use azalea_buf::AzBuf;
use azalea_protocol_macros::ClientboundGamePacket;
use azalea_registry::identifier::Identifier;

#[derive(AzBuf, ClientboundGamePacket, Clone, Debug, PartialEq)]
pub struct ClientboundCooldown {
    /// The item's `use_cooldown` cooldown group, or the item's id if it doesn't
    /// have one.
    pub cooldown_group: Identifier,
    #[var]
    pub duration: u32,
}
