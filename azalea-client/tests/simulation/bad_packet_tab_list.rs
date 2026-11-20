//! Bad packet policy, tab list: a tab list update that can't be decoded isn't
//! state-free (player entities take their profile from it), so the tab list
//! is marked stale until the next login, counted and alerted the first time.
//! A player that spawns without a tab list entry is kept, marked
//! `MissingProfile` (and counted), and gets its profile when the entry comes.

use azalea_auth::game_profile::GameProfile;
use azalea_client::{
    bad_packet::{
        AlertReason, BadPacketAction, BadPacketAlert, BadPacketEvent, MissingProfile, PacketClass,
        TabListStale,
    },
    connection::RawConnection,
    player::GameProfileComponent,
    test_utils::prelude::*,
};
use azalea_core::{decode_errors::decode_errors, entity_id::MinecraftEntityId};
use azalea_entity::indexing::EntityIdIndex;
use azalea_protocol::packets::{
    ConnectionProtocol,
    game::{
        ClientboundPlayerInfoUpdate,
        c_player_info_update::{ActionEnumSet, PlayerInfoEntry},
    },
};
use azalea_registry::builtin::EntityKind;
use bevy_ecs::entity::Entity;
use uuid::Uuid;

const OTHER: Uuid = Uuid::from_u128(0x5555);

fn add_player_info(uuid: Uuid, name: &str) -> ClientboundPlayerInfoUpdate {
    ClientboundPlayerInfoUpdate {
        actions: ActionEnumSet {
            add_player: true,
            initialize_chat: false,
            update_game_mode: false,
            update_listed: true,
            update_latency: false,
            update_display_name: false,
            update_list_order: false,
            update_hat: false,
        },
        entries: vec![PlayerInfoEntry {
            profile: GameProfile::new(uuid, name.to_owned()),
            listed: true,
            ..Default::default()
        }],
    }
}

fn entity(simulation: &Simulation, id: i32) -> Option<Entity> {
    simulation
        .app
        .world()
        .get::<EntityIdIndex>(simulation.entity)?
        .get_by_minecraft_entity(MinecraftEntityId(id))
}

#[test]
fn bad_tab_list_update_marks_it_stale_and_alerts_once() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    let events = simulation.record_messages::<BadPacketEvent>();
    let alerts = simulation.record_messages::<BadPacketAlert>();
    let before = decode_errors();

    // the actions and the count are there, the entry is cut in its uuid
    simulation.receive_raw_packet(truncated_packet(add_player_info(OTHER, "Other"), 4));
    simulation.tick();

    assert_eq!(decode_errors().tab_list_stale, before.tab_list_stale + 1);
    assert_eq!(
        decode_errors().bad_packet_dropped,
        before.bad_packet_dropped
    );
    assert_eq!(simulation.component::<TabListStale>().drops, 1);
    {
        let events = events.lock();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].class, Some(PacketClass::TabList));
        assert_eq!(events[0].action, BadPacketAction::TabListStale);
        let alerts = alerts.lock();
        assert_eq!(alerts.len(), 1, "alerted the first time: {alerts:?}");
        assert_eq!(alerts[0].reason, AlertReason::ModelGap);
    }

    // a player spawns whose entry we lost: kept, marked, counted
    let mut add = make_basic_add_entity(EntityKind::Player, 50, (0.5, 64., 0.5));
    add.uuid = OTHER;
    simulation.receive_packet(add);
    simulation.tick();
    simulation.tick();
    let e = entity(&simulation, 50).expect("the player is kept");
    let w = simulation.app.world();
    assert_eq!(
        w.get::<MissingProfile>(e),
        Some(&MissingProfile {
            tab_list_stale: true
        })
    );
    assert!(w.get::<GameProfileComponent>(e).is_none());
    assert_eq!(decode_errors().missing_profile, before.missing_profile + 1);

    // a second bad one: still stale, the alert is the usual repeat alert
    simulation.receive_raw_packet(truncated_packet(add_player_info(OTHER, "Other"), 4));
    simulation.tick();
    assert_eq!(simulation.component::<TabListStale>().drops, 2);
    {
        let alerts = alerts.lock();
        assert_eq!(alerts.len(), 2, "{alerts:?}");
        assert_eq!(alerts[1].reason, AlertReason::Repeated);
    }

    // the entry comes after all: the player gets its profile
    simulation.receive_packet(add_player_info(OTHER, "Other"));
    simulation.tick();
    simulation.tick();
    let w = simulation.app.world();
    assert!(w.get::<MissingProfile>(e).is_none());
    assert_eq!(w.get::<GameProfileComponent>(e).unwrap().name, "Other");

    // a new login sends the whole list again: not stale anymore
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    simulation.tick();
    assert!(!simulation.has_component::<TabListStale>());
    assert!(
        simulation
            .app
            .world()
            .get::<RawConnection>(simulation.entity)
            .unwrap()
            .is_alive()
    );
}

#[test]
fn player_without_a_tab_list_entry_is_marked_even_with_a_good_tab_list() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    let before = decode_errors();
    let mut add = make_basic_add_entity(EntityKind::Player, 51, (0.5, 64., 0.5));
    add.uuid = OTHER;
    simulation.receive_packet(add);
    simulation.tick();
    simulation.tick();
    let e = entity(&simulation, 51).expect("kept");
    assert_eq!(
        simulation.app.world().get::<MissingProfile>(e),
        Some(&MissingProfile {
            tab_list_stale: false
        })
    );
    assert_eq!(decode_errors().missing_profile, before.missing_profile + 1);
    // a non-player without an entry isn't marked
    simulation.receive_packet(make_basic_add_entity(EntityKind::Cow, 52, (0.5, 64., 0.5)));
    simulation.tick();
    simulation.tick();
    let cow = entity(&simulation, 52).unwrap();
    assert!(simulation.app.world().get::<MissingProfile>(cow).is_none());
}
