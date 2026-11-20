pub mod equipment_effects;

use azalea_chat::FormattedText;
use azalea_core::tick::GameTick;
use azalea_entity::{PlayerAbilities, inventory::Inventory as Inv};
use azalea_inventory::operations::ClickOperation;
pub use azalea_inventory::*;
use azalea_protocol::packets::game::{
    s_container_click::ServerboundContainerClick, s_container_close::ServerboundContainerClose,
    s_set_carried_item::ServerboundSetCarriedItem,
};
use azalea_registry::builtin::MenuKind;
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use indexmap::IndexMap;
use tracing::{error, warn};

use crate::{
    bad_packet::InventoryUnknown,
    inventory::equipment_effects::{collect_equipment_changes, handle_equipment_changes},
    packet::game::SendGamePacketEvent,
};

// TODO: when this is removed, remove the Inv alias above (which just exists to
// avoid conflicting with this pub deprecated type)
#[doc(hidden)]
#[deprecated = "moved to `azalea_entity::inventory::Inventory`."]
pub type Inventory = azalea_entity::inventory::Inventory;

pub struct InventoryPlugin;
impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            GameTick,
            (
                ensure_has_sent_carried_item.after(super::mining::handle_mining_queued),
                collect_equipment_changes
                    .after(super::interact::handle_start_use_item_queued)
                    .before(azalea_physics::ai_step),
            ),
        )
        .add_observer(handle_client_side_close_container_trigger)
        .add_observer(handle_menu_opened_trigger)
        .add_observer(handle_container_close_event)
        .add_observer(handle_set_container_content_trigger)
        .add_observer(handle_container_click_event)
        // number keys are checked on tick but scrolling can happen outside of ticks, therefore
        // this is fine
        .add_observer(handle_set_selected_hotbar_slot_event)
        .add_observer(handle_equipment_changes);
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, SystemSet)]
pub struct InventorySystems;

/// A Bevy trigger that's fired when our client should show a new screen (like a
/// chest or crafting table).
///
/// To watch for the menu being closed, you could use
/// [`ClientsideCloseContainerEvent`]. To close it manually, use
/// [`CloseContainerEvent`].
#[derive(Clone, Debug, EntityEvent)]
pub struct MenuOpenedEvent {
    pub entity: Entity,
    pub window_id: i32,
    pub menu_type: MenuKind,
    pub title: FormattedText,
}
fn handle_menu_opened_trigger(event: On<MenuOpenedEvent>, mut query: Query<&mut Inv>) {
    let Ok(mut inventory) = query.get_mut(event.entity) else {
        warn!("Got MenuOpenedEvent for an entity without an inventory");
        return;
    };
    inventory.id = event.window_id;
    inventory.container_menu = Some(Menu::from_kind(event.menu_type));
    inventory.container_menu_title = Some(event.title.clone());
}

/// Tell the server that we want to close a container.
///
/// Note that this is also sent when the client closes its own inventory, even
/// though there is no packet for opening its inventory.
#[derive(EntityEvent)]
pub struct CloseContainerEvent {
    pub entity: Entity,
    /// The ID of the container to close. 0 for the player's inventory.
    ///
    /// If this is not the same as the currently open inventory, nothing will
    /// happen.
    pub id: i32,
}
fn handle_container_close_event(
    close_container: On<CloseContainerEvent>,
    mut commands: Commands,
    query: Query<(Entity, &Inv)>,
) {
    let Ok((entity, inventory)) = query.get(close_container.entity) else {
        warn!("Tried to close a container, but the client has no inventory");
        return;
    };
    if close_container.id != inventory.id {
        warn!(
            "Tried to close container with ID {}, but the current container ID is {}",
            close_container.id, inventory.id
        );
        return;
    }

    commands.trigger(SendGamePacketEvent::new(
        entity,
        ServerboundContainerClose {
            container_id: inventory.id,
        },
    ));
    commands.trigger(ClientsideCloseContainerEvent {
        entity: close_container.entity,
    });
}

