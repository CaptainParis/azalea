//! What a client does with a packet it couldn't decode (bad packet policy,
//! hunt WP2 section 4).
//!
//! A packet is classed by its id (its name in the current state):
//!
//! - frame errors (bad length, inflate failure, size mismatch): the stream is
//!   lost, the connection is closed ([`close_connection`]);
//! - state-free packets (sounds, particles, chat display, animations, titles,
//!   ...): dropped, counted, logged once per packet id per minute;
//! - inventory and container packets: the inventory is marked unknown
//!   ([`InventoryUnknown`]) and resynced with a click on slot -1, mode PICKUP,
//!   button 0, carrying a stale state id, for the container we believe is open.
//!   In 1.21.4 `doClick` ignores negative slots other than -999, and the state
//!   id mismatch makes `handleContainerClick` call `broadcastFullState`, which
//!   resends the whole menu and the carried item. Retried once after 2 s; still
//!   unknown after 5 s closes the connection. Ordinary clicks wait for the
//!   resync ([`PendingClicks`]); reflex clicks
//!   ([`ContainerClickEvent::reflex`]) never wait;
//! - packets about other entities: the entity keeps its last good state and is
//!   marked [`StaleEntity`] until the server removes it or its chunk unloads. A
//!   spawn that can't be decoded spawns a [`PlaceholderEntity`] from the fields
//!   that can be read (id, uuid, type, position), marked stale and alerted at
//!   once;
//! - chunk and block packets: the chunk is unknown, treated as unloaded
//!   ([`UnknownChunks`]);
//! - everything with no safe repair (login, respawn, teleport, start
//!   configuration, our own entity, registries and tags, the login state,
//!   unknown ids): the connection is closed and an alert raised.
//!
//! Configuration packets are classed too ([`classify_config`]): display-only
//! ones (server links, report details, custom payloads) are dropped, so a bad
//! one during a queue-to-main transfer doesn't cost the session.
//!
//! Every outcome is counted ([`DecodeErrorKind`]) and sent as a
//! [`BadPacketEvent`], which carries the raw packet bytes (for a proxy to
//! forward). Any packet id that goes bad more than once in an hour raises a
//! [`BadPacketAlert`], so a dropped packet is never a silent steady state.
//!
//! [`ContainerClickEvent::reflex`]: crate::inventory::ContainerClickEvent::reflex

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Cursor,
    net::Shutdown,
    sync::Arc,
    time::{Duration, Instant},
};

use azalea_buf::{AzBuf, AzBufVar};
use azalea_core::{
    decode_errors::{DecodeErrorKind, record_decode_error},
    delta::QuantizedVelocity,
    entity_id::MinecraftEntityId,
    position::{BlockPos, ChunkPos, ChunkSectionPos, Vec3},
};
use azalea_entity::{InLoadedChunk, indexing::EntityIdIndex, inventory::Inventory};
use azalea_inventory::operations::{ClickOperation, ClickType};
use azalea_protocol::{
    packets::{
        ConnectionProtocol,
        game::{ClientboundAddEntity, s_container_click::ServerboundContainerClick},
    },
    read::ReadPacketError,
};
use azalea_registry::builtin::EntityKind;
use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;
use indexmap::IndexMap;
use tracing::{debug, error, warn};
use uuid::Uuid;

use crate::{
    chunks::handle_receive_chunk_event,
    connection::{ConnectionSocket, RawConnection},
    inventory::ContainerClickEvent,
    local_player::WorldHolder,
    packet::game::{GamePacketHandler, SendGamePacketEvent},
};

/// How a clientbound game packet that couldn't be decoded is handled.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PacketClass {
    /// No model state depends on it: dropped.
    StateFree,
    /// Inventory or container state: resync.
    Inventory,
    /// Another entity's state: that entity is stale.
    Entity,
    /// Chunk or block state: that chunk is unknown.
    Chunk,
    /// No safe repair: close and reconnect, alert.
    Critical,
}

