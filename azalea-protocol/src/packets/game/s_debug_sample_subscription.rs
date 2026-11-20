use azalea_buf::AzBuf;
use azalea_protocol_macros::ServerboundGamePacket;

use crate::packets::game::c_debug_sample::RemoteDebugSampleType;

#[derive(AzBuf, Clone, Debug, PartialEq, ServerboundGamePacket)]
pub struct ServerboundDebugSampleSubscription {
    pub sample_type: RemoteDebugSampleType,
}
