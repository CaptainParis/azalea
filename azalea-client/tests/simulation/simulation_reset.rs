//! `Simulation::reset` (used by the fuzz targets to reuse one app): after a
//! session with a world, a chunk, an entity, a tab list entry and a bad
//! packet's marks, the reset client is a fresh one, and a second session
//! plays the same way as the first.

use azalea_client::{
    bad_packet::TabListStale, local_player::TabListResource, test_utils::prelude::*,
};
use azalea_core::{entity_id::MinecraftEntityId, position::ChunkPos};
use azalea_entity::indexing::EntityIdIndex;
use azalea_protocol::packets::ConnectionProtocol;
use azalea_registry::builtin::EntityKind;
use azalea_world::{WorldName, Worlds};

fn session(simulation: &mut Simulation) {
    simulation.receive_packet(default_login_packet());
    simulation.receive_packet(make_basic_empty_chunk(ChunkPos::new(0, 0), (384 + 64) / 16));
    simulation.tick();
    simulation.receive_packet(make_basic_add_entity(EntityKind::Cow, 123, (0.5, 64., 0.5)));
    simulation.tick();
    simulation.tick();
    assert!(simulation.has_component::<WorldName>());
    assert!(simulation.chunk(ChunkPos::new(0, 0)).is_some());
    let index = simulation
        .app
        .world()
        .get::<EntityIdIndex>(simulation.entity)
        .unwrap();
    assert!(
        index
            .get_by_minecraft_entity(MinecraftEntityId(123))
            .is_some()
    );
}

#[test]
fn reset_gives_a_fresh_client() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    session(&mut simulation);
    // a mark from the bad packet policy
    let first = simulation.entity;
    simulation
        .app
        .world_mut()
        .entity_mut(first)
        .insert(TabListStale {
            since: std::time::Instant::now(),
            drops: 1,
        });

    simulation.reset(ConnectionProtocol::Game);

    assert_ne!(simulation.entity, first);
    assert!(simulation.app.world().get_entity(first).is_err());
    assert!(!simulation.has_component::<WorldName>());
    assert!(!simulation.has_component::<TabListStale>());
    // nothing of the old session's game entities is left
    let mut q = simulation.app.world_mut().query::<&MinecraftEntityId>();
    assert_eq!(q.iter(simulation.app.world()).count(), 0);
    assert!(simulation.resource::<TabListResource>().is_empty());
    simulation.with_resource::<Worlds>(|worlds| assert!(worlds.map.is_empty()));

    // and it plays like the first one
    session(&mut simulation);
}
