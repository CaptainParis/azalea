use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "online-mode")]
use azalea_protocol::packets::game::{
    ServerboundChatCommandSigned, s_chat_command_signed::ArgumentSignature,
};
use azalea_protocol::packets::{
    Packet,
    game::{ServerboundChat, ServerboundChatCommand},
};
use bevy_ecs::prelude::*;
use tracing::debug;
#[cfg(feature = "online-mode")]
use tracing::warn;

use super::ChatKind;
#[cfg(feature = "online-mode")]
use crate::{account::Account, chat_signing::ChatSigningSession, secure_chat::CommandTree};
use crate::{
    packet::game::SendGamePacketEvent,
    secure_chat::{SecureChatMutation, SecureChatState},
};

/// Send a chat packet to the server of a specific kind (chat message or
/// command). Usually you just want [`SendChatEvent`] instead.
///
/// Usually setting the kind to `Message` will make it send a chat message even
/// if it starts with a slash, but some server implementations will always do a
/// command if it starts with a slash.
///
/// If you're wondering why this isn't two separate events, it's so ordering is
/// preserved if multiple chat messages and commands are sent at the same time.
///
/// [`SendChatEvent`]: super::SendChatEvent
#[derive(Message)]
pub struct SendChatKindEvent {
    pub entity: Entity,
    pub content: String,
    pub kind: ChatKind,
}

/// The text as sent: control characters and `§` removed, 256 characters.
fn clean(content: &str) -> String {
    content
        .chars()
        .filter(|c| !matches!(c, '\x00'..='\x1F' | '\x7F' | '§'))
        .take(256)
        .collect()
}

/// The longest a held message waits. Past it, a client in game sends it
/// anyway (signed with the session it has, as vanilla, which never holds);
/// a client out of game (configuration) drops it.
pub const HOLD_CAP: std::time::Duration = std::time::Duration::from_secs(10);
/// Held messages per client at most (the oldest are dropped beyond).
pub const HOLD_MAX: usize = 64;

/// The client's chat session as the hold sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionView {
    /// No chat session: chat goes unsigned, nothing to wait for.
    None,
    /// Announced less than `SESSION_SETTLE` ago.
    Unsettled,
    Settled,
}

/// What the hold needs to know about one client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientView {
    /// The entity still exists.
    pub exists: bool,
    /// In game (has its secure chat state; not in configuration).
    pub in_game: bool,
    pub session: SessionView,
}

/// Signed chat held after a chat session update (see
/// [`crate::chat_signing::SESSION_SETTLE`]). Only messages that would be
/// signed are held; a held queue is released in order when the session
/// settles, when the session goes away (they go unsigned, as vanilla without
/// a session), or at [`HOLD_CAP`]; a despawned client's entries are dropped.
#[derive(Debug, Default)]
pub struct ChatHold {
    held: std::collections::HashMap<
        Entity,
        std::collections::VecDeque<(String, ChatKind, std::time::Instant)>,
    >,
}

impl ChatHold {
    /// Entries that may go now, in order per client, and how many were
    /// dropped.
    pub fn release(
        &mut self,
        now: std::time::Instant,
        view: &dyn Fn(Entity) -> ClientView,
    ) -> (Vec<(Entity, String, ChatKind)>, usize) {
        let mut out = Vec::new();
        let mut dropped = 0;
        for (e, q) in self.held.iter_mut() {
            let v = view(*e);
            if !v.exists {
                dropped += q.len();
                q.clear();
                continue;
            }
            while let Some((_, _, since)) = q.front() {
                let free = v.in_game && v.session != SessionView::Unsettled;
                let over = now.saturating_duration_since(*since) >= HOLD_CAP;
                if !(free || over) {
                    break;
                }
                let Some((c, k, _)) = q.pop_front() else {
                    break;
                };
                if v.in_game {
                    out.push((*e, c, k));
                } else {
                    dropped += 1;
                }
            }
        }
        self.held.retain(|_, q| !q.is_empty());
        (out, dropped)
    }