/// The class of a clientbound game packet, by its name (as in
/// `declare_state_packets!`). `None` for a name that isn't a clientbound game
/// packet.
pub fn classify_game(name: &str) -> Option<PacketClass> {
    use PacketClass::*;
    Some(match name {
        // display only, or bookkeeping nothing in the model reads
        "bundle_delimiter"
        | "animate"
        | "award_stats"
        | "block_destruction"
        | "block_event"
        | "boss_event"
        | "change_difficulty"
        | "clear_titles"
        | "command_suggestions"
        | "commands"
        | "cooldown"
        | "custom_chat_completions"
        | "custom_payload"
        | "damage_event"
        | "debug_sample"
        | "delete_chat"
        | "disguised_chat"
        | "hurt_animation"
        | "initialize_border"
        | "level_event"
        | "level_particles"
        | "light_update"
        | "map_item_data"
        | "merchant_offers"
        | "open_book"
        | "open_sign_editor"
        | "place_ghost_recipe"
        | "player_chat"
        | "player_combat_end"
        | "player_combat_enter"
        | "player_info_remove"
        | "player_info_update"
        | "pong_response"
        | "recipe_book_add"
        | "recipe_book_remove"
        | "recipe_book_settings"
        | "reset_score"
        | "resource_pack_pop"
        | "select_advancements_tab"
        | "server_data"
        | "set_action_bar_text"
        | "set_border_center"
        | "set_border_lerp_size"
        | "set_border_size"
        | "set_border_warning_delay"
        | "set_border_warning_distance"
        | "set_camera"
        | "set_default_spawn_position"
        | "set_display_objective"
        | "set_objective"
        | "set_player_team"
        | "set_score"
        | "set_simulation_distance"
        | "set_subtitle_text"
        | "set_time"
        | "set_title_text"
        | "set_titles_animation"
        | "sound_entity"
        | "sound"
        | "stop_sound"
        | "system_chat"
        | "tab_list"
        | "tag_query"
        | "update_advancements"
        | "update_recipes"
        | "custom_report_details"
        | "server_links" => StateFree,

        "container_close"
        | "container_set_content"
        | "container_set_data"
        | "container_set_slot"
        | "open_screen"
        | "horse_screen_open"
        | "set_cursor_item"
        | "set_player_inventory" => Inventory,

        "add_entity"
        | "add_experience_orb"
        | "entity_event"
        | "entity_position_sync"
        | "move_entity_pos"
        | "move_entity_pos_rot"
        | "move_entity_rot"
        | "move_minecart_along_track"
        | "projectile_power"
        | "remove_entities"
        | "rotate_head"
        | "set_entity_data"
        | "set_entity_link"
        | "set_entity_motion"
        | "set_equipment"
        | "set_passengers"
        | "take_item_entity"
        | "teleport_entity"
        | "update_attributes"
        | "update_mob_effect"
        | "remove_mob_effect" => Entity,

        "block_entity_data"
        | "block_update"
        | "chunks_biomes"
        | "forget_level_chunk"
        | "level_chunk_with_light"
        | "section_blocks_update" => Chunk,

        // login, respawn, teleport, start configuration, our own player, and
        // packets the server waits for an answer to or that size the world
        // view
        "login"
        | "respawn"
        | "player_position"
        | "player_rotation"
        | "player_look_at"
        | "start_configuration"
        | "set_health"
        | "set_experience"
        | "player_abilities"
        | "game_event"
        | "set_held_slot"
        | "explode"
        | "player_combat_kill"
        | "keep_alive"
        | "ping"
        | "disconnect"
        | "transfer"
        | "cookie_request"
        | "resource_pack_push"
        | "chunk_batch_start"
        | "chunk_batch_finished"
        | "set_chunk_cache_center"
        | "set_chunk_cache_radius"
        | "update_tags"
        | "block_changed_ack"
        | "ticking_state"
        | "ticking_step"
        | "move_vehicle"
        // state the server reads back after a transfer (as in configuration)
        | "store_cookie" => Critical,

        _ => return None,
    })
}

/// The class of a clientbound configuration packet, by its name. `None` for a
/// name that isn't one.
///
/// Configuration also runs in the middle of a session (a proxy moving us from
/// the queue to the main server sends `start_configuration`, then a whole
/// configuration phase), so a display-only packet there must not cost the
/// session either.
pub fn classify_config(name: &str) -> Option<PacketClass> {
    use PacketClass::*;
    Some(match name {
        // 1.21.4 ClientCommonPacketListenerImpl and
        // ClientConfigurationPacketListenerImpl: server links and report
        // details only feed the pause and crash report screens; custom
        // payloads (brand, plugin channels) are only logged by Azalea;
        // reset_chat clears the chat display; resource_pack_pop unloads a
        // pack, and a bot never loads one
        "server_links" | "custom_report_details" | "custom_payload" | "reset_chat"
        | "resource_pack_pop" => StateFree,

        // registry_data: every registry id after a lost entry would point to
        // the wrong entry (dimension heights, biomes, damage types), so
        // nothing later can be trusted
        "registry_data"
        // update_tags: the tag view (climbable, mineable, ...) would differ
        // from the server's for the whole session
        | "update_tags"
        // update_enabled_features: decides which registry entries exist
        | "update_enabled_features"
        // finish_configuration: the switch to play; the server waits for our
        // acknowledgement
        | "finish_configuration"
        // select_known_packs: the server waits for our answer, and the answer
        // decides which registry data it sends
        | "select_known_packs"
        // keep_alive and ping: the server waits for the answer
        | "keep_alive" | "ping"
        // cookie_request and resource_pack_push: the server waits for an
        // answer about a key or a pack id we couldn't read
        | "cookie_request" | "resource_pack_push"
        // store_cookie: state the server reads back after a transfer
        | "store_cookie"
        // the session itself
        | "disconnect" | "transfer" => Critical,

        _ => return None,
    })
}

/// What the policy did with a bad packet.
#[derive(Clone, Debug, PartialEq)]
pub enum BadPacketAction {
    /// Dropped, no model change (state-free).
    Dropped,
    /// The inventory was marked unknown and a resync click sent (or one was
    /// already on the way).
    InventoryResync { container_id: i32 },
    /// These entities keep their last good state, marked stale. Empty if the
    /// entity id couldn't be read or isn't in our index.
    EntityStale { entities: Vec<MinecraftEntityId> },
    /// An entity spawn couldn't be decoded: a placeholder entity was spawned
    /// from the fields that could be read, marked [`StaleEntity`] and
    /// [`PlaceholderEntity`].
    PlaceholderSpawned {
        id: MinecraftEntityId,
        read: PlaceholderEntity,
    },
    /// These chunks are unknown (unloaded).
    ChunkUnknown { chunks: Vec<ChunkPos> },
    /// The connection was closed for a reconnect.
    Closed { reason: String },
}

