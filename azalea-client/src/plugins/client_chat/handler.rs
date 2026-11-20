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
) {
    for event in events.read() {
        let content = event
            .content
            .chars()
            .filter(|c| !matches!(c, '\x00'..='\x1F' | '\x7F' | '§'))
            .take(256)
            .collect::<String>();

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