    /// A new message: `Some` to send now, `None` held. `signed`: it would be
    /// signed if sent now.
    pub fn offer(
        &mut self,
        e: Entity,
        content: String,
        kind: ChatKind,
        signed: bool,
        v: ClientView,
        now: std::time::Instant,
    ) -> Option<(Entity, String, ChatKind)> {
        let waiting = self.held.get(&e).is_some_and(|q| !q.is_empty());
        if !signed || !(waiting || v.session == SessionView::Unsettled) {
            return Some((e, content, kind));
        }
        let q = self.held.entry(e).or_default();
        if q.len() >= HOLD_MAX {
            q.pop_front();
        }
        q.push_back((content, kind, now));
        None
    }

    /// Messages held for `e`.
    pub fn held(&self, e: Entity) -> usize {
        self.held.get(&e).map_or(0, |q| q.len())
    }
}

fn millis(t: SystemTime) -> u64 {
    // network-safe: our own clock, not network data; u64 milliseconds last for
    // 584 million years
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// As vanilla's `sendChat` and `sendCommand`: a chat message carries the
/// last-seen update (offset and window) and, with a chat session, a signature
/// over its text and the window; a command whose tree marks message
/// arguments is sent as `ServerboundChatCommandSigned` with one signature
/// per such argument (each advancing the session's chain), else unsigned.
#[allow(clippy::type_complexity)]
pub fn handle_send_chat_kind_event(
    mut events: MessageReader<SendChatKindEvent>,
    mut commands: Commands,
    mut chat: Query<(Option<&mut SecureChatState>, Option<&SecureChatMutation>)>,
    #[cfg(feature = "online-mode")] mut signing: Query<(
        &Account,
        &mut ChatSigningSession,
        Option<&CommandTree>,
    )>,
    #[cfg(feature = "online-mode")] mut held: Local<ChatHold>,
) {
    // Signed chat right after a chat session update waits until the server
    // has surely applied it (`SESSION_SETTLE`); order is kept per client
    // among held messages. Unsigned chat and commands never wait (P3), and
    // nothing waits forever (see `ChatHold`).
    let mut todo: Vec<(Entity, String, ChatKind)> = Vec::new();
    #[cfg(feature = "online-mode")]
    {
        let now = std::time::Instant::now();
        let view = |e: Entity| ClientView {
            exists: chat.get(e).is_ok(),
            in_game: chat.get(e).is_ok_and(|(s, _)| s.is_some()),
            session: match signing.get(e) {
                Err(_) => SessionView::None,
                Ok((_, s, _)) if s.announced_at.elapsed() < crate::chat_signing::SESSION_SETTLE => {
                    SessionView::Unsettled
                }
                Ok(_) => SessionView::Settled,
            },
        };
        let (released, dropped) = held.release(now, &view);
        if dropped > 0 {
            warn!(
                "{dropped} held chat messages dropped (client gone, or out of game past the hold cap)"
            );
        }
        todo.extend(released);
        for event in events.read() {
            let signed = match event.kind {
                ChatKind::Message => signing.get(event.entity).is_ok(),
                ChatKind::Command => signing.get(event.entity).is_ok_and(|(_, _, tree)| {
                    tree.and_then(|t| {
                        crate::secure_chat::signable_arguments(&t.0, &clean(&event.content))
                    })
                    .is_some_and(|a| !a.is_empty())
                }),
            };
            if let Some(go) = held.offer(
                event.entity,
                event.content.clone(),
                event.kind,
                signed,
                view(event.entity),
                now,
            ) {
                todo.push(go);
            }
        }
    }
    #[cfg(not(feature = "online-mode"))]
    todo.extend(events.read().map(|e| (e.entity, e.content.clone(), e.kind)));

    for (entity, raw, kind) in todo {
        let event = SendChatKindEvent {
            entity,
            content: raw,
            kind,
        };
        let content = clean(&event.content);

        let timestamp = SystemTime::now();
        let Ok((mut state, mutation)) = chat.get_mut(event.entity) else {
            continue;
        };

        let packet = match event.kind {
            ChatKind::Message => {
                let salt = azalea_crypto::signing::make_salt();
                let update = state
                    .as_deref_mut()
                    .map(|s| s.next_update(mutation))
                    .unwrap_or_else(|| crate::secure_chat::LastSeenUpdate {
                        last_seen: Vec::new(),
                        update: Default::default(),
                    });

                #[cfg(feature = "online-mode")]
                let signature =
                    if let Ok((account, mut chat_session, _)) = signing.get_mut(event.entity) {
                        if let Some(s) = state.as_deref_mut() {
                            s.signed_sent += 1;
                        }
                        Some(create_signature(
                            account,
                            &mut chat_session,
                            salt,
                            timestamp,
                            &content,
                            &update.last_seen,
                        ))
                    } else {
                        None
                    };
                #[cfg(not(feature = "online-mode"))]
                let signature = None;

                ServerboundChat {
                    message: content,
                    timestamp: millis(timestamp),
                    salt,
                    signature,
                    last_seen_messages: update.update,
                }
                .into_variant()
            }
            ChatKind::Command => {
                #[cfg(feature = "online-mode")]
                let signed = signing.get_mut(event.entity).ok().and_then(
                    |(account, mut chat_session, tree)| {
                        let args = crate::secure_chat::signable_arguments(&tree?.0, &content)?;
                        if args.is_empty() {
                            return None;
                        }
                        let state = state.as_deref_mut()?;
                        let salt = azalea_crypto::signing::make_salt();
                        let update = state.next_update(mutation);
                        state.signed_sent += 1;
                        let argument_signatures = args
                            .into_iter()
                            .map(|a| ArgumentSignature {
                                signature: create_signature(
                                    account,
                                    &mut chat_session,
                                    salt,
                                    timestamp,
                                    &a.value,
                                    &update.last_seen,
                                ),
                                name: a.name,
                            })
                            .collect();
                        Some(
                            ServerboundChatCommandSigned {
                                command: content.clone(),
                                timestamp: millis(timestamp),
                                salt,
                                argument_signatures,
                                last_seen_messages: update.update,
                            }
                            .into_variant(),
                        )
                    },
                );
                #[cfg(not(feature = "online-mode"))]
                let signed = None;
                match signed {
                    Some(p) => p,
                    None => {
                        if let Some(s) = state.as_deref_mut() {
                            s.unsigned_commands += 1;
                        }
                        debug!("sending command unsigned: {content}");
                        ServerboundChatCommand { command: content }.into_variant()
                    }
                }
            }
        };

        commands.trigger(SendGamePacketEvent::new(event.entity, packet));
    }
}