/// Sent for every packet the bad packet policy handled. Carries the raw
/// packet bytes (after decompression: id and body) so the connection layer
/// still has them after the decode error (for example to forward them to an
/// attached client).
#[derive(Clone, Debug, Message)]
pub struct BadPacketEvent {
    pub entity: Entity,
    pub state: ConnectionProtocol,
    /// `None` if the id itself couldn't be read (or for a frame error).
    pub packet_id: Option<u32>,
    pub packet_name: Option<String>,
    pub class: Option<PacketClass>,
    pub action: BadPacketAction,
    pub error: String,
    /// Empty for a frame error (there's no packet to keep).
    pub raw: Arc<[u8]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlertReason {
    /// The same packet id went bad more than once in the alert window.
    Repeated,
    /// A packet with no safe repair: the connection was closed.
    Closed,
    /// The inventory resync didn't arrive in time: the connection was closed.
    ResyncFailed,
    /// A part of the model was lost (an entity spawn, the tab list). Raised
    /// the first time, not only when it repeats.
    ModelGap,
}

/// Something a human should look at (consumed by metrics). Raised for every
/// close, and whenever a packet id goes bad more than once in an hour.
#[derive(Clone, Debug, Message)]
pub struct BadPacketAlert {
    pub entity: Entity,
    pub state: ConnectionProtocol,
    pub packet_id: Option<u32>,
    pub packet_name: Option<String>,
    /// How many times this packet id went bad in the alert window, this one
    /// included.
    pub count_in_window: usize,
    pub reason: AlertReason,
}

/// The policy's time limits.
#[derive(Clone, Debug, Resource)]
pub struct BadPacketConfig {
    /// Resend the resync click once if the inventory is still unknown after
    /// this long.
    pub resync_retry: Duration,
    /// Close the connection if the inventory is still unknown after this
    /// long. Ordinary clicks never wait longer than this.
    pub resync_give_up: Duration,
    /// A packet id that goes bad is logged at most once per this.
    pub log_interval: Duration,
    /// More than one bad packet with the same id in this window raises an
    /// alert.
    pub alert_window: Duration,
}
impl Default for BadPacketConfig {
    fn default() -> Self {
        Self {
            resync_retry: Duration::from_secs(2),
            resync_give_up: Duration::from_secs(5),
            log_interval: Duration::from_secs(60),
            alert_window: Duration::from_secs(3600),
        }
    }
}

/// A function that may change a raw clientbound packet (id and body, after
/// decompression) before it's decoded. For tests that corrupt a packet in
/// the stream; it's never set by Azalea itself.
pub type RawPacketHook = Arc<dyn Fn(Entity, ConnectionProtocol, &mut Vec<u8>) + Send + Sync>;

/// The [`RawPacketHook`]s of an app, called in order on every clientbound
/// packet before it's decoded.
#[derive(Clone, Default, Resource)]
pub struct RawPacketHooks(pub Vec<RawPacketHook>);

/// Our inventory may not match the server's: an inventory or container
/// packet couldn't be decoded. Removed when the server sends the full
/// content of the container we have open.
#[derive(Clone, Component, Debug)]
pub struct InventoryUnknown {
    pub since: Instant,
    /// Resync clicks sent so far (at most 2).
    pub clicks_sent: u8,
}

/// Ordinary container clicks that wait for an inventory resync. They run in
/// order once the inventory is known again, or are dropped after
/// [`BadPacketConfig::resync_give_up`].
#[derive(Clone, Component, Debug, Default)]
pub struct PendingClicks(pub Vec<PendingClick>);

#[derive(Clone, Debug)]
pub struct PendingClick {
    pub at: Instant,
    pub window_id: i32,
    pub operation: ClickOperation,
}

/// An entity that keeps its last good state because a packet about it
/// couldn't be decoded. Later packets for it still apply. Removed when the
/// entity is removed or its chunk unloads.
#[derive(Clone, Component, Debug)]
pub struct StaleEntity {
    pub since: Instant,
    pub packet: String,
}

/// An entity spawned by the bad packet policy from the readable start of an
/// `add_entity` packet that couldn't be decoded whole (it also has
/// [`StaleEntity`]). Fields that couldn't be read have placeholder values: a
/// `marker` type (vanilla servers never send markers, so it can't be a real
/// one), a uuid made from the entity id, the origin as position, zero
/// rotations, data and velocity. Removed with the entity.
#[derive(Clone, Component, Debug, Eq, PartialEq)]
pub struct PlaceholderEntity {
    pub uuid_known: bool,
    pub kind_known: bool,
    pub position_known: bool,
    /// Rotations, data and velocity.
    pub rest_known: bool,
}

/// Chunks of this client that are unknown because a chunk or block packet
/// for them couldn't be decoded. They're unloaded (treated as unloaded) and
/// leave this set when the server sends them again or forgets them.
#[derive(Clone, Component, Debug, Default)]
pub struct UnknownChunks {
    pub chunks: HashSet<ChunkPos>,
    /// Marked in this update; unloaded again after this update's chunk
    /// packets are applied (a chunk packet that came before the bad packet
    /// in the same update must not bring it back).
    pub(crate) pending: Vec<ChunkPos>,
}

/// Per packet id: when it went bad (in the alert window) and when it was
/// last logged.
#[derive(Component, Debug, Default)]
pub struct BadPacketLog {
    ids: HashMap<(ConnectionProtocol, Option<u32>), IdLog>,
}

#[derive(Debug, Default)]
struct IdLog {
    hits: VecDeque<Instant>,
    last_logged: Option<Instant>,
    suppressed: u64,
}

pub struct BadPacketPlugin;
impl Plugin for BadPacketPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BadPacketConfig>()
            .add_message::<BadPacketEvent>()
            .add_message::<BadPacketAlert>()
            .add_systems(
                Update,
                (
                    unload_pending_unknown_chunks.after(handle_receive_chunk_event),
                    inventory_resync_timers,
                    clear_stale_entities_out_of_loaded_chunks,
                ),
            );
    }
}

