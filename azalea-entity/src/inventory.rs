use std::{cmp, collections::HashSet};

use azalea_chat::FormattedText;
use azalea_inventory::{
    ItemStack, ItemStackData, Menu,
    components::EquipmentSlot,
    item::MaxStackSizeExt,
    operations::{
        ClickOperation, CloneClick, PickupAllClick, PickupClick, QuickCraftKind, QuickCraftStatus,
        QuickCraftStatusKind, QuickMoveClick, ThrowClick,
    },
};

use crate::PlayerAbilities;

/// A local player's inventory and related data, including the container that
/// they may have opened.
#[cfg_attr(feature = "bevy_ecs", derive(bevy_ecs::component::Component))]
#[derive(Clone, Debug)]
pub struct Inventory {
    /// The player's inventory menu. This is guaranteed to be a `Menu::Player`.
    ///
    /// We keep it as a [`Menu`] since `Menu` has some useful functions that
    /// bare [`azalea_inventory::Player`] doesn't have.
    pub inventory_menu: azalea_inventory::Menu,

    /// The ID of the container that's currently open.
    ///
    /// Its value is not guaranteed to be anything specific, and it may change
    /// every time you open a container (unless it's 0, in which case it
    /// means that no container is open).
    pub id: i32,
    /// The current container menu that the player has open, or `None` if no
    /// container is open.
    pub container_menu: Option<azalea_inventory::Menu>,
    /// The custom name of the menu that's currently open.
    ///
    /// This can only be `Some` when `container_menu` is `Some`.
    pub container_menu_title: Option<FormattedText>,
    /// The item that is currently held by the cursor, or `Slot::Empty` if
    /// nothing is currently being held.
    ///
    /// This is different from [`Self::selected_hotbar_slot`], which is the
    /// item that's selected in the hotbar.
    pub carried: ItemStack,
    /// An identifier used by the server to track client inventory desyncs.
    ///
    /// This is the state id of the menu that's currently open; it's sent on
    /// every container click, and it's only ever updated when the server sends
    /// a new container update.
    pub state_id: u32,
    /// The state id of the player's inventory menu (container 0), which vanilla
    /// keeps separately from the open container's. It becomes
    /// [`Self::state_id`] again when the container is closed.
    pub inventory_menu_state_id: u32,

    pub quick_craft_status: QuickCraftStatusKind,
    pub quick_craft_kind: QuickCraftKind,
    /// A set of the indexes of the slots that have been right clicked in
    /// this "quick craft".
    pub quick_craft_slots: HashSet<u16>,

    /// The index of the item in the hotbar that's currently being held by the
    /// player. This must be in the range 0..=8.
    ///
    /// In a vanilla client this is changed by pressing the number keys or using
    /// the scroll wheel.
    pub selected_hotbar_slot: u8,
}

impl Inventory {
    /// Sets slot `i` of the player's inventory menu (container 0). While
    /// another container is open, its player slots show the same items (vanilla
    /// shares the `Slot` objects between the two menus), so they're updated
    /// too.
    pub fn set_inventory_menu_slot(&mut self, i: usize, item: ItemStack) {
        if let Some(menu) = &mut self.container_menu
            && (9..=44).contains(&i)
        {
            let at = *menu.player_slots_range().start() + (i - 9);
            if let Some(slot) = menu.slot_mut(at) {
                *slot = item.clone();
            }
        }
        if let Some(slot) = self.inventory_menu.slot_mut(i) {
            *slot = item;
        }
    }

    /// 1.21.4 `Inventory.setItem`: sets slot `i` of the player's inventory
    /// container, whose indexes are 0..=8 hotbar, 9..=35 the rest of the
    /// inventory, 36..=39 armor (feet, legs, chest, head) and 40 the offhand.
    /// Other indexes do nothing, as in vanilla.
    ///
    /// This is a different index space from the inventory menu's (see
    /// [`Self::set_inventory_menu_slot`]); returns the menu slot that was set.
    pub fn set_player_inventory_item(&mut self, i: u32, item: ItemStack) -> Option<usize> {
        let menu_slot = match i {
            0..=8 => 36 + i as usize,
            9..=35 => i as usize,
            36..=39 => 8 - (i as usize - 36),
            40 => 45,
            _ => return None,
        };
        self.set_inventory_menu_slot(menu_slot, item);
        Some(menu_slot)
    }

