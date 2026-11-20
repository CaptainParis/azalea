use std::borrow::Cow;

use azalea_block::BlockState;
use azalea_core::{
    math::get_seed,
    position::{BlockPos, Vec3},
};
use azalea_registry::builtin::BlockKind;

use crate::collision::{VoxelShape, blocks::RANDOM_SHAPE_OFFSETS_MAP};

/// Adds the random offset for the shape, given the block state and position.
///
/// For most blocks, this won't have any effect. It's only used for a few blocks
/// like flowers and bamboo.
pub fn apply_shape_offset(
    block: BlockState,
    pos: BlockPos,
    shape: &'static VoxelShape,
) -> Cow<'static, VoxelShape> {
    // don't waste time checking the block if it's already known to be empty.
    // also, it's faster to compare addresses than to call `.is_empty`.
    if std::ptr::eq(shape, &*super::EMPTY_SHAPE) {
        return Cow::Borrowed(shape);
    }

    // network-safe: state ids are at most BlockState::MAX_STATE (checked when
    // decoded), the map has MAX_STATE + 1 entries (test
    // every_block_state_has_shape_table_entries)
    let offset_kind = RANDOM_SHAPE_OFFSETS_MAP[block.id() as usize];
    if offset_kind == 0 {
        return Cow::Borrowed(shape);
    }

    let kind = block.as_block_kind();

    let mut max_horizontal_offset = 0.25;
    // search `getMaxHorizontalOffset` in the vanilla code
    // TODO: sulfur spike gets added here in 26.2
    if kind == BlockKind::PointedDripstone {
        max_horizontal_offset = 2. / 16.;
    }

    // these ids are required to be the same as the ones in shapes.py
    let delta = match offset_kind {
        // see offsetType in BlockBehaviour.java
        1 => {
            // xz
            xyz_offset_for_pos(pos.with_y(0), max_horizontal_offset, 0.)
        }
        2 => {
            // xyz

            let mut max_vertical_offset = 0.2;
            if kind == BlockKind::SmallDripleaf {
                // search `getMaxVerticalOffset` in the vanilla code
                max_vertical_offset = 0.1;
            }

            xyz_offset_for_pos(pos, max_horizontal_offset, max_vertical_offset)
        }
        // network-safe: the generated table only has 0, 1 and 2 (shapes.py)
        _ => unreachable!(),
    };

    Cow::Owned(shape.move_relative(delta))
}

fn xyz_offset_for_pos(pos: BlockPos, max_horizontal_offset: f64, max_vertical_offset: f64) -> Vec3 {
    let seed = get_seed(pos);
    let y = if max_vertical_offset == 0. {
        0.
    } else {
        ((((seed >> 4) & 15) as f32 / 15.) as f64 - 1.) * max_vertical_offset
    };

    let x = (((seed & 15) as f32 / 15.) as f64 - 0.5) * 0.5;
    let x = x.clamp(-max_horizontal_offset, max_horizontal_offset);
    let z = ((((seed >> 8) & 15) as f32 / 15.) as f64 - 0.5) * 0.5;
    let z = z.clamp(-max_horizontal_offset, max_horizontal_offset);

    Vec3 { x, y, z }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::BlockWithShape;

    // the tables indexed by state id (here and in blocks.rs) cover every state
    // a decoded block state can have
    #[test]
    fn every_block_state_has_shape_table_entries() {
        assert_eq!(
            RANDOM_SHAPE_OFFSETS_MAP.len(),
            BlockState::MAX_STATE as usize + 1
        );
        for id in 0..=BlockState::MAX_STATE {
            let state = BlockState::try_from(id).unwrap();
            let _ = state.is_collision_shape_empty();
            let _ = state.is_collision_shape_full();
            let _ = state.collision_shape(BlockPos::new(i32::MAX, 0, i32::MIN));
        }
    }
}
