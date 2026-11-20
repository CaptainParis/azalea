//! Bad packet policy, inventory class: a container packet that can't be
//! decoded makes the inventory unknown and sends the resync click (slot -1,
//! PICKUP, button 0, a stale state id) for the container we believe is open.
//! A reflex click (totem swap) still runs at once, an ordinary click is held
//! and, when the full content clears "unknown" (keeping the carried item),
//! cancelled with `ClickCancelledByResync` instead of replayed; with no resync
//! the click is retried once, then the connection is closed. A bad
//! `open_screen` resyncs the container id read from its body (or closes a menu
//! it can't model), and a reflex for the player's inventory while a container
//! is open closes the container first.

use std::time::Duration;

use azalea_client::{
    bad_packet::{
        AlertReason, BadPacketAction, BadPacketAlert, BadPacketConfig, BadPacketEvent,
        ClickCancelledByResync, InventoryUnknown, PacketClass, PendingClicks,
    },
    inventory::ContainerClickEvent,
    test_utils::prelude::*,
};
use azalea_core::decode_errors::decode_errors;
use azalea_entity::inventory::Inventory;
use azalea_inventory::{
    ItemStack,
    operations::{ClickType, PickupClick, SwapClick},
};
use azalea_protocol::packets::{
    ConnectionProtocol,
    game::{ClientboundContainerSetContent, ClientboundContainerSetSlot, ServerboundGamePacket},
};
use azalea_registry::builtin::ItemKind;

fn full_inventory(state_id: u32, carried: ItemStack) -> ClientboundContainerSetContent {
    let mut items = vec![ItemStack::Empty; 46];
    // hotbar 0: a totem; main inventory: dirt
    items[36] = ItemStack::new(ItemKind::TotemOfUndying, 1);
    items[9] = ItemStack::new(ItemKind::Dirt, 5);
    ClientboundContainerSetContent {
        container_id: 0,
        state_id,
        items,
        carried_item: carried,
    }
}

fn bad_slot_packet() -> Box<[u8]> {
    truncated_packet(
        ClientboundContainerSetSlot {
            container_id: 0,
            state_id: 9,
            slot: 10,
            item_stack: ItemStack::new(ItemKind::Stone, 1),
        },
        2,
    )
}

fn setup() -> (Simulation, SentPackets) {
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    simulation.receive_packet(full_inventory(5, ItemStack::new(ItemKind::Stick, 2)));
    simulation.tick();
    let sent = SentPackets::new(&mut simulation);
    (simulation, sent)
}

fn clicks(sent: &SentPackets) -> Vec<azalea_protocol::packets::game::ServerboundContainerClick> {
    let mut out = Vec::new();
    while let Some(p) = sent.next() {
        if let ServerboundGamePacket::ContainerClick(c) = p {
            out.push(c);
        }
    }
    out
}

fn is_resync_click(
    c: &azalea_protocol::packets::game::ServerboundContainerClick,
    stale_state: u32,
) -> bool {
    c.container_id == 0
        && c.slot_num == -1
        && c.button_num == 0
        && c.click_type == ClickType::Pickup
        && c.state_id == stale_state
        && c.changed_slots.is_empty()
}

