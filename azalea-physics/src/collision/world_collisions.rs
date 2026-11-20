use std::{borrow::Cow, collections::HashSet, sync::Arc};

use azalea_block::{BlockState, fluid_state::FluidState};
use azalea_core::{
    cursor3d::{Cursor3d, CursorIteration, CursorIterationType},
    math::EPSILON,
    position::{BlockPos, ChunkBlockPos, ChunkPos, ChunkSectionBlockPos, ChunkSectionPos, Vec3},
};
use azalea_inventory::ItemStack;
use azalea_world::{Chunk, World};
use bevy_ecs::entity::Entity;
use parking_lot::RwLock;

use super::{BLOCK_SHAPE, Shapes};
use crate::collision::{Aabb, BlockWithShape, VoxelShape};

pub fn get_block_collisions(world: &World, aabb: &Aabb) -> Vec<VoxelShape> {
    let mut state = BlockCollisionsState::new(world, aabb, EntityCollisionContext::of(None));
    let mut block_collisions = Vec::new();

    let initial_chunk_pos = ChunkPos::from(state.cursor.origin());
    let initial_chunk = world.chunks.get(&initial_chunk_pos);
    let initial_chunk = initial_chunk.as_deref().map(RwLock::read);

    while let Some(item) = state.cursor.next() {
        state.compute_next(
            item,
            &mut block_collisions,
            initial_chunk_pos,
            initial_chunk.as_deref(),
        );
    }

    block_collisions
}

pub fn get_block_and_liquid_collisions(world: &World, aabb: &Aabb) -> Vec<VoxelShape> {
    let mut state = BlockCollisionsState::new(
        world,
        aabb,
        EntityCollisionContext::of(None).with_include_liquids(true),
    );
    let mut block_collisions = Vec::new();

    let initial_chunk_pos = ChunkPos::from(state.cursor.origin());
    let initial_chunk = world.chunks.get(&initial_chunk_pos);
    let initial_chunk = initial_chunk.as_deref().map(RwLock::read);

    while let Some(item) = state.cursor.next() {
        state.compute_next(
            item,
            &mut block_collisions,
            initial_chunk_pos,
            initial_chunk.as_deref(),
        );
    }

    block_collisions
}

pub struct BlockCollisionsState<'a> {
    pub world: &'a World,
    pub aabb: &'a Aabb,
    pub entity_shape: VoxelShape,
    pub cursor: Cursor3d,

    _context: EntityCollisionContext,

    cached_sections: Vec<(ChunkSectionPos, azalea_world::Section)>,
    cached_block_shapes: Vec<(BlockState, &'static VoxelShape)>,
}

impl<'a> BlockCollisionsState<'a> {
    fn compute_next(
        &mut self,
        item: CursorIteration,
        block_collisions: &mut Vec<VoxelShape>,
        initial_chunk_pos: ChunkPos,
        initial_chunk: Option<&Chunk>,
    ) {
        if item.iteration_type == CursorIterationType::Corner {
            return;
        }

        let item_chunk_pos = ChunkPos::from(item.pos);
        let block_state: BlockState = if item_chunk_pos == initial_chunk_pos {
            initial_chunk
                .and_then(|chunk| {
                    chunk.get_block_state(&ChunkBlockPos::from(item.pos), self.world.chunks.min_y())
                })
                .unwrap_or(BlockState::AIR)
        } else {
            self.get_block_state(item.pos)
        };

        if block_state.is_air() {
            // fast path since we can't collide with air
            return;
        }

        // TODO: if self.only_suffocating_blocks, return if the block isn't
        // suffocating

        // if it's a full block do a faster collision check
        if block_state.is_collision_shape_full() {
            if !self.aabb.intersects_aabb(&Aabb {
                min: item.pos.to_vec3_floored(),
                max: (item.pos + 1).to_vec3_floored(),
            }) {
                return;
            }

            block_collisions.push(BLOCK_SHAPE.move_relative(item.pos.to_vec3_floored()));
            return;
        }

        let block_shape = self.get_block_shape(block_state, item.pos);

        let block_shape = block_shape.move_relative(item.pos.to_vec3_floored());
        // if the entity shape and block shape don't collide, continue
        if !Shapes::matches_anywhere(&block_shape, &self.entity_shape, |a, b| a && b) {
            return;
        }

        block_collisions.push(block_shape);
    }