/// A Bevy event that's fired when our client closed a container.
///
/// This can also be triggered directly to close a container silently without
/// sending any packets to the server. You probably don't want that though, and
/// should instead use [`CloseContainerEvent`].
///
/// If you want to watch for a container being opened, you should use
/// [`MenuOpenedEvent`].
#[derive(Clone, EntityEvent)]
pub struct ClientsideCloseContainerEvent {
    pub entity: Entity,
}
pub fn handle_client_side_close_container_trigger(
    event: On<ClientsideCloseContainerEvent>,
    mut query: Query<&mut Inv>,
) {
    let Ok(mut inventory) = query.get_mut(event.entity) else {
        warn!("Got ClientsideCloseContainerEvent for an entity without an inventory");
        return;
    };

    // copy the Player part of the container_menu to the inventory_menu
    if let Some(inventory_menu) = inventory.container_menu.take() {
        // this isn't the same as what vanilla does. i believe vanilla
        // synchronizes the slots between inventoryMenu and containerMenu by
        // just having the player slots point to the same ItemStack in memory,
        // but emulating this in rust would require us to wrap our `ItemStack`s
        // as `Arc<Mutex<ItemStack>>` which would have kinda terrible
        // ergonomics.

        // the simpler solution i chose to go with here is to only copy the
        // player slots when the container is closed. this is perfectly fine for
        // vanilla, but it might cause issues if a server modifies id 0 while we
        // have a container open...

        // if we do encounter this issue in the wild then the simplest solution
        // would probably be to just add logic for updating the container_menu
        // when the server tries to modify id 0 for slots within `inventory`.
        // not implemented for now because i'm not sure if that's worth worrying
        // about.

        // network-safe: player_slots_range() is a range of this menu's own
        // slots
        let new_inventory = inventory_menu.slots()[inventory_menu.player_slots_range()].to_vec();
        // network-safe: the player slots of every menu are the 36 of the player
        // inventory
        let new_inventory = <[ItemStack; 36]>::try_from(new_inventory).unwrap();
        *inventory.inventory_menu.as_player_mut().inventory = new_inventory;
    }

    inventory.id = 0;
    inventory.container_menu_title = None;
    // back to the inventory menu and its own state id
    inventory.state_id = inventory.inventory_menu_state_id;
}

#[derive(Debug, EntityEvent)]
pub struct ContainerClickEvent {
    pub entity: Entity,
    pub window_id: i32,
    pub operation: ClickOperation,
    /// A survival reflex (for example a totem swap): it runs even while the
    /// inventory is unknown after a bad packet, and its result resyncs the
    /// state. Ordinary clicks (`false`) made while the inventory is unknown
    /// are held and then cancelled, never sent: see
    /// [`ClickCancelledByResync`](crate::bad_packet::ClickCancelledByResync).
    pub reflex: bool,
}
impl ContainerClickEvent {
    /// An ordinary click.
    pub fn new(entity: Entity, window_id: i32, operation: impl Into<ClickOperation>) -> Self {
        Self {
            entity,
            window_id,
            operation: operation.into(),
            reflex: false,
        }
    }
    /// A reflex click: never waits for an inventory resync.
    pub fn reflex(entity: Entity, window_id: i32, operation: impl Into<ClickOperation>) -> Self {
        Self {
            reflex: true,
            ..Self::new(entity, window_id, operation)
        }
    }
}
pub fn handle_container_click_event(
    container_click: On<ContainerClickEvent>,
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &mut Inv,
        Option<&PlayerAbilities>,
        Has<InventoryUnknown>,
    )>,
) {
    let Ok((entity, mut inventory, player_abilities, unknown)) =
        query.get_mut(container_click.entity)
    else {
        warn!("Tried to click a container, but the client has no inventory");
        return;
    };
    if unknown && !container_click.reflex {
        // held, then cancelled when the inventory resync ends (bad packet
        // policy): it was decided on a model that may be wrong
        let (window_id, operation) = (container_click.window_id, container_click.operation.clone());
        commands.queue(move |ecs: &mut World| {
            crate::bad_packet::defer_click(ecs, entity, window_id, operation);
        });
        return;
    }
    if container_click.reflex && container_click.window_id == 0 && inventory.id != 0 {
        // a reflex written for the player's inventory while a container is
        // open (perhaps one the server opened with a packet we couldn't
        // decode): the server ignores clicks for another container id, so
        // close the container first, as a player would, then click
        let open = inventory.id;
        commands.trigger(CloseContainerEvent { entity, id: open });
        commands.trigger(ContainerClickEvent {
            entity,
            window_id: 0,
            operation: container_click.operation.clone(),
            reflex: true,
        });
        return;
    }
    if inventory.id != container_click.window_id {
        error!(
            "Tried to click container with ID {}, but the current container ID is {}. Click packet won't be sent.",
            container_click.window_id, inventory.id
        );
        return;
    }

    let old_slots = inventory.menu().slots();
    inventory.simulate_click(
        &container_click.operation,
        player_abilities.unwrap_or(&PlayerAbilities::default()),
    );
    let new_slots = inventory.menu().slots();

    // see which slots changed after clicking and put them in the map the server
    // uses this to check if we desynced. 1.21.4
    // MultiPlayerGameMode.handleInventoryMouseClick sends copies of the full
    // item stacks (1.21.5 switched to hashed stacks).
    let mut changed_slots: IndexMap<u16, ItemStack> = IndexMap::new();
    for (slot_index, (old_slot, new_slot)) in old_slots.iter().zip(&new_slots).enumerate() {
        if old_slot != new_slot {
            changed_slots.insert(slot_index as u16, new_slot.clone());
        }
    }

    commands.trigger(SendGamePacketEvent::new(
        entity,
        ServerboundContainerClick {
            container_id: container_click.window_id,
            state_id: inventory.state_id,
            slot_num: container_click
                .operation
                .slot_num()
                .map(|n| n as i16)
                .unwrap_or(-999),
            button_num: container_click.operation.button_num(),
            click_type: container_click.operation.click_type(),
            changed_slots,
            carried_item: inventory.carried.clone(),
        },
    ));
}