#[test]
fn bad_container_packet_resyncs_the_inventory() {
    let _lock = init_expecting_errors();
    let (mut simulation, sent) = setup();
    let events = simulation.record_messages::<BadPacketEvent>();
    let before = decode_errors();

    simulation.receive_raw_packet(bad_slot_packet());
    simulation.tick();

    assert_eq!(
        decode_errors().inventory_unknown,
        before.inventory_unknown + 1
    );
    assert!(simulation.has_component::<InventoryUnknown>());
    {
        let events = events.lock();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].class, Some(PacketClass::Inventory));
        assert_eq!(
            events[0].action,
            BadPacketAction::InventoryResync { container_id: 0 }
        );
    }
    // the resync click, with a state id the server can't have (ours is 5)
    let c = clicks(&sent);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(is_resync_click(&c[0], 4), "{:?}", c[0]);
    // it carries what we believe we carry
    assert_eq!(c[0].carried_item, ItemStack::new(ItemKind::Stick, 2));

    // an ordinary click is held
    let cancelled = simulation.record_messages::<ClickCancelledByResync>();
    simulation.trigger(ContainerClickEvent::new(
        simulation.entity,
        0,
        PickupClick::Left { slot: Some(9) },
    ));
    simulation.tick();
    assert!(clicks(&sent).is_empty());
    assert_eq!(simulation.component::<PendingClicks>().0.len(), 1);

    // a reflex (totem from hotbar 0 to the offhand) runs at once
    simulation.trigger(ContainerClickEvent::reflex(
        simulation.entity,
        0,
        SwapClick {
            source_slot: 36,
            target_slot: 40,
        },
    ));
    simulation.tick();
    let c = clicks(&sent);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].click_type, ClickType::Swap);
    assert_eq!(c[0].slot_num, 36);
    assert_eq!(c[0].button_num, 40);
    assert_eq!(
        simulation.component::<Inventory>().inventory_menu.slots()[45],
        ItemStack::new(ItemKind::TotemOfUndying, 1)
    );

    // the server's full state (as broadcastFullState: everything and the
    // carried item, a new state id) makes the inventory known; the held click
    // isn't replayed blindly, it's cancelled
    let mut full = full_inventory(7, ItemStack::new(ItemKind::Stick, 2));
    full.items.swap(36, 45);
    simulation.receive_packet(full);
    simulation.tick();
    assert!(!simulation.has_component::<InventoryUnknown>());
    let inv = simulation.component::<Inventory>();
    assert_eq!(inv.state_id, 7);
    assert!(clicks(&sent).is_empty(), "the held click isn't sent");
    assert!(simulation.component::<PendingClicks>().0.is_empty());
    let operation = {
        let cancelled = cancelled.lock();
        assert_eq!(cancelled.len(), 1, "{cancelled:?}");
        assert!(cancelled[0].resynced);
        assert_eq!(cancelled[0].window_id, 0);
        cancelled[0].operation.clone()
    };
    // the caller looks at the real content (slot 9 still has the dirt) and
    // clicks again
    assert_eq!(
        simulation.component::<Inventory>().inventory_menu.slots()[9],
        ItemStack::new(ItemKind::Dirt, 5)
    );
    simulation.trigger(ContainerClickEvent::new(simulation.entity, 0, operation));
    simulation.tick();
    let c = clicks(&sent);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].click_type, ClickType::Pickup);
    assert_eq!(c[0].slot_num, 9);
    assert_eq!(c[0].state_id, 7);
    // the carried item wasn't lost: the click put the stick in slot 9 and
    // picked the dirt up (a swap of cursor and slot)
    let inv = simulation.component::<Inventory>();
    assert_eq!(inv.carried, ItemStack::new(ItemKind::Dirt, 5));
    assert_eq!(
        inv.inventory_menu.slots()[9],
        ItemStack::new(ItemKind::Stick, 2)
    );
    assert_eq!(
        decode_errors().inventory_resync_failed,
        before.inventory_resync_failed
    );
    assert_eq!(decode_errors().policy_close, before.policy_close);
}

#[test]
fn inventory_resync_retries_once_then_closes() {
    let _lock = init_expecting_errors();
    let (mut simulation, sent) = setup();
    simulation.app.insert_resource(BadPacketConfig {
        resync_retry: Duration::from_millis(200),
        resync_give_up: Duration::from_millis(500),
        ..Default::default()
    });
    let alerts = simulation.record_messages::<BadPacketAlert>();
    let disconnects = simulation.record_disconnects();
    let before = decode_errors();

    let cancelled = simulation.record_messages::<ClickCancelledByResync>();
    simulation.receive_raw_packet(bad_slot_packet());
    simulation.tick();
    assert_eq!(clicks(&sent).len(), 1);
    simulation.trigger(ContainerClickEvent::new(
        simulation.entity,
        0,
        PickupClick::Left { slot: Some(9) },
    ));
    // a second bad container packet while unknown sends no second click
    simulation.receive_raw_packet(bad_slot_packet());
    simulation.tick();
    assert!(clicks(&sent).is_empty());

    std::thread::sleep(Duration::from_millis(250));
    simulation.tick();
    simulation.tick();
    let c = clicks(&sent);
    assert_eq!(c.len(), 1, "one retry: {c:?}");
    assert!(is_resync_click(&c[0], 4));
    assert!(disconnects.lock().is_empty());

    std::thread::sleep(Duration::from_millis(300));
    simulation.tick();
    simulation.tick();
    simulation.tick();
    assert!(clicks(&sent).is_empty(), "no third click");
    let after = decode_errors();
    assert_eq!(
        after.inventory_resync_failed,
        before.inventory_resync_failed + 1
    );
    assert_eq!(after.policy_close, before.policy_close + 1);
    let disconnects = disconnects.lock();
    assert_eq!(disconnects.len(), 1, "{disconnects:?}");
    assert!(
        disconnects[0]
            .as_deref()
            .unwrap_or_default()
            .contains("inventory still unknown")
    );
    let alerts = alerts.lock();
    assert!(
        alerts.iter().any(|a| a.reason == AlertReason::ResyncFailed),
        "{alerts:?}"
    );
    // the held click was cancelled, not sent
    let cancelled = cancelled.lock();
    assert_eq!(cancelled.len(), 1, "{cancelled:?}");
    assert!(!cancelled[0].resynced);
    // the second bad packet with the same id alerted too
    assert!(alerts.iter().any(|a| a.reason == AlertReason::Repeated));
}