    pub fn new(world: &'a World, aabb: &'a Aabb, context: EntityCollisionContext) -> Self {
        // saturating, not as Java: a position the server set can be far
        // outside i32 (the cast saturates, as Java's (int) cast in Mth.floor,
        // then `- 1` overflowed). Vanilla's int `- 1` and `+ 1` wrap there
        // (min - 1 becomes i32::MAX), which turns the box inside out; we
        // keep the box at the i32 edge instead. Either way no chunk is loaded
        // there (|x| or |z| near 2^31 is beyond the 30 000 000 world border
        // and every chunk position), so the blocks found are the same: none.
        // Cursor3d bounds the volume.
        let origin = BlockPos {
            x: ((aabb.min.x - EPSILON).floor() as i32).saturating_sub(1),
            y: ((aabb.min.y - EPSILON).floor() as i32).saturating_sub(1),
            z: ((aabb.min.z - EPSILON).floor() as i32).saturating_sub(1),
        };

        let end = BlockPos {
            x: ((aabb.max.x + EPSILON).floor() as i32).saturating_add(1),
            y: ((aabb.max.y + EPSILON).floor() as i32).saturating_add(1),
            z: ((aabb.max.z + EPSILON).floor() as i32).saturating_add(1),
        };

        let cursor = Cursor3d::new(origin, end);

        Self {
            world,
            aabb,
            entity_shape: VoxelShape::from(aabb),
            cursor,

            _context: context,

            cached_sections: Vec::new(),
            cached_block_shapes: Vec::new(),
        }
    }

    fn get_chunk(&self, block_x: i32, block_z: i32) -> Option<Arc<RwLock<Chunk>>> {
        let chunk_x = ChunkSectionPos::block_to_section_coord(block_x);
        let chunk_z = ChunkSectionPos::block_to_section_coord(block_z);
        let chunk_pos = ChunkPos::new(chunk_x, chunk_z);

        // TODO: minecraft caches chunk here
        // ```
        // int chunkX = SectionPos.blockToSectionCoord(blockX);
        // int chunkZ = SectionPos.blockToSectionCoord(blockZ);
        // long chunkPosLong = ChunkPos.asLong(chunkX, chunkZ);
        // if (this.cachedBlockGetter != null && this.cachedBlockGetterPos == var5) {
        //     return this.cachedBlockGetter;
        // } else {
        //    BlockGetter var7 = this.collisionGetter.getChunkForCollisions(chunkX, chunkZ);
        //    this.cachedBlockGetter = var7;
        //    this.cachedBlockGetterPos =
        //    chunkPosLong;
        //    return var7;
        // }
        // ```

        self.world.chunks.get(&chunk_pos)
    }

    fn get_block_state(&mut self, block_pos: BlockPos) -> BlockState {
        if block_pos.y < self.world.chunks.min_y() {
            // below the world
            return BlockState::AIR;
        }

        let section_pos = ChunkSectionPos::from(block_pos);
        let section_block_pos = ChunkSectionBlockPos::from(block_pos);

        for (cached_section_pos, cached_section) in &self.cached_sections {
            if section_pos == *cached_section_pos {
                return cached_section.get_block_state(section_block_pos);
            }
        }

        let chunk = self.get_chunk(block_pos.x, block_pos.z);
        let Some(chunk) = chunk else {
            return BlockState::AIR;
        };
        let chunk = chunk.read();

        let sections = &chunk.sections;
        let section_index =
            azalea_world::chunk::section_index(block_pos.y, self.world.chunks.min_y()) as usize;

        let Some(section) = sections.get(section_index) else {
            return BlockState::AIR;
        };

        self.cached_sections.push((section_pos, section.clone()));

        section.get_block_state(section_block_pos)
    }