/// Sent from the server when the contents of a container are replaced.
///
/// Usually triggered by the `ContainerSetContent` packet.
#[derive(EntityEvent)]
pub struct SetContainerContentEvent {
    pub entity: Entity,
    pub slots: Vec<ItemStack>,
    pub container_id: i32,
}
pub fn handle_set_container_content_trigger(
    set_container_content: On<SetContainerContentEvent>,
    mut query: Query<&mut Inv>,
) {
    let Ok(mut inventory) = query.get_mut(set_container_content.entity) else {
        warn!("Got SetContainerContentEvent for an entity without an inventory");
        return;
    };

    if set_container_content.container_id != inventory.id {
        warn!(
            "Got SetContainerContentEvent for container with ID {}, but the current container ID is {}",
            set_container_content.container_id, inventory.id
        );
        return;
    }

    let menu = inventory.menu_mut();
    for (i, slot) in set_container_content.slots.iter().enumerate() {
        if let Some(slot_mut) = menu.slot_mut(i) {
            *slot_mut = slot.clone();
        }
    }
}

/// An ECS message to switch our hand to a different hotbar slot.
///
/// This is equivalent to using the scroll wheel or number keys in Minecraft.
#[derive(EntityEvent)]
pub struct SetSelectedHotbarSlotEvent {
    pub entity: Entity,
    /// The hotbar slot to select. This should be in the range 0..=8.
    pub slot: u8,
}
pub fn handle_set_selected_hotbar_slot_event(
    set_selected_hotbar_slot: On<SetSelectedHotbarSlotEvent>,
    mut query: Query<&mut Inv>,
) {
    let Ok(mut inventory) = query.get_mut(set_selected_hotbar_slot.entity) else {
        warn!("Got SetSelectedHotbarSlotEvent for an entity without an inventory");
        return;
    };
    inventory.selected_hotbar_slot = set_selected_hotbar_slot.slot;
}

/// The item slot that the server thinks we have selected.
///
/// See [`ensure_has_sent_carried_item`].
#[derive(Component)]
pub struct LastSentSelectedHotbarSlot {
    pub slot: u8,
}
/// A system that makes sure that [`LastSentSelectedHotbarSlot`] is in sync with
/// [`Inv::selected_hotbar_slot`].
///
/// This is necessary to make sure that [`ServerboundSetCarriedItem`] is sent in
/// the right order, since it's not allowed to happen outside of a tick.
pub fn ensure_has_sent_carried_item(
    mut commands: Commands,
    query: Query<(Entity, &Inv, Option<&LastSentSelectedHotbarSlot>)>,
) {
    for (entity, inventory, last_sent) in query.iter() {
        if let Some(last_sent) = last_sent {
            if last_sent.slot == inventory.selected_hotbar_slot {
                continue;
            }

            commands.trigger(SendGamePacketEvent::new(
                entity,
                ServerboundSetCarriedItem {
                    slot: inventory.selected_hotbar_slot as u16,
                },
            ));
        }

        commands.entity(entity).insert(LastSentSelectedHotbarSlot {
            slot: inventory.selected_hotbar_slot,
        });
    }
}