fn bad_open_screen(keep: usize) -> Box<[u8]> {
    truncated_packet(
        azalea_protocol::packets::game::ClientboundOpenScreen {
            container_id: 3,
            menu_type: azalea_registry::builtin::MenuKind::Generic9x3,
            title: "Chest".into(),
        },
        keep,
    )
}

fn sent_closes(
    sent: &SentPackets,
) -> (
    Vec<i32>,
    Vec<azalea_protocol::packets::game::ServerboundContainerClick>,
) {
    let mut closes = Vec::new();
    let mut clicks = Vec::new();
    while let Some(p) = sent.next() {
        match p {
            ServerboundGamePacket::ContainerClose(c) => closes.push(c.container_id),
            ServerboundGamePacket::ContainerClick(c) => clicks.push(c),
            _ => {}
        }
    }
    (closes, clicks)
}

#[test]
fn bad_open_screen_opens_the_menu_and_resyncs_its_id() {
    let _lock = init_expecting_errors();
    let (mut simulation, sent) = setup();
    let events = simulation.record_messages::<BadPacketEvent>();

    // id (1 byte) and menu type (1 byte) are there, the title is cut
    simulation.receive_raw_packet(bad_open_screen(3));
    simulation.tick();

    let inv = simulation.component::<Inventory>();
    assert_eq!(inv.id, 3);
    assert!(
        inv.container_menu.is_some(),
        "the menu is open, as on the server"
    );
    assert!(simulation.has_component::<InventoryUnknown>());
    assert_eq!(
        events.lock()[0].action,
        BadPacketAction::InventoryResync { container_id: 3 }
    );
    let (closes, c) = sent_closes(&sent);
    assert!(closes.is_empty());
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].container_id, 3);
    assert_eq!(c[0].slot_num, -1);

    // a reflex written for the player's inventory (container 0) while the
    // server has container 3 open: the container is closed first, then the
    // click goes to container 0, where the server takes it
    simulation.trigger(ContainerClickEvent::reflex(
        simulation.entity,
        0,
        SwapClick {
            source_slot: 36,
            target_slot: 40,
        },
    ));
    simulation.tick();
    let mut order = Vec::new();
    while let Some(p) = sent.next() {
        match p {
            ServerboundGamePacket::ContainerClose(c) => order.push(("close", c.container_id)),
            ServerboundGamePacket::ContainerClick(c) => order.push(("click", c.container_id)),
            _ => {}
        }
    }
    assert_eq!(order, vec![("close", 3), ("click", 0)]);
    let inv = simulation.component::<Inventory>();
    assert_eq!(inv.id, 0);
    assert_eq!(
        inv.inventory_menu.slots()[45],
        ItemStack::new(ItemKind::TotemOfUndying, 1)
    );

    // the click resyncs the inventory (its state id is stale): known again
    let mut full = full_inventory(9, ItemStack::Empty);
    full.items.swap(36, 45);
    simulation.receive_packet(full);
    simulation.tick();
    assert!(!simulation.has_component::<InventoryUnknown>());
}

#[test]
fn bad_open_screen_with_only_its_id_closes_it_and_resyncs_the_inventory() {
    let _lock = init_expecting_errors();
    let (mut simulation, sent) = setup();
    let events = simulation.record_messages::<BadPacketEvent>();

    simulation.receive_raw_packet(bad_open_screen(1));
    simulation.tick();

    let inv = simulation.component::<Inventory>();
    assert_eq!(inv.id, 0);
    assert!(inv.container_menu.is_none());
    assert!(simulation.has_component::<InventoryUnknown>());
    assert_eq!(
        events.lock()[0].action,
        BadPacketAction::InventoryResync { container_id: 0 }
    );
    // closed as vanilla's closeContainer, then the resync for container 0
    let (closes, c) = sent_closes(&sent);
    assert_eq!(closes, vec![3]);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(is_resync_click(&c[0], 4), "{:?}", c[0]);
}
