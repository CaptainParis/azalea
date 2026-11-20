use azalea_buf::AzBuf;
use azalea_core::position::BlockPos;
use azalea_protocol_macros::ServerboundGamePacket;

#[derive(AzBuf, Clone, Debug, PartialEq, ServerboundGamePacket)]
pub struct ServerboundPickItemFromBlock {
    pub pos: BlockPos,
    /// Whether to copy the block entity data (the vanilla client sends this
    /// when control is held in creative mode).
    pub include_data: bool,
}
