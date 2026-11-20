use std::{
    io::{self, Cursor, Write},
    ops::Deref,
    str::FromStr,
    sync::Arc,
};

use azalea_buf::{AzBuf, BufReadError};
use azalea_core::{
    decode_errors::{DecodeErrorKind, record_decode_error},
    heightmap_kind::HeightmapKind,
};
use azalea_protocol_macros::ClientboundGamePacket;
use azalea_registry::builtin::BlockEntityKind;
use simdnbt::owned::{Nbt, NbtCompound, NbtTag};
use tracing::warn;

use super::c_light_update::ClientboundLightUpdatePacketData;

#[derive(AzBuf, ClientboundGamePacket, Clone, Debug, PartialEq)]
pub struct ClientboundLevelChunkWithLight {
    // this can't be a ChunkPos since that reads z first and then x
    pub x: i32,
    pub z: i32,
    pub chunk_data: ClientboundLevelChunkPacketData,
    pub light_data: ClientboundLightUpdatePacketData,
}

#[derive(AzBuf, Clone, Debug, PartialEq)]
pub struct ClientboundLevelChunkPacketData {
    pub heightmaps: Heightmaps,
    /// The raw chunk sections.
    ///
    /// We can't parse the data in `azalea-protocol` because sometimes we want
    /// to skip parsing this.
    ///
    /// This is an Arc because it's often very big and we want it to be cheap to
    /// clone.
    pub data: Arc<Box<[u8]>>,
    pub block_entities: Vec<BlockEntity>,
}

/// The chunk's heightmaps.
///
/// In 1.21.4 these are sent as an NBT compound that maps each kind's name
/// (like `MOTION_BLOCKING`) to a long array.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heightmaps(pub Vec<(HeightmapKind, Box<[u64]>)>);

impl Deref for Heightmaps {
    type Target = [(HeightmapKind, Box<[u64]>)];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AzBuf for Heightmaps {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let compound = NbtCompound::azalea_read(buf)?;
        let mut heightmaps = Vec::new();
        for (name, tag) in compound.iter() {
            let name = name.to_str();
            let Ok(kind) = HeightmapKind::from_str(&name) else {
                warn!("Unknown heightmap kind {name:?}");
                record_decode_error(DecodeErrorKind::Chunk);
                continue;
            };
            let Some(data) = tag.long_array() else {
                return Err(BufReadError::Custom(format!(
                    "Heightmap {name} is not a long array"
                )));
            };
            heightmaps.push((kind, data.iter().map(|&n| n as u64).collect()));
        }
        Ok(Heightmaps(heightmaps))
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        let values = self
            .iter()
            .map(|(kind, data)| {
                let data = data.iter().map(|&n| n as i64).collect();
                (kind.to_string().into(), NbtTag::LongArray(data))
            })
            .collect();
        NbtCompound::from_values(values).azalea_write(buf)
    }
}

#[derive(AzBuf, Clone, Debug, PartialEq)]
pub struct BlockEntity {
    pub packed_xz: u8,
    pub y: u16,
    pub kind: BlockEntityKind,
    pub data: Nbt,
}