/// A raw packet read from the server, after the [`RawPacketHooks`].
pub(crate) fn apply_raw_packet_hooks(
    ecs: &World,
    entity: Entity,
    state: ConnectionProtocol,
    raw: Arc<[u8]>,
) -> Arc<[u8]> {
    let Some(hooks) = ecs.get_resource::<RawPacketHooks>() else {
        return raw;
    };
    if hooks.0.is_empty() {
        return raw;
    }
    let mut bytes = raw.to_vec();
    for hook in &hooks.0 {
        hook(entity, state, &mut bytes);
    }
    bytes.into()
}

/// Close the connection of `entity` so that the supervisor reconnects:
/// the socket is shut down (the server sees a disconnect at once), and the
/// next update sends a `DisconnectEvent` with `reason`. Counted as
/// [`DecodeErrorKind::PolicyClose`].
pub fn close_connection(ecs: &mut World, entity: Entity, reason: String) {
    error!("closing the connection: {reason}");
    // decode error hook: a close by the bad packet policy
    record_decode_error(DecodeErrorKind::PolicyClose);
    if let Some(socket) = ecs.get::<ConnectionSocket>(entity) {
        // the duplicate keeps the socket open after the halves are dropped
        let _ = socket.0.shutdown(Shutdown::Both);
    }
    if let Some(mut conn) = ecs.get_mut::<RawConnection>(entity) {
        conn.close(reason);
    }
}

/// A frame error (bad length prefix, inflate failure, size mismatch): the
/// byte stream is lost. Already counted by the reader as its own kind.
pub(crate) fn handle_frame_error(ecs: &mut World, entity: Entity, err: &ReadPacketError) {
    let state = ecs
        .get::<RawConnection>(entity)
        .map(|c| c.state)
        .unwrap_or(ConnectionProtocol::Game);
    let reason = format!("frame error: {err}");
    close_connection(ecs, entity, reason.clone());
    send(
        ecs,
        BadPacketEvent {
            entity,
            state,
            packet_id: None,
            packet_name: None,
            class: None,
            action: BadPacketAction::Closed { reason },
            error: err.to_string(),
            raw: Arc::from([]),
        },
        Some(AlertReason::Closed),
    );
}

/// A packet that couldn't be decoded (already counted by
/// `deserialize_packet` as its own kind): apply the policy for its class.
pub(crate) fn handle_bad_packet(
    ecs: &mut World,
    entity: Entity,
    state: ConnectionProtocol,
    raw: Arc<[u8]>,
    err: &ReadPacketError,
) {
    let mut cursor = Cursor::new(raw.as_ref());
    let packet_id = u32::azalea_read_var(&mut cursor).ok();
    let packet_name = match err {
        ReadPacketError::Parse { packet_name, .. }
        | ReadPacketError::LeftoverData { packet_name, .. } => Some(packet_name.clone()),
        _ => None,
    };
    let class = match (state, &packet_name) {
        (ConnectionProtocol::Game, Some(name)) => classify_game(name),
        (ConnectionProtocol::Configuration, Some(name)) => classify_config(name),
        // login: the session itself; and a packet id that doesn't exist in
        // this state (no name): vanilla closes the connection too
        // (`PacketDecoder` throws "Unknown packet id", which disconnects)
        _ => None,
    };
    let body_start = cursor.position() as usize;
    let body = raw.get(body_start..).unwrap_or_default();
    let what = format!(
        "{state:?} packet {} (id {})",
        packet_name.as_deref().unwrap_or("?"),
        packet_id.map_or("?".to_owned(), |id| id.to_string())
    );

    let action = match class {
        Some(PacketClass::StateFree) => {
            // decode error hook: a state-free packet, dropped
            record_decode_error(DecodeErrorKind::BadPacketDropped);
            BadPacketAction::Dropped
        }
        Some(PacketClass::Inventory) => start_inventory_resync(ecs, entity),
        Some(PacketClass::Entity) => {
            let ids = entity_ids(packet_name.as_deref().unwrap_or_default(), body);
            let own = ecs.get::<MinecraftEntityId>(entity).copied();
            if own.is_some_and(|own| ids.contains(&own)) {
                let reason = format!("bad {what} about our own entity: {err}");
                close_connection(ecs, entity, reason.clone());
                BadPacketAction::Closed { reason }
            } else if packet_name.as_deref() == Some("add_entity") {
                spawn_placeholder(ecs, entity, body)
            } else {
                mark_entities_stale(ecs, entity, &ids, packet_name.as_deref().unwrap_or("?"))
            }
        }
        Some(PacketClass::Chunk) => {
            match chunk_positions(packet_name.as_deref().unwrap_or_default(), body) {
                Some(chunks) => mark_chunks_unknown(ecs, entity, chunks),
                None => {
                    let reason = format!("bad {what}, its chunk can't be read: {err}");
                    close_connection(ecs, entity, reason.clone());
                    BadPacketAction::Closed { reason }
                }
            }
        }
        Some(PacketClass::Critical) | None => {
            let reason = format!("bad {what}, no safe repair: {err}");
            close_connection(ecs, entity, reason.clone());
            BadPacketAction::Closed { reason }
        }
    };

    let alert = match &action {
        BadPacketAction::Closed { .. } => Some(AlertReason::Closed),
        // a lost spawn is a hole in the model: alerted the first time, not
        // only when it repeats
        BadPacketAction::PlaceholderSpawned { .. } => Some(AlertReason::ModelGap),
        BadPacketAction::EntityStale { .. } if packet_name.as_deref() == Some("add_entity") => {
            Some(AlertReason::ModelGap)
        }
        _ => None,
    };
    send(
        ecs,
        BadPacketEvent {
            entity,
            state,
            packet_id,
            packet_name,
            class,
            action,
            error: err.to_string(),
            raw,
        },
        alert,
    );
}

