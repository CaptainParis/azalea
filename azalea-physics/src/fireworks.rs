//! Firework rocket boosting while fall flying.

use azalea_core::{entity_id::MinecraftEntityId, position::Vec3};
use azalea_entity::{
    LocalEntity, LookDirection, Physics,
    metadata::{AttachedToTarget, FallFlying, FireworkRocket},
    view_vector,
};
use azalea_world::WorldName;
use bevy_ecs::prelude::*;

/// 1.21.4 `FireworkRocketEntity.tick`: a rocket attached to a fall-flying entity
/// pushes it along its look direction every tick, towards 1.5 blocks per tick.
///
/// Vanilla runs this on the client too, and for the local player it's the only
/// place the boost happens (the player's movement is client-side), so without
/// it a rocket used while gliding does nothing for us.
///
/// The rocket ticks after the player in the client level (it's added later), so
/// this runs after the player's physics and the boost shows up in the next
/// tick's movement.
#[allow(clippy::type_complexity)]
pub fn boost_from_attached_fireworks(
    rockets: Query<(&AttachedToTarget, &WorldName), With<FireworkRocket>>,
    mut players: Query<
        (
            &MinecraftEntityId,
            &WorldName,
            &FallFlying,
            &LookDirection,
            &mut Physics,
        ),
        With<LocalEntity>,
    >,
) {
    for (attached, rocket_world) in &rockets {
        let Some(target) = (attached.0).0 else {
            continue;
        };
        for (id, world, fall_flying, look, mut physics) in &mut players {
            if id.0 as u32 != target || world != rocket_world || !**fall_flying {
                continue;
            }
            let look = view_vector(*look);
            let v = physics.velocity;
            physics.velocity = v + Vec3::new(
                look.x * 0.1 + (look.x * 1.5 - v.x) * 0.5,
                look.y * 0.1 + (look.y * 1.5 - v.y) * 0.5,
                look.z * 0.1 + (look.z * 1.5 - v.z) * 0.5,
            );
        }
    }
}
