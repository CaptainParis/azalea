use azalea_client::test_utils::prelude::*;
use azalea_core::decode_errors::decode_errors;
use azalea_entity::inventory::Inventory;
use azalea_inventory::ItemStack;
use azalea_protocol::packets::{
    ConnectionProtocol,
    game::{ClientboundSetCursorItem, ClientboundSetPlayerInventory},
};
use azalea_registry::builtin::ItemKind;

// 1.21.4 handleSetCursorItem: `player.containerMenu.setCarried(contents)`
#[test]
fn test_set_cursor_item() {
    let _lock = init();

    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();

    simulation.receive_packet(ClientboundSetCursorItem {
        contents: ItemStack::new(ItemKind::Stone, 3),
    });
    simulation.tick();
    assert_eq!(
        simulation.component::<Inventory>().carried,
        ItemStack::new(ItemKind::Stone, 3)
    );

    simulation.receive_packet(ClientboundSetCursorItem {
        contents: ItemStack::Empty,
    });
    simulation.tick();
    assert_eq!(
        simulation.component::<Inventory>().carried,
        ItemStack::Empty
    );
}

// 1.21.4 handleSetPlayerInventory: `player.getInventory().setItem(slot,
// contents)`, in the Inventory's index space (hotbar 0..9, main 9..36, armor
// 36..40 feet first, offhand 40)
#[test]
fn test_set_player_inventory() {
    let _lock = init();

    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();

    for (slot, kind) in [
        (0, ItemKind::Elytra),
        (20, ItemKind::Dirt),
        (38, ItemKind::DiamondChestplate),
        (40, ItemKind::TotemOfUndying),
    ] {
        simulation.receive_packet(ClientboundSetPlayerInventory {
            slot,
            contents: ItemStack::new(kind, 1),
        });
    }
    simulation.tick();

    let inventory = simulation.component::<Inventory>();
    let menu = &inventory.inventory_menu;
    assert_eq!(menu.slot(36), Some(&ItemStack::new(ItemKind::Elytra, 1)));
    assert_eq!(menu.slot(20), Some(&ItemStack::new(ItemKind::Dirt, 1)));
    assert_eq!(
        menu.slot(6),
        Some(&ItemStack::new(ItemKind::DiamondChestplate, 1))
    );
    assert_eq!(
        menu.slot(45),
        Some(&ItemStack::new(ItemKind::TotemOfUndying, 1))
    );
    // the selected hotbar slot is 0, so that's what's in the main hand
    assert_eq!(inventory.held_item(), &ItemStack::new(ItemKind::Elytra, 1));

    // a negative slot (vanilla throws and disconnects): dropped and counted
    let slots = menu.slots();
    let before = decode_errors();
    simulation.receive_packet(ClientboundSetPlayerInventory {
        slot: -1i32 as u32,
        contents: ItemStack::new(ItemKind::Stone, 1),
    });
    tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
        simulation.tick();
    });
    assert_eq!(
        simulation.component::<Inventory>().inventory_menu.slots(),
        slots
    );
    assert!(decode_errors().packet_handler > before.packet_handler);
}