/// Log (at most once per packet id per log interval), count the packet id in
/// the alert window, and send the event and any alert.
fn send(ecs: &mut World, event: BadPacketEvent, alert: Option<AlertReason>) {
    let config = ecs
        .get_resource::<BadPacketConfig>()
        .cloned()
        .unwrap_or_default();
    let now = Instant::now();
    let entity = event.entity;
    let key = (event.state, event.packet_id);

    let (count, log_now, suppressed) = {
        let Ok(mut e) = ecs.get_entity_mut(entity) else {
            return;
        };
        if !e.contains::<BadPacketLog>() {
            e.insert(BadPacketLog::default());
        }
        let Some(mut log) = e.get_mut::<BadPacketLog>() else {
            return;
        };
        let id_log = log.ids.entry(key).or_default();
        id_log.hits.push_back(now);
        while id_log
            .hits
            .front()
            .is_some_and(|&t| now.duration_since(t) > config.alert_window)
        {
            id_log.hits.pop_front();
        }
        let log_now = id_log
            .last_logged
            .is_none_or(|t| now.duration_since(t) >= config.log_interval);
        let suppressed = id_log.suppressed;
        if log_now {
            id_log.last_logged = Some(now);
            id_log.suppressed = 0;
        } else {
            id_log.suppressed += 1;
        }
        (id_log.hits.len(), log_now, suppressed)
    };

    if log_now {
        warn!(
            "bad {:?} packet {} (id {:?}, class {:?}): {}; {:?}; {} more since last logged, {count} in the alert window",
            event.state,
            event.packet_name.as_deref().unwrap_or("?"),
            event.packet_id,
            event.class,
            event.error,
            event.action,
            suppressed,
        );
    } else {
        debug!(
            "bad {:?} packet {:?}: {:?}",
            event.state, event.packet_id, event.action
        );
    }

    let reason = alert.or((count > 1).then_some(AlertReason::Repeated));
    if let Some(reason) = reason {
        ecs.write_message(BadPacketAlert {
            entity,
            state: event.state,
            packet_id: event.packet_id,
            packet_name: event.packet_name.clone(),
            count_in_window: count,
            reason,
        });
    }
    ecs.write_message(event);
}

// --- inventory ---

fn start_inventory_resync(ecs: &mut World, entity: Entity) -> BadPacketAction {
    // decode error hook: inventory or container packet, the inventory is
    // unknown until the resync
    record_decode_error(DecodeErrorKind::InventoryUnknown);
    let container_id = ecs.get::<Inventory>(entity).map(|i| i.id).unwrap_or(0);
    if ecs.get::<InventoryUnknown>(entity).is_none() {
        let now = Instant::now();
        ecs.entity_mut(entity).insert(InventoryUnknown {
            since: now,
            clicks_sent: 0,
        });
        send_resync_click(ecs, entity);
    }
    BadPacketAction::InventoryResync { container_id }
}

/// The resync click: slot -1, PICKUP, button 0 (changes nothing on the
/// server), with a state id the server can't have (ours minus one, mod
/// 32768: server state ids only move forward), for the container we believe
/// is open. The server answers with `broadcastFullState`.
fn send_resync_click(ecs: &mut World, entity: Entity) {
    let Some(inventory) = ecs.get::<Inventory>(entity) else {
        return;
    };
    let packet = ServerboundContainerClick {
        container_id: inventory.id,
        state_id: inventory.state_id.wrapping_add(32767) & 32767,
        slot_num: -1,
        button_num: 0,
        click_type: ClickType::Pickup,
        changed_slots: IndexMap::new(),
        carried_item: inventory.carried.clone(),
    };
    if let Some(mut unknown) = ecs.get_mut::<InventoryUnknown>(entity) {
        unknown.clicks_sent += 1;
    }
    ecs.trigger(SendGamePacketEvent::new(entity, packet));
}