    fn get_block_shape(
        &mut self,
        block_state: BlockState,
        pos: BlockPos,
    ) -> Cow<'static, VoxelShape> {
        // TODO (2026-05-06): does this cache still help? i'm pretty sure
        // getting shapes has been optimized since this cache was implemented
        for (cached_block_state, cached_shape) in &self.cached_block_shapes {
            if block_state == *cached_block_state {
                return Cow::Borrowed(cached_shape);
            }
        }

        let shape = block_state.collision_shape(pos);
        // if it has a random offset then we can't cache them, and it'll be a
        // Cow::Owned
        if let Cow::Borrowed(shape) = shape {
            self.cached_block_shapes.push((block_state, shape));
        }

        shape
    }
}

pub struct EntityCollisionContext {
    pub descending: bool,
    pub entity_bottom: f64,
    pub held_item: ItemStack,
    can_stand_on_fluid_predicate: CanStandOnFluidPredicate,
    pub entity: Option<Entity>,
}

impl EntityCollisionContext {
    pub fn of(entity: Option<Entity>) -> Self {
        Self {
            descending: false,
            entity_bottom: 0.0,
            held_item: ItemStack::Empty,
            can_stand_on_fluid_predicate: CanStandOnFluidPredicate::PassToEntity,
            entity,
        }
    }
    pub fn with_include_liquids(mut self, include_liquids: bool) -> Self {
        self.can_stand_on_fluid_predicate = if include_liquids {
            CanStandOnFluidPredicate::AlwaysTrue
        } else {
            CanStandOnFluidPredicate::PassToEntity
        };
        self
    }

    pub fn can_stand_on_fluid(&self, above: &FluidState, target: &FluidState) -> bool {
        self.can_stand_on_fluid_predicate.matches(target) && !above.is_same_kind(target)
    }
}

enum CanStandOnFluidPredicate {
    PassToEntity,
    AlwaysTrue,
}
impl CanStandOnFluidPredicate {
    pub fn matches(&self, _state: &FluidState) -> bool {
        match self {
            Self::AlwaysTrue => true,
            // minecraft sometimes returns true for striders here, false for
            // every other entity though
            Self::PassToEntity => false,
        }
    }
}