#[cfg(feature = "online-mode")]
pub fn create_signature(
    account: &Account,
    chat_session: &mut ChatSigningSession,
    salt: u64,
    timestamp: SystemTime,
    message: &str,
    last_seen: &[azalea_crypto::signing::MessageSignature],
) -> azalea_crypto::signing::MessageSignature {
    use azalea_crypto::signing::SignChatMessageOptions;

    let signature = azalea_crypto::signing::sign_chat_message(&SignChatMessageOptions {
        account_uuid: account.uuid(),
        chat_session_uuid: chat_session.session_id,
        message_index: chat_session.messages_sent,
        salt,
        timestamp,
        message: message.to_owned(),
        private_key: chat_session.private_key.clone(),
        last_seen: last_seen.to_vec(),
    });

    chat_session.messages_sent += 1;

    signature
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn e(n: u32) -> Entity {
        Entity::from_raw_u32(n).unwrap()
    }
    const UNSETTLED: ClientView = ClientView {
        exists: true,
        in_game: true,
        session: SessionView::Unsettled,
    };
    const SETTLED: ClientView = ClientView {
        session: SessionView::Settled,
        ..UNSETTLED
    };
    const NO_SESSION: ClientView = ClientView {
        session: SessionView::None,
        ..UNSETTLED
    };

    fn names(v: &[(Entity, String, ChatKind)]) -> Vec<&str> {
        v.iter().map(|(_, c, _)| c.as_str()).collect()
    }

    #[test]
    fn held_until_settled_in_order() {
        let mut h = ChatHold::default();
        let t = Instant::now();
        assert!(
            h.offer(e(1), "a".into(), ChatKind::Message, true, UNSETTLED, t)
                .is_none()
        );
        assert!(
            h.offer(e(1), "b".into(), ChatKind::Message, true, UNSETTLED, t)
                .is_none()
        );
        let (r, d) = h.release(t + Duration::from_millis(500), &|_| UNSETTLED);
        assert!(r.is_empty() && d == 0);
        // settled, but a queue exists: a new signed message goes behind it
        assert!(
            h.offer(e(1), "c".into(), ChatKind::Message, true, SETTLED, t)
                .is_none()
        );
        let (r, _) = h.release(t + Duration::from_secs(2), &|_| SETTLED);
        assert_eq!(names(&r), ["a", "b", "c"]);
        assert_eq!(h.held(e(1)), 0);
    }

    #[test]
    fn unsigned_never_waits() {
        // P3: an unsigned command (no signable argument) or chat without a
        // session is never blocked by signing (a failed cert fetch, a held
        // queue)
        let mut h = ChatHold::default();
        let t = Instant::now();
        assert!(
            h.offer(e(1), "a".into(), ChatKind::Message, true, UNSETTLED, t)
                .is_none()
        );
        let go = h.offer(
            e(1),
            "/tpa x".into(),
            ChatKind::Command,
            false,
            UNSETTLED,
            t,
        );
        assert!(go.is_some(), "unsigned command goes at once");
        let go = h.offer(e(2), "hi".into(), ChatKind::Message, false, NO_SESSION, t);
        assert!(go.is_some());
    }

    #[test]
    fn session_gone_releases_unsigned() {
        // a held queue when the chat session goes away (cert fetch failed and
        // the session was dropped, a chain repair): released at once
        let mut h = ChatHold::default();
        let t = Instant::now();
        h.offer(e(1), "a".into(), ChatKind::Message, true, UNSETTLED, t);
        let (r, d) = h.release(t + Duration::from_millis(100), &|_| NO_SESSION);
        assert_eq!(names(&r), ["a"]);
        assert_eq!(d, 0);
    }

    #[test]
    fn hold_is_capped() {
        // a session that keeps being re-announced never settles: the cap
        // sends anyway; out of game (configuration) past the cap: dropped
        let mut h = ChatHold::default();
        let t = Instant::now();
        h.offer(e(1), "a".into(), ChatKind::Message, true, UNSETTLED, t);
        h.offer(e(2), "b".into(), ChatKind::Message, true, UNSETTLED, t);
        let config = ClientView {
            in_game: false,
            session: SessionView::None,
            ..UNSETTLED
        };
        let view = |x: Entity| if x == e(1) { UNSETTLED } else { config };
        let (r, d) = h.release(t + HOLD_CAP - Duration::from_millis(1), &view);
        assert!(
            r.is_empty() && d == 0,
            "in configuration it waits (a server switch)"
        );
        let (r, d) = h.release(t + HOLD_CAP, &view);
        assert_eq!(names(&r), ["a"]);
        assert_eq!(d, 1);
        assert_eq!(h.held(e(1)) + h.held(e(2)), 0);
        // bounded per client
        for i in 0..(HOLD_MAX + 5) {
            h.offer(e(3), format!("{i}"), ChatKind::Message, true, UNSETTLED, t);
        }
        assert_eq!(h.held(e(3)), HOLD_MAX);
    }

    #[test]
    fn despawned_client_entries_are_dropped() {
        let mut h = ChatHold::default();
        let t = Instant::now();
        h.offer(e(1), "a".into(), ChatKind::Message, true, UNSETTLED, t);
        h.offer(e(1), "b".into(), ChatKind::Message, true, UNSETTLED, t);
        let gone = ClientView {
            exists: false,
            in_game: false,
            session: SessionView::None,
        };
        let (r, d) = h.release(t, &|_| gone);
        assert!(r.is_empty());
        assert_eq!(d, 2);
        assert_eq!(h.held(e(1)), 0);
    }
}