    /// Returns a reference to the currently active menu.
    ///
    /// If a container is open then it'll return [`Self::container_menu`],
    /// otherwise [`Self::inventory_menu`].
    ///
    /// Use [`Self::menu_mut`] if you need a mutable reference.
    pub fn menu(&self) -> &azalea_inventory::Menu {
        match &self.container_menu {
            Some(menu) => menu,
            _ => &self.inventory_menu,
        }
    }

    /// Returns a mutable reference to the currently active menu.
    ///
    /// If a container is open then it'll return [`Self::container_menu`],
    /// otherwise [`Self::inventory_menu`].
    ///
    /// Use [`Self::menu`] if you don't need a mutable reference.
    pub fn menu_mut(&mut self) -> &mut azalea_inventory::Menu {
        match &mut self.container_menu {
            Some(menu) => menu,
            _ => &mut self.inventory_menu,
        }
    }

    /// Modify the inventory as if the given operation was performed on it.
    pub fn simulate_click(
        &mut self,
        operation: &ClickOperation,
        player_abilities: &PlayerAbilities,
    ) {
        if let ClickOperation::QuickCraft(quick_craft) = operation {
            let last_quick_craft_status_tmp = self.quick_craft_status.clone();
            self.quick_craft_status = last_quick_craft_status_tmp.clone();
            let last_quick_craft_status = last_quick_craft_status_tmp;

            // no carried item, reset
            if self.carried.is_empty() {
                return self.reset_quick_craft();
            }
            // if we were starting or ending, or now we aren't ending and the
            // status changed, reset
            if (last_quick_craft_status == QuickCraftStatusKind::Start
                || last_quick_craft_status == QuickCraftStatusKind::End
                || self.quick_craft_status != QuickCraftStatusKind::End)
                && (self.quick_craft_status != last_quick_craft_status)
            {
                return self.reset_quick_craft();
            }
            if self.quick_craft_status == QuickCraftStatusKind::Start {
                self.quick_craft_kind = quick_craft.kind.clone();
                if self.quick_craft_kind == QuickCraftKind::Middle && player_abilities.instant_break
                {
                    self.quick_craft_status = QuickCraftStatusKind::Add;
                    self.quick_craft_slots.clear();
                } else {
                    self.reset_quick_craft();
                }
                return;
            }
            if let QuickCraftStatus::Add { slot } = quick_craft.status {
                let slot_item = self.menu().slot(slot as usize);
                if let Some(slot_item) = slot_item
                    && let ItemStack::Present(carried) = &self.carried
                {
                    // minecraft also checks slot.may_place(carried) and
                    // menu.can_drag_to(slot)
                    // but they always return true so they're not relevant for
                    // us
                    if can_item_quick_replace(slot_item, &self.carried, true)
                        && (self.quick_craft_kind == QuickCraftKind::Right
                            || carried.count as usize > self.quick_craft_slots.len())
                    {
                        self.quick_craft_slots.insert(slot);
                    }
                }
                return;
            }
            if self.quick_craft_status == QuickCraftStatusKind::End {
                if !self.quick_craft_slots.is_empty() {
                    if self.quick_craft_slots.len() == 1 {
                        // if we only clicked one slot, then turn this
                        // QuickCraftClick into a PickupClick
                        // network-safe: len() == 1 checked above
                        let slot = *self.quick_craft_slots.iter().next().unwrap();
                        self.reset_quick_craft();
                        self.simulate_click(
                            &match self.quick_craft_kind {
                                QuickCraftKind::Left => {
                                    PickupClick::Left { slot: Some(slot) }.into()
                                }
                                QuickCraftKind::Right => {
                                    PickupClick::Left { slot: Some(slot) }.into()
                                }
                                QuickCraftKind::Middle => {
                                    // idk just do nothing i guess
                                    return;
                                }
                            },
                            player_abilities,
                        );
                        return;
                    }

                    let ItemStack::Present(mut carried) = self.carried.clone() else {
                        // this should never happen
                        return self.reset_quick_craft();
                    };

                    let mut carried_count = carried.count;
                    let mut quick_craft_slots_iter = self.quick_craft_slots.iter();

                    loop {
                        let mut slot: &ItemStack;
                        let mut slot_index: u16;
                        let mut item_stack: &ItemStack;

                        loop {
                            let Some(&next_slot) = quick_craft_slots_iter.next() else {
                                carried.count = carried_count;
                                self.carried = ItemStack::Present(carried);
                                return self.reset_quick_craft();
                            };

                            // the menu can have changed since the slot was
                            // added (the server opened another one)
                            let Some(next) = self.menu().slot(next_slot as usize) else {
                                return self.reset_quick_craft();
                            };
                            slot = next;
                            slot_index = next_slot;
                            item_stack = &self.carried;

                            if slot.is_present()
                                    && can_item_quick_replace(slot, item_stack, true)
                                    // this always returns true in most cases
                                    // && slot.may_place(item_stack)
                                    && (
                                        self.quick_craft_kind == QuickCraftKind::Middle
                                        || item_stack.count()  >= self.quick_craft_slots.len() as i32
                                    )
                            {
                                break;
                            }
                        }

                        // get the ItemStackData for the slot
                        let ItemStack::Present(slot) = slot else {
                            // network-safe: the loop above only breaks on a
                            // present slot
                            unreachable!("the loop above requires the slot to be present to break")
                        };

                        // if self.can_drag_to(slot) {
                        let mut new_carried = carried.clone();
                        let slot_item_count = slot.count;
                        get_quick_craft_slot_count(
                            &self.quick_craft_slots,
                            &self.quick_craft_kind,
                            &mut new_carried,
                            slot_item_count,
                        );
                        let max_stack_size = i32::min(
                            new_carried.kind.max_stack_size(),
                            i32::min(
                                new_carried.kind.max_stack_size(),
                                slot.kind.max_stack_size(),
                            ),
                        );
                        if new_carried.count > max_stack_size {
                            new_carried.count = max_stack_size;
                        }

                        // counts come from the server (any positive i32)
                        carried_count = carried_count
                            .saturating_sub(new_carried.count.saturating_sub(slot_item_count));
                        // we have to inline self.menu_mut() here to avoid the
                        // borrow checker complaining
                        let menu = match &mut self.container_menu {
                            Some(menu) => menu,
                            _ => &mut self.inventory_menu,
                        };
                        // network-safe: the slot was read from this menu at
                        // this index in the loop above
                        *menu.slot_mut(slot_index as usize).unwrap() =
                            ItemStack::Present(new_carried);
                    }
                }
            } else {
                return self.reset_quick_craft();
            }
        }
        // the quick craft status should always be in start if we're not in
        // quick craft mode
        if self.quick_craft_status != QuickCraftStatusKind::Start {
            return self.reset_quick_craft();
        }

        match operation {
            // left clicking outside inventory
            ClickOperation::Pickup(PickupClick::Left { slot: None })
                if self.carried.is_present() =>
            {
                // vanilla has `player.drop`s but they're only used
                // server-side
                // they're included as comments here in case you want to adapt
                // this for a server implementation

                // player.drop(self.carried, true);
                self.carried = ItemStack::Empty;
            }
            ClickOperation::Pickup(PickupClick::Right { slot: None })
                if self.carried.is_present() =>
            {
                let _item = self.carried.split(1);
                // player.drop(item, true);
            }
            &ClickOperation::Pickup(
                // lol
                ref pickup @ (PickupClick::Left { slot: Some(slot) }
                | PickupClick::Right { slot: Some(slot) }),
            ) => {
                let slot = slot as usize;
                let Some(slot_item) = self.menu().slot(slot) else {
                    return;
                };

                if self.try_item_click_behavior_override(operation, slot) {
                    return;
                }

                let is_left_click = matches!(pickup, PickupClick::Left { .. });

                match slot_item {
                    ItemStack::Empty => {
                        if self.carried.is_present() {
                            let place_count = if is_left_click {
                                self.carried.count()
                            } else {
                                1
                            };
                            self.carried =
                                self.safe_insert(slot, self.carried.clone(), place_count);
                        }
                    }
                    ItemStack::Present(_) => {
                        if !self.menu().may_pickup(slot) {
                            return;
                        }
                        if let ItemStack::Present(carried) = self.carried.clone() {
                            let slot_is_same_item_as_carried = slot_item
                                .as_present()
                                .is_some_and(|s| carried.is_same_item_and_components(s));

                            if self.menu().may_place(slot, &carried) {
                                if slot_is_same_item_as_carried {
                                    let place_count = if is_left_click { carried.count } else { 1 };
                                    self.carried =
                                        self.safe_insert(slot, self.carried.clone(), place_count);
                                } else if carried.count
                                    <= self
                                        .menu()
                                        .max_stack_size(slot)
                                        .min(carried.kind.max_stack_size())
                                {
                                    // swap slot_item and carried
                                    self.carried = slot_item.clone();
                                    // network-safe: slot was checked at the top
                                    let slot_item = self.menu_mut().slot_mut(slot).unwrap();
                                    *slot_item = carried.into();
                                }
                            } else if slot_is_same_item_as_carried
                                && let Some(removed) = self.try_remove(
                                    slot,
                                    slot_item.count(),
                                    carried.kind.max_stack_size() - carried.count,
                                )
                            {
                                // network-safe: carried is present (the
                                // if let above); counts saturate
                                let carried = self.carried.as_present_mut().unwrap();
                                carried.count = carried.count.saturating_add(removed.count());
                                // slot.onTake(player, removed);
                            }
                        } else {
                            let pickup_count = if is_left_click {
                                slot_item.count()
                            } else {
                                (slot_item.count() + 1) / 2
                            };
                            if let Some(new_slot_item) =
                                self.try_remove(slot, pickup_count, i32::MAX)
                            {
                                self.carried = new_slot_item;
                                // slot.onTake(player, newSlot);
                            }
                        }
                    }
                }
            }
            &ClickOperation::QuickMove(
                QuickMoveClick::Left { slot } | QuickMoveClick::Right { slot },
            ) => {
                // in vanilla it also tests if QuickMove has a slot index of
                // -999 but i don't think that's ever possible so it's not
                // covered here
                let slot = slot as usize;
                loop {
                    let new_slot_item = self.menu_mut().quick_move_stack(slot);
                    // a slot the menu doesn't have (the server can open a
                    // different menu than the bot expects)
                    let Some(slot_item) = self.menu().slot(slot) else {
                        break;
                    };
                    if new_slot_item.is_empty() || slot_item.kind() != new_slot_item.kind() {
                        break;
                    }
                }
            }
            ClickOperation::Swap(s) => {
                // 1.21.4 AbstractContainerMenu.doClick, SWAP: the button is a
                // player inventory index (hotbar 0..9, or 40 for the offhand),
                // swapped with the clicked menu slot
                let slot_index = s.source_slot as usize;
                let button = s.target_slot as usize;
                if !(button < 9 || button == 40) {
                    return;
                }
                // where inventory index `button` is: in the open menu, or (the
                // offhand while a container is open) only in the inventory menu
                let in_open_menu = button != 40 || self.id == 0;
                let inventory_slot_index = if button == 40 {
                    45
                } else {
                    *self.menu().hotbar_slots_range().start() + button
                };
                let Some(slot_item) = self.menu().slot(slot_index).cloned() else {
                    return;
                };
                let inventory_item = if in_open_menu {
                    self.menu().slot(inventory_slot_index).cloned()
                } else {
                    self.inventory_menu.slot(inventory_slot_index).cloned()
                };
                let Some(inventory_item) = inventory_item else {
                    return;
                };
                if inventory_item.is_empty() && slot_item.is_empty() {
                    return;
                }

                let (new_inventory_item, new_slot_item) = if inventory_item.is_empty() {
                    if !self.menu().may_pickup(slot_index) {
                        return;
                    }
                    (slot_item, ItemStack::Empty)
                } else if slot_item.is_empty() {
                    let ItemStack::Present(item) = &inventory_item else {
                        return;
                    };
                    if !self.menu().may_place(slot_index, item) {
                        return;
                    }
                    let max = self
                        .menu()
                        .max_stack_size(slot_index)
                        .min(item.kind.max_stack_size());
                    let mut rest = inventory_item.clone();
                    if item.count > max {
                        let placed = rest.split(max.max(0) as u32);
                        (rest, placed)
                    } else {
                        (ItemStack::Empty, rest)
                    }
                } else {
                    let ItemStack::Present(item) = &inventory_item else {
                        return;
                    };
                    if !self.menu().may_pickup(slot_index) || !self.menu().may_place(slot_index, item)
                    {
                        return;
                    }
                    let max = self
                        .menu()
                        .max_stack_size(slot_index)
                        .min(item.kind.max_stack_size());
                    if item.count > max {
                        // vanilla puts the slot's item back into the
                        // inventory (or drops it); the server's answer
                        // corrects us if this differs
                        let mut rest = inventory_item.clone();
                        let placed = rest.split(max.max(0) as u32);
                        (rest, placed)
                    } else {
                        (slot_item, inventory_item)
                    }
                };

                if let Some(slot) = self.menu_mut().slot_mut(slot_index) {
                    *slot = new_slot_item;
                }
                let target = if in_open_menu {
                    self.menu_mut().slot_mut(inventory_slot_index)
                } else {
                    self.inventory_menu.slot_mut(inventory_slot_index)
                };
                if let Some(target) = target {
                    *target = new_inventory_item;
                }
            }
            ClickOperation::Clone(CloneClick { slot }) => {
                if !player_abilities.instant_break || self.carried.is_present() {
                    return;
                }
                let Some(source_slot) = self.menu().slot(*slot as usize) else {
                    return;
                };
                let ItemStack::Present(source_item) = source_slot else {
                    return;
                };
                let mut new_carried = source_item.clone();
                new_carried.count = new_carried.kind.max_stack_size();
                self.carried = ItemStack::Present(new_carried);
            }
            ClickOperation::Throw(c) => {
                if self.carried.is_present() {
                    return;
                }

                let (ThrowClick::Single { slot: slot_index }
                | ThrowClick::All { slot: slot_index }) = c;
                let slot_index = *slot_index as usize;

                let Some(slot) = self.menu_mut().slot_mut(slot_index) else {
                    return;
                };
                let ItemStack::Present(slot_item) = slot else {
                    return;
                };

                let dropping_count = match c {
                    ThrowClick::Single { .. } => 1,
                    ThrowClick::All { .. } => slot_item.count,
                };

                let _dropping = slot_item.split(dropping_count as u32);
                // player.drop(dropping, true);
            }
            ClickOperation::PickupAll(PickupAllClick {
                slot: source_slot_index,
                reversed,
            }) => {
                let source_slot_index = *source_slot_index as usize;

                let Some(source_slot) = self.menu().slot(source_slot_index) else {
                    return;
                };
                let target_slot = self.carried.clone();

                if target_slot.is_empty()
                    || (source_slot.is_present() && self.menu().may_pickup(source_slot_index))
                {
                    return;
                }

                let ItemStack::Present(target_slot_item) = &target_slot else {
                    // network-safe: not empty, so present
                    unreachable!("target slot is not empty but is not present");
                };

                for round in 0..2 {
                    let iterator: Box<dyn Iterator<Item = usize>> = if *reversed {
                        Box::new((0..self.menu().len()).rev())
                    } else {
                        Box::new(0..self.menu().len())
                    };

                    for i in iterator {
                        if target_slot_item.count < target_slot_item.kind.max_stack_size() {
                            // network-safe: i < menu len
                            let checking_slot = self.menu().slot(i).unwrap();
                            if let ItemStack::Present(checking_item) = checking_slot
                                && can_item_quick_replace(checking_slot, &target_slot, true)
                                && self.menu().may_pickup(i)
                                && (round != 0
                                    || checking_item.count != checking_item.kind.max_stack_size())
                            {
                                // get the checking_slot and checking_item again
                                // but mutable
                                // network-safe: i < menu len
                                let checking_slot = self.menu_mut().slot_mut(i).unwrap();

                                let taken_item = checking_slot.split(checking_slot.count() as u32);

                                // now extend the carried item
                                let target_slot = &mut self.carried;
                                let ItemStack::Present(target_slot_item) = target_slot else {
                                    // network-safe: carried was present above
                                    // and only grows here
                                    unreachable!("target slot is not empty but is not present");
                                };
                                target_slot_item.count =
                                    target_slot_item.count.saturating_add(taken_item.count());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn reset_quick_craft(&mut self) {
        self.quick_craft_status = QuickCraftStatusKind::Start;
        self.quick_craft_slots.clear();
    }

    /// Get the item in the player's hotbar that is currently being held in
    /// their main hand.
    pub fn held_item(&self) -> &ItemStack {
        self.get_equipment(EquipmentSlot::Mainhand)
            // network-safe: selected_hotbar_slot is at most 8 (set_held_slot
            // checks it) and every menu has 9 hotbar slots
            .expect("The main hand item should always be present")
    }

    /// TODO: implement bundles
    fn try_item_click_behavior_override(
        &self,
        _operation: &ClickOperation,
        _slot_item_index: usize,
    ) -> bool {
        false
    }

    fn safe_insert(&mut self, slot: usize, src_item: ItemStack, take_count: i32) -> ItemStack {
        let Some(slot_item) = self.menu_mut().slot_mut(slot) else {
            return src_item;
        };
        let ItemStack::Present(mut src_item) = src_item else {
            return src_item;
        };

        let take_count = cmp::min(
            cmp::min(take_count, src_item.count),
            src_item.kind.max_stack_size() - slot_item.count(),
        );
        if take_count <= 0 {
            return src_item.into();
        }
        let take_count = take_count as u32;

        if slot_item.is_empty() {
            *slot_item = src_item.split(take_count).into();
        } else if let ItemStack::Present(slot_item) = slot_item
            && slot_item.is_same_item_and_components(&src_item)
        {
            src_item.count = src_item.count.saturating_sub(take_count as i32);
            slot_item.count = slot_item.count.saturating_add(take_count as i32);
        }

        src_item.into()
    }

    fn try_remove(&mut self, slot: usize, count: i32, limit: i32) -> Option<ItemStack> {
        if !self.menu().may_pickup(slot) {
            return None;
        }
        let mut slot_item = self.menu().slot(slot)?.clone();
        if !self.menu().allow_modification(slot) && limit < slot_item.count() {
            return None;
        }

        let count = count.min(limit);
        if count <= 0 {
            return None;
        }
        // 1.21.4 Slot.tryRemove -> Slot.remove -> Container.removeItem splits
        // the stack in the slot, so the slot keeps what's left (a partial
        // pickup, like a right click on a stack, takes half and leaves half).
        let removed = slot_item.split(count as u32);
        slot_item.update_empty();
        // network-safe: read from this index above
        *self.menu_mut().slot_mut(slot).unwrap() = slot_item;

        Some(removed)
    }

    /// Get the item at the given equipment slot, or `None` if the inventory
    /// can't contain that slot.
    pub fn get_equipment(&self, equipment_slot: EquipmentSlot) -> Option<&ItemStack> {
        let player = self.inventory_menu.as_player();
        let item = match equipment_slot {
            EquipmentSlot::Mainhand => {
                let menu = self.menu();
                let main_hand_slot_idx =
                    *menu.hotbar_slots_range().start() + self.selected_hotbar_slot as usize;
                menu.slot(main_hand_slot_idx)?
            }
            EquipmentSlot::Offhand => &player.offhand,
            EquipmentSlot::Feet => &player.armor[3], // network-safe: fixed array
            EquipmentSlot::Legs => &player.armor[2], // network-safe: fixed array
            EquipmentSlot::Chest => &player.armor[1], // network-safe: fixed array
            EquipmentSlot::Head => &player.armor[0], // network-safe: fixed array
            EquipmentSlot::Body => {
                // TODO: when riding entities is implemented, mount/horse
                // inventories should be implemented too. note that horse
                // inventories aren't a normal menu (they're not in MenuKind),
                // maybe they should be a separate field in `Inventory`?
                return None;
            }
        };
        Some(item)
    }
}

fn can_item_quick_replace(
    target_slot: &ItemStack,
    item: &ItemStack,
    ignore_item_count: bool,
) -> bool {
    let ItemStack::Present(target_slot) = target_slot else {
        return false;
    };
    let ItemStack::Present(item) = item else {
        // i *think* this is what vanilla does
        // not 100% sure lol probably doesn't matter though
        return false;
    };

    if !item.is_same_item_and_components(target_slot) {
        return false;
    }
    let count = target_slot.count as u16
        + if ignore_item_count {
            0
        } else {
            item.count as u16
        };
    count <= item.kind.max_stack_size() as u16
}

fn get_quick_craft_slot_count(
    quick_craft_slots: &HashSet<u16>,
    quick_craft_kind: &QuickCraftKind,
    item: &mut ItemStackData,
    slot_item_count: i32,
) {
    item.count = match quick_craft_kind {
        QuickCraftKind::Left => item.count / quick_craft_slots.len() as i32,
        QuickCraftKind::Right => 1,
        QuickCraftKind::Middle => item.kind.max_stack_size(),
    };
    item.count = item.count.saturating_add(slot_item_count);
}

impl Default for Inventory {
    fn default() -> Self {
        Inventory {
            inventory_menu: Menu::Player(azalea_inventory::Player::default()),
            id: 0,
            container_menu: None,
            container_menu_title: None,
            carried: ItemStack::Empty,
            state_id: 0,
            inventory_menu_state_id: 0,
            quick_craft_status: QuickCraftStatusKind::Start,
            quick_craft_kind: QuickCraftKind::Middle,
            quick_craft_slots: HashSet::new(),
            selected_hotbar_slot: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use azalea_inventory::{SlotList, operations::SwapClick};
    use azalea_registry::builtin::ItemKind;

    use super::*;

    #[test]
    fn test_simulate_shift_click_in_crafting_table() {
        let spruce_planks = ItemStack::new(ItemKind::SprucePlanks, 4);

        let mut inventory = Inventory {
            inventory_menu: Menu::Player(azalea_inventory::Player::default()),
            id: 1,
            container_menu: Some(Menu::Crafting {
                result: spruce_planks.clone(),
                // simulate_click won't delete the items from here
                grid: SlotList::default(),
                player: SlotList::default(),
            }),
            container_menu_title: None,
            carried: ItemStack::Empty,
            state_id: 0,
            inventory_menu_state_id: 0,
            quick_craft_status: QuickCraftStatusKind::Start,
            quick_craft_kind: QuickCraftKind::Middle,
            quick_craft_slots: HashSet::new(),
            selected_hotbar_slot: 0,
        };

        inventory.simulate_click(
            &ClickOperation::QuickMove(QuickMoveClick::Left { slot: 0 }),
            &PlayerAbilities::default(),
        );

        let new_slots = inventory.menu().slots();
        assert_eq!(&new_slots[0], &ItemStack::Empty);
        assert_eq!(
            &new_slots[*Menu::CRAFTING_PLAYER_SLOTS.start()],
            &spruce_planks
        );
    }

    fn with_shulker(contents: &[(usize, ItemStack)], player: &[(usize, ItemStack)]) -> Inventory {
        let mut menu = Menu::ShulkerBox {
            contents: SlotList::default(),
            player: SlotList::default(),
        };
        for (i, item) in contents.iter().chain(player.iter()) {
            *menu.slot_mut(*i).unwrap() = item.clone();
        }
        Inventory {
            id: 1,
            container_menu: Some(menu),
            ..Inventory::default()
        }
    }

    // 1.21.4 Slot.tryRemove: a right click takes half and leaves the rest
    #[test]
    fn test_right_click_picks_up_half() {
        let mut inventory = with_shulker(&[(0, ItemStack::new(ItemKind::Cobblestone, 10))], &[]);
        inventory.simulate_click(
            &ClickOperation::Pickup(PickupClick::Right { slot: Some(0) }),
            &PlayerAbilities::default(),
        );
        assert_eq!(inventory.carried, ItemStack::new(ItemKind::Cobblestone, 5));
        assert_eq!(
            inventory.menu().slot(0),
            Some(&ItemStack::new(ItemKind::Cobblestone, 5))
        );
    }

    // 1.21.4 ShulkerBoxMenu.quickMoveStack: contents go to the player inventory
    // last slot first (the end of the hotbar)
    #[test]
    fn test_shift_click_from_shulker_fills_hotbar_end_first() {
        let mut inventory = with_shulker(&[(0, ItemStack::new(ItemKind::Elytra, 1))], &[]);
        inventory.simulate_click(
            &ClickOperation::QuickMove(QuickMoveClick::Left { slot: 0 }),
            &PlayerAbilities::default(),
        );
        let slots = inventory.menu().slots();
        assert_eq!(slots[0], ItemStack::Empty);
        assert_eq!(slots[62], ItemStack::new(ItemKind::Elytra, 1));
    }

    // AbstractContainerMenu.moveItemStackTo merges into a partial stack
    #[test]
    fn test_shift_click_into_shulker_merges() {
        let mut inventory = with_shulker(
            &[(5, ItemStack::new(ItemKind::Obsidian, 1))],
            &[(62, ItemStack::new(ItemKind::Obsidian, 31))],
        );
        inventory.simulate_click(
            &ClickOperation::QuickMove(QuickMoveClick::Left { slot: 62 }),
            &PlayerAbilities::default(),
        );
        let slots = inventory.menu().slots();
        assert_eq!(slots[5], ItemStack::new(ItemKind::Obsidian, 32));
        assert_eq!(slots[62], ItemStack::Empty);
        assert_eq!(slots[0], ItemStack::Empty);
    }

    // 1.21.4 doClick SWAP: the button is a player inventory index (40 is the
    // offhand), swapped with the clicked slot; nothing is duplicated
    #[test]
    fn test_swap_to_offhand_in_inventory_menu() {
        let mut inventory = Inventory::default();
        *inventory.inventory_menu.slot_mut(20).unwrap() = ItemStack::new(ItemKind::TotemOfUndying, 1);
        *inventory.inventory_menu.slot_mut(45).unwrap() = ItemStack::new(ItemKind::Dirt, 3);
        inventory.simulate_click(
            &ClickOperation::Swap(SwapClick {
                source_slot: 20,
                target_slot: 40,
            }),
            &PlayerAbilities::default(),
        );
        let slots = inventory.menu().slots();
        assert_eq!(slots[45], ItemStack::new(ItemKind::TotemOfUndying, 1));
        assert_eq!(slots[20], ItemStack::new(ItemKind::Dirt, 3));

        // into an empty hotbar slot: moved, not copied
        inventory.simulate_click(
            &ClickOperation::Swap(SwapClick {
                source_slot: 45,
                target_slot: 2,
            }),
            &PlayerAbilities::default(),
        );
        let slots = inventory.menu().slots();
        assert_eq!(slots[38], ItemStack::new(ItemKind::TotemOfUndying, 1));
        assert_eq!(slots[45], ItemStack::Empty);

        // a button that isn't a hotbar index or the offhand does nothing
        let before = inventory.menu().slots();
        inventory.simulate_click(
            &ClickOperation::Swap(SwapClick {
                source_slot: 20,
                target_slot: 12,
            }),
            &PlayerAbilities::default(),
        );
        assert_eq!(inventory.menu().slots(), before);
    }

    #[test]
    fn test_swap_with_container_open() {
        let mut inventory = with_shulker(
            &[(3, ItemStack::new(ItemKind::TotemOfUndying, 1))],
            // hotbar 0 of the shulker menu
            &[(54, ItemStack::new(ItemKind::Stone, 64))],
        );
        inventory.simulate_click(
            &ClickOperation::Swap(SwapClick {
                source_slot: 3,
                target_slot: 0,
            }),
            &PlayerAbilities::default(),
        );
        let slots = inventory.menu().slots();
        assert_eq!(slots[3], ItemStack::new(ItemKind::Stone, 64));
        assert_eq!(slots[54], ItemStack::new(ItemKind::TotemOfUndying, 1));
        // the offhand isn't in the shulker menu: it's the inventory menu's
        inventory.simulate_click(
            &ClickOperation::Swap(SwapClick {
                source_slot: 54,
                target_slot: 40,
            }),
            &PlayerAbilities::default(),
        );
        assert_eq!(inventory.menu().slots()[54], ItemStack::Empty);
        assert_eq!(
            inventory.inventory_menu.slots()[45],
            ItemStack::new(ItemKind::TotemOfUndying, 1)
        );
    }

    // 1.21.4 Inventory.setItem (ClientboundSetPlayerInventory): items 0..36
    // (hotbar first), armor 36..40 (feet first), offhand 40
    #[test]
    fn test_set_player_inventory_item_slots() {
        let mut inventory = Inventory::default();
        let cases = [
            (0, 36),
            (8, 44),
            (9, 9),
            (35, 35),
            (36, 8),
            (39, 5),
            (40, 45),
        ];
        for (i, (index, menu_slot)) in cases.into_iter().enumerate() {
            let item = ItemStack::new(ItemKind::Stone, i as i32 + 1);
            assert_eq!(
                inventory.set_player_inventory_item(index, item.clone()),
                Some(menu_slot)
            );
            assert_eq!(inventory.inventory_menu.slot(menu_slot), Some(&item));
        }
        let player = inventory.inventory_menu.as_player();
        assert_eq!(player.armor[3], ItemStack::new(ItemKind::Stone, 5), "feet");
        assert_eq!(player.armor[0], ItemStack::new(ItemKind::Stone, 6), "head");
        assert_eq!(player.offhand, ItemStack::new(ItemKind::Stone, 7));
        // past the offhand: nothing, as in vanilla
        let before = inventory.inventory_menu.slots();
        assert_eq!(
            inventory.set_player_inventory_item(41, ItemStack::new(ItemKind::Dirt, 1)),
            None
        );
        assert_eq!(inventory.inventory_menu.slots(), before);
    }

    // the open container's player slots show the same items
    #[test]
    fn test_set_player_inventory_item_with_container_open() {
        let mut inventory = with_shulker(&[], &[]);
        inventory.set_player_inventory_item(0, ItemStack::new(ItemKind::Elytra, 1));
        // shulker: 27 contents, then 27 inventory, then 9 hotbar
        assert_eq!(
            inventory.menu().slot(54),
            Some(&ItemStack::new(ItemKind::Elytra, 1))
        );
        assert_eq!(
            inventory.inventory_menu.slot(36),
            Some(&ItemStack::new(ItemKind::Elytra, 1))
        );
    }
}