/// Called by the `container_set_content` handler after it applied the
/// content: the full content of the container we have open makes the
/// inventory known again, and the clicks that waited run.
pub(crate) fn container_content_received(ecs: &mut World, entity: Entity, container_id: i32) {
    let Some(inventory) = ecs.get::<Inventory>(entity) else {
        return;
    };
    if inventory.id != container_id {
        return;
    }
    let Some(unknown) = ecs.get::<InventoryUnknown>(entity) else {
        return;
    };
    debug!(
        "inventory known again after {:?} ({} resync clicks)",
        unknown.since.elapsed(),
        unknown.clicks_sent
    );
    ecs.entity_mut(entity).remove::<InventoryUnknown>();
    let pending = ecs
        .get_mut::<PendingClicks>(entity)
        .map(|mut p| std::mem::take(&mut p.0))
        .unwrap_or_default();
    for click in pending {
        ecs.trigger(ContainerClickEvent {
            entity,
            window_id: click.window_id,
            operation: click.operation,
            reflex: false,
        });
    }
}

/// Retry the resync click once, close the connection if the inventory is
/// still unknown after the give-up time, and drop clicks that waited too
/// long.
fn inventory_resync_timers(
    mut commands: Commands,
    config: Res<BadPacketConfig>,
    mut query: Query<(Entity, &InventoryUnknown, Option<&mut PendingClicks>)>,
) {
    for (entity, unknown, pending) in &mut query {
        let elapsed = unknown.since.elapsed();
        if let Some(mut pending) = pending {
            pending.0.retain(|c| c.at.elapsed() < config.resync_give_up);
        }
        if elapsed >= config.resync_give_up {
            let clicks = unknown.clicks_sent;
            commands.queue(move |ecs: &mut World| resync_failed(ecs, entity, elapsed, clicks));
        } else if elapsed >= config.resync_retry && unknown.clicks_sent < 2 {
            commands.queue(move |ecs: &mut World| send_resync_click(ecs, entity));
        }
    }
}

fn resync_failed(ecs: &mut World, entity: Entity, elapsed: Duration, clicks: u8) {
    if ecs.get::<InventoryUnknown>(entity).is_none() {
        return;
    }
    ecs.entity_mut(entity)
        .remove::<(InventoryUnknown, PendingClicks)>();
    // decode error hook: the resync didn't come
    record_decode_error(DecodeErrorKind::InventoryResyncFailed);
    let reason = format!("inventory still unknown after {elapsed:?} and {clicks} resync clicks");
    close_connection(ecs, entity, reason.clone());
    let state = ecs
        .get::<RawConnection>(entity)
        .map(|c| c.state)
        .unwrap_or(ConnectionProtocol::Game);
    send(
        ecs,
        BadPacketEvent {
            entity,
            state,
            packet_id: None,
            packet_name: None,
            class: Some(PacketClass::Inventory),
            action: BadPacketAction::Closed { reason },
            error: "inventory resync timed out".to_owned(),
            raw: Arc::from([]),
        },
        Some(AlertReason::ResyncFailed),
    );
}

/// Queue an ordinary click that came while the inventory is unknown; it
/// runs once the inventory is known again (reflex clicks never come here).
pub(crate) fn defer_click(
    ecs: &mut World,
    entity: Entity,
    window_id: i32,
    operation: ClickOperation,
) {
    debug!("click waits for the inventory resync: {operation:?}");
    let Ok(mut e) = ecs.get_entity_mut(entity) else {
        return;
    };
    if !e.contains::<PendingClicks>() {
        e.insert(PendingClicks::default());
    }
    if let Some(mut pending) = e.get_mut::<PendingClicks>() {
        pending.0.push(PendingClick {
            at: Instant::now(),
            window_id,
            operation,
        });
    }
}

// --- entities ---

/// The entity ids a packet is about, read from the start of its body (as
/// many as can be read).
fn entity_ids(name: &str, body: &[u8]) -> Vec<MinecraftEntityId> {
    let mut buf = Cursor::new(body);
    let var = |buf: &mut Cursor<&[u8]>| i32::azalea_read_var(buf).ok().map(MinecraftEntityId);
    let fixed = |buf: &mut Cursor<&[u8]>| i32::azalea_read(buf).ok().map(MinecraftEntityId);
    match name {
        // a fixed-width int
        "entity_event" => fixed(&mut buf).into_iter().collect(),
        "set_entity_link" => [fixed(&mut buf), fixed(&mut buf)]
            .into_iter()
            .flatten()
            .collect(),
        "remove_entities" | "set_passengers" => {
            let mut ids = Vec::new();
            if name == "set_passengers" {
                ids.extend(var(&mut buf));
            }
            let Some(n) = u32::azalea_read_var(&mut buf).ok() else {
                return ids;
            };
            for _ in 0..n {
                let Some(id) = var(&mut buf) else { break };
                ids.push(id);
            }
            ids
        }
        // item entity first, then the collector
        "take_item_entity" => [var(&mut buf), var(&mut buf)]
            .into_iter()
            .flatten()
            .collect(),
        _ => var(&mut buf).into_iter().collect(),
    }
}