/// This basically gets all the chunks that an entity colliding with
/// that bounding box could be in.
///
/// This is forEachAccessibleNonEmptySection in vanilla Minecraft because they
/// sort entities into sections instead of just chunks. In theory this might be
/// a performance loss for Azalea. If this ever turns out to be a bottleneck,
/// then maybe you should try having it do that instead.
pub fn for_entities_in_chunks_colliding_with(
    world: &World,
    aabb: &Aabb,
    mut consumer: impl FnMut(ChunkPos, &HashSet<Entity>),
) {
    let min_section = ChunkSectionPos::from(aabb.min - Vec3::new(2., 4., 2.));
    let max_section = ChunkSectionPos::from(aabb.max + Vec3::new(2., 0., 2.));

    let min_chunk = ChunkPos::from(min_section);
    let max_chunk = ChunkPos::from(max_section);

    // A box from a huge server-set position or velocity can span billions of
    // chunks; then go through the chunks that have entities instead (as
    // vanilla's section storage does), so the cost is bounded either way.
    // Only for boxes over LARGE_BOX_CHUNKS (ordinary boxes, a few chunks,
    // take the walk below as before), and in the walk's order (x, then z):
    // the order is visible to callers (pick_entity keeps the first of
    // equally close entities, the last of those containing the eye), so it
    // must not depend on the box size or on the map's iteration order.
    const LARGE_BOX_CHUNKS: i64 = 1024;
    let area = (max_chunk.x as i64 - min_chunk.x as i64 + 1).max(0)
        * (max_chunk.z as i64 - min_chunk.z as i64 + 1).max(0);
    if area > LARGE_BOX_CHUNKS && area > world.entities_by_chunk.len() as i64 {
        let mut in_box = world
            .entities_by_chunk
            .iter()
            .filter(|(chunk_pos, _)| {
                (min_chunk.x..=max_chunk.x).contains(&chunk_pos.x)
                    && (min_chunk.z..=max_chunk.z).contains(&chunk_pos.z)
            })
            .collect::<Vec<_>>();
        in_box.sort_unstable_by_key(|(chunk_pos, _)| (chunk_pos.x, chunk_pos.z));
        for (&chunk_pos, entities) in in_box {
            consumer(chunk_pos, entities);
        }
        return;
    }

    for chunk_x in min_chunk.x..=max_chunk.x {
        for chunk_z in min_chunk.z..=max_chunk.z {
            let chunk_pos = ChunkPos::new(chunk_x, chunk_z);
            if let Some(entities) = world.entities_by_chunk.get(&chunk_pos) {
                consumer(chunk_pos, entities);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    // fuzz: handler_sequence (timeout): a box over a huge range of chunks
    // walked every chunk position
    #[test]
    fn entity_lookup_over_a_huge_box_is_bounded() {
        let mut world = World::default();
        world
            .entities_by_chunk
            .insert(ChunkPos::new(3, -4), HashSet::from([Entity::PLACEHOLDER]));
        let aabb = Aabb {
            min: Vec3::new(-1e9, 0., -1e9),
            max: Vec3::new(1e9, 10., 1e9),
        };
        let start = Instant::now();
        let mut seen = Vec::new();
        for_entities_in_chunks_colliding_with(&world, &aabb, |pos, _| seen.push(pos));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(seen, vec![ChunkPos::new(3, -4)]);

        // a small box still finds what's in range, and only that
        let aabb = Aabb {
            min: Vec3::new(40., 0., -70.),
            max: Vec3::new(60., 10., -60.),
        };
        let mut seen = Vec::new();
        for_entities_in_chunks_colliding_with(&world, &aabb, |pos, _| seen.push(pos));
        assert_eq!(seen, vec![ChunkPos::new(3, -4)]);
        let far = Aabb {
            min: Vec3::new(400., 0., 400.),
            max: Vec3::new(401., 1., 401.),
        };
        let mut seen = Vec::new();
        for_entities_in_chunks_colliding_with(&world, &far, |pos, _| seen.push(pos));
        assert!(seen.is_empty());
    }

    // fuzz: handler_sequence (crash): a position past i32 saturated in the
    // cast, then `- 1` overflowed
    #[test]
    fn block_collisions_at_a_position_past_i32_dont_overflow() {
        let world = World::default();
        let aabb = Aabb {
            min: Vec3::new(-1e300, -1e300, -1e300),
            max: Vec3::new(1e300, 1e300, 1e300),
        };
        let _ = BlockCollisionsState::new(&world, &aabb, EntityCollisionContext::of(None));
    }

    // the order the chunks are given in is the same for any box size and
    // doesn't come from the map's iteration order
    #[test]
    fn entity_lookup_order_is_the_walk_order_for_any_box() {
        let mut world = World::default();
        let mut chunks = Vec::new();
        for x in -3_i32..=3 {
            for z in -3..=3 {
                if (x * 7 + z * 3).rem_euclid(4) == 0 {
                    chunks.push(ChunkPos::new(x, z));
                }
            }
        }
        for (i, &c) in chunks.iter().enumerate() {
            world.entities_by_chunk.insert(
                c,
                HashSet::from([Entity::from_raw_u32(i as u32 + 1).unwrap()]),
            );
        }
        let walk_order = {
            let mut c = chunks.clone();
            c.sort_by_key(|c| (c.x, c.z));
            c
        };
        // small (sparse world: more chunk positions than chunks with
        // entities), and huge (the map is walked)
        for half in [50., 1e9] {
            let aabb = Aabb {
                min: Vec3::new(-half, 0., -half),
                max: Vec3::new(half, 10., half),
            };
            let mut seen = Vec::new();
            for_entities_in_chunks_colliding_with(&world, &aabb, |pos, _| seen.push(pos));
            assert_eq!(seen, walk_order, "half size {half}");
        }
    }
}