fn mark_entities_stale(
    ecs: &mut World,
    player: Entity,
    ids: &[MinecraftEntityId],
    packet: &str,
) -> BadPacketAction {
    // decode error hook: a packet about another entity, the entity keeps its
    // last good state
    record_decode_error(DecodeErrorKind::StaleEntity);
    let found: Vec<(MinecraftEntityId, Entity)> = match ecs.get::<EntityIdIndex>(player) {
        Some(index) => ids
            .iter()
            .filter_map(|&id| Some((id, index.get_by_minecraft_entity(id)?)))
            .collect(),
        None => Vec::new(),
    };
    let now = Instant::now();
    for &(_, e) in &found {
        if let Ok(mut e) = ecs.get_entity_mut(e)
            && !e.contains::<StaleEntity>()
        {
            e.insert(StaleEntity {
                since: now,
                packet: packet.to_owned(),
            });
        }
    }
    BadPacketAction::EntityStale {
        entities: found.into_iter().map(|(id, _)| id).collect(),
    }
}

/// An `add_entity` that couldn't be decoded: read its body field by field
/// (id, uuid, type, position, then the rest), and spawn the entity from what
/// could be read, marked stale, so that later packets for its id apply to
/// something and the hole in the model is visible. Nothing is spawned if not
/// even the id can be read.
fn spawn_placeholder(ecs: &mut World, player: Entity, body: &[u8]) -> BadPacketAction {
    // decode error hook: a lost spawn, the entity is a placeholder (or
    // missing if its id can't be read)
    record_decode_error(DecodeErrorKind::StaleEntity);
    let mut buf = Cursor::new(body);
    let Ok(id) = i32::azalea_read_var(&mut buf).map(MinecraftEntityId) else {
        return BadPacketAction::EntityStale { entities: vec![] };
    };
    let uuid = Uuid::azalea_read(&mut buf).ok();
    let kind = uuid.and(EntityKind::azalea_read(&mut buf).ok());
    let position = kind.and(Vec3::azalea_read(&mut buf).ok());
    let rest = position.and_then(|_| {
        Some((
            i8::azalea_read(&mut buf).ok()?,
            i8::azalea_read(&mut buf).ok()?,
            i8::azalea_read(&mut buf).ok()?,
            i32::azalea_read_var(&mut buf).ok()?,
            QuantizedVelocity::azalea_read(&mut buf).ok()?,
        ))
    });
    let read = PlaceholderEntity {
        uuid_known: uuid.is_some(),
        kind_known: kind.is_some(),
        position_known: position.is_some(),
        rest_known: rest.is_some(),
    };
    let (x_rot, y_rot, y_head_rot, data, movement) = rest.unwrap_or_default();
    let packet = ClientboundAddEntity {
        id,
        // not a real player's uuid (version 0), unique per entity id
        uuid: uuid.unwrap_or(Uuid::from_u64_pair(
            0x6875_6e74_0000_0000,
            id.0 as u32 as u64,
        )),
        entity_type: kind.unwrap_or(EntityKind::Marker),
        position: position.unwrap_or_default(),
        x_rot,
        y_rot,
        y_head_rot,
        data,
        movement,
    };

    let existed = ecs
        .get::<EntityIdIndex>(player)
        .and_then(|index| index.get_by_minecraft_entity(id))
        .is_some();
    GamePacketHandler { ecs, player }.add_entity(&packet);
    // decode error hook: counted as its own outcome too
    record_decode_error(DecodeErrorKind::PlaceholderEntity);
    let spawned = ecs
        .get::<EntityIdIndex>(player)
        .and_then(|index| index.get_by_minecraft_entity(id));
    if let Some(e) = spawned
        && !existed
        && let Ok(mut e) = ecs.get_entity_mut(e)
    {
        e.insert((
            StaleEntity {
                since: Instant::now(),
                packet: "add_entity".to_owned(),
            },
            read.clone(),
        ));
    }
    BadPacketAction::PlaceholderSpawned { id, read }
}

/// Called by the `remove_entities` handler.
pub(crate) fn entity_removed(ecs: &mut World, entity: Entity) {
    if let Ok(mut e) = ecs.get_entity_mut(entity) {
        e.remove::<StaleEntity>();
    }
}

/// A stale entity whose chunk unloads isn't stale anymore (the server sends
/// it again, whole, if the chunk comes back). Only entities that were in a
/// loaded chunk and left it: a new placeholder isn't in one yet.
fn clear_stale_entities_out_of_loaded_chunks(
    mut commands: Commands,
    mut left_loaded_chunks: RemovedComponents<InLoadedChunk>,
    query: Query<Option<&PlaceholderEntity>, With<StaleEntity>>,
) {
    for entity in left_loaded_chunks.read() {
        let Ok(placeholder) = query.get(entity) else {
            continue;
        };
        // a placeholder whose position couldn't be read is at the origin, so
        // the chunk it's in says nothing about it
        if placeholder.is_some_and(|p| !p.position_known) {
            continue;
        }
        commands.entity(entity).remove::<StaleEntity>();
    }
}

// --- chunks ---

/// The chunks a chunk or block packet is about, read from the start of its
/// body. `None` if not even one position can be read.
fn chunk_positions(name: &str, body: &[u8]) -> Option<Vec<ChunkPos>> {
    let mut buf = Cursor::new(body);
    let one = match name {
        "level_chunk_with_light" => {
            let x = i32::azalea_read(&mut buf).ok()?;
            let z = i32::azalea_read(&mut buf).ok()?;
            ChunkPos::new(x, z)
        }
        "forget_level_chunk" => ChunkPos::azalea_read(&mut buf).ok()?,
        "block_update" | "block_entity_data" => {
            ChunkPos::from(BlockPos::azalea_read(&mut buf).ok()?)
        }
        "section_blocks_update" => ChunkPos::from(ChunkSectionPos::azalea_read(&mut buf).ok()?),
        "chunks_biomes" => {
            let n = u32::azalea_read_var(&mut buf).ok()?;
            let mut chunks = Vec::new();
            for _ in 0..n {
                let Ok(pos) = ChunkPos::azalea_read(&mut buf) else {
                    break;
                };
                chunks.push(pos);
                let Ok(len) = u32::azalea_read_var(&mut buf) else {
                    break;
                };
                buf.set_position(buf.position().saturating_add(u64::from(len)));
            }
            return (!chunks.is_empty()).then_some(chunks);
        }
        _ => return None,
    };
    Some(vec![one])
}

fn mark_chunks_unknown(ecs: &mut World, player: Entity, chunks: Vec<ChunkPos>) -> BadPacketAction {
    // decode error hook: a chunk or block packet, the chunk is unknown
    record_decode_error(DecodeErrorKind::ChunkUnknown);
    unload_chunks(ecs, player, &chunks);
    let mut e = ecs.entity_mut(player);
    if !e.contains::<UnknownChunks>() {
        e.insert(UnknownChunks::default());
    }
    if let Some(mut unknown) = e.get_mut::<UnknownChunks>() {
        for &pos in &chunks {
            unknown.chunks.insert(pos);
            unknown.pending.push(pos);
        }
    }
    BadPacketAction::ChunkUnknown { chunks }
}

fn unload_chunks(ecs: &World, player: Entity, chunks: &[ChunkPos]) {
    let Some(world_holder) = ecs.get::<WorldHolder>(player) else {
        return;
    };
    let mut partial = world_holder.partial.write();
    for pos in chunks {
        partial.chunks.limited_set(pos, None);
    }
}

/// Called when the server sends a chunk again or forgets it: it isn't
/// unknown anymore.
pub(crate) fn chunk_known(unknown: Option<Mut<UnknownChunks>>, pos: &ChunkPos) {
    if let Some(mut unknown) = unknown
        && unknown.chunks.contains(pos)
        && !unknown.pending.contains(pos)
    {
        unknown.chunks.remove(pos);
    }
}

fn unload_pending_unknown_chunks(mut query: Query<(&WorldHolder, &mut UnknownChunks)>) {
    for (world_holder, mut unknown) in &mut query {
        if unknown.pending.is_empty() {
            continue;
        }
        let pending = std::mem::take(&mut unknown.pending);
        let mut partial = world_holder.partial.write();
        for pos in &pending {
            partial.chunks.limited_set(pos, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_clientbound_game_packet_has_a_class() {
        let src = include_str!("../../../azalea-protocol/src/packets/game/mod.rs");
        let start = src.find("Clientbound => [").unwrap();
        let end = start + src[start..].find(']').unwrap();
        let names: Vec<&str> = src[start + "Clientbound => [".len()..end]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        assert!(names.len() > 100, "{names:?}");
        for name in names {
            assert!(classify_game(name).is_some(), "{name} has no class");
        }
    }

    #[test]
    fn every_clientbound_config_packet_has_a_class() {
        let src = include_str!("../../../azalea-protocol/src/packets/config/mod.rs");
        let start = src.find("Clientbound => [").unwrap();
        let end = start + src[start..].find(']').unwrap();
        let names: Vec<&str> = src[start + "Clientbound => [".len()..end]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        assert!(names.len() > 15, "{names:?}");
        for name in names {
            assert!(classify_config(name).is_some(), "{name} has no class");
        }
    }

    #[test]
    fn positions_are_read_from_the_body() {
        let mut body = Vec::new();
        BlockPos::new(-17, 70, 33).azalea_write(&mut body).unwrap();
        assert_eq!(
            chunk_positions("block_update", &body),
            Some(vec![ChunkPos::new(-2, 2)])
        );
        let mut body = Vec::new();
        5i32.azalea_write(&mut body).unwrap();
        (-9i32).azalea_write(&mut body).unwrap();
        assert_eq!(
            chunk_positions("level_chunk_with_light", &body),
            Some(vec![ChunkPos::new(5, -9)])
        );
        assert_eq!(chunk_positions("block_update", &[1, 2]), None);

        let mut body = Vec::new();
        2u32.azalea_write_var(&mut body).unwrap();
        7i32.azalea_write_var(&mut body).unwrap();
        // truncated after the first id
        assert_eq!(
            entity_ids("remove_entities", &body),
            vec![MinecraftEntityId(7)]
        );
        assert_eq!(
            entity_ids("entity_event", &[0, 0, 1, 0]),
            vec![MinecraftEntityId(256)]
        );
    }
}
