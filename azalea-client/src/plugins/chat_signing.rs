use std::time::{Duration, Instant};

use azalea_auth::certs::{Certificates, FetchCertificatesError};
use azalea_protocol::packets::game::{
    ServerboundChatSessionUpdate,
    s_chat_session_update::{ProfilePublicKeyData, RemoteChatSessionData},
};
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_tasks::{Task, futures_lite::future};
use chrono::Utc;
use tracing::{debug, error};
use uuid::Uuid;

use super::{client_chat, login::IsAuthenticated, packet::game::SendGamePacketEvent};
use crate::{InGameState, account::Account};

pub struct ChatSigningPlugin;
impl Plugin for ChatSigningPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                request_certs_if_needed,
                poll_request_certs_task,
                handle_queued_certs_to_send,
            )
                .chain()
                .before(client_chat::handler::handle_send_chat_kind_event),
        );
    }
}

#[derive(Component)]
pub struct RequestCertsTask(pub Task<Result<Certificates, FetchCertificatesError>>);

/// A component that makes us have to wait until the given time to refresh the
/// certs.
///
/// This is used to avoid spamming requests if requesting certs fails. Usually,
/// we just check [`Certificates::expires_at`].
#[derive(Component, Debug)]
pub struct OnlyRefreshCertsAfter {
    pub refresh_at: Instant,
}
/// A component that's present when this client has sent its certificates to the
/// server.
///
/// This should be removed if you want to re-send the certs.
///
/// If you want to get the client's actual certificates, you can get that from
/// the `certs` in the [`Account`] component.
#[derive(Component)]
pub struct ChatSigningSession {
    pub session_id: Uuid,
    pub messages_sent: u32,
    /// The key this session was announced with (a different key in the
    /// account starts a new session).
    pub public_key_der: Vec<u8>,
    /// Its private key: messages are signed with the session's key, not with
    /// a key the account renewed but hasn't announced yet.
    pub private_key: azalea_auth::certs::RsaPrivateKey,
    /// When the session was sent. Signed chat waits [`SESSION_SETTLE`] after
    /// it: Paper applies the update on its main thread but decodes chat on
    /// its network threads, so a message signed with a session the server
    /// hasn't applied yet is refused and breaks the chain until the next
    /// session.
    pub announced_at: Instant,
}

/// How long signed chat waits after a chat session update (see
/// [`ChatSigningSession::announced_at`]).
pub const SESSION_SETTLE: Duration = Duration::from_millis(1500);

pub fn poll_request_certs_task(
    mut commands: Commands,
    mut query: Query<(Entity, &mut RequestCertsTask, &Account)>,
) {
    for (entity, mut auth_task, account) in query.iter_mut() {
        if let Some(poll_res) = future::block_on(future::poll_once(&mut auth_task.0)) {
            debug!("Finished requesting certs");
            commands.entity(entity).remove::<RequestCertsTask>();

            match poll_res {
                Ok(certs) => {
                    commands.entity(entity).insert(QueuedCertsToSend {
                        certs: certs.clone(),
                    });
                    account.set_certs(certs);
                }
                Err(err) => {
                    // an hour, or sooner if the current key runs out first
                    let wait = Duration::from_secs(60 * 60);
                    error!("Error requesting certs: {err:?}. Retrying in {wait:?}.");

                    commands.entity(entity).insert(OnlyRefreshCertsAfter {
                        refresh_at: Instant::now() + wait,
                    });
                }
            }
        }
    }
}

/// How long before the certificates expire the client renews them (WP3: 6 h,
/// so a 36 h queue plus an 8 h session never chats on an expired key).
/// Insert it on a client to change it.
#[derive(Clone, Component, Copy, Debug)]
pub struct CertRefreshMargin(pub Duration);
impl Default for CertRefreshMargin {
    fn default() -> Self {
        Self(Duration::from_secs(6 * 3600))
    }
}

fn due(certs: &Certificates, margin: Duration) -> bool {
    let margin = chrono::Duration::from_std(margin).unwrap_or(chrono::Duration::MAX);
    certs
        .expires_at
        .checked_sub_signed(margin)
        .is_none_or(|t| Utc::now() >= t)
}

#[allow(clippy::type_complexity)]
pub fn request_certs_if_needed(
    mut commands: Commands,
    mut query: Query<
        (
            Entity,
            &Account,
            Option<&OnlyRefreshCertsAfter>,
            Option<&ChatSigningSession>,
            Option<&CertRefreshMargin>,
        ),
        (
            Without<RequestCertsTask>,
            Without<QueuedCertsToSend>,
            With<InGameState>,
            With<IsAuthenticated>,
            // the play Login of this connection was handled (vanilla sends the
            // session from handleLogin; the configuration's end comes earlier)
            With<crate::secure_chat::SecureChatState>,
        ),
    >,
) {
    for (entity, account, only_refresh_certs_after, chat_signing_session, margin) in
        query.iter_mut()
    {
        let margin = margin.copied().unwrap_or_default().0;
        if let Some(certs) = account.certs()
            && !due(&certs, margin)
        {
            // valid certs: send them if this connection has no chat session
            // yet, or if the account's key changed (renewed elsewhere): a new
            // chat session with the new key, as vanilla's setKeyPair
            let send = chat_signing_session.is_none_or(|s| s.public_key_der != certs.public_key_der);
            if send {
                commands.entity(entity).insert(QueuedCertsToSend { certs });
            }
            continue;
        }
        if let Some(only_refresh_certs_after) = only_refresh_certs_after
            && only_refresh_certs_after.refresh_at > Instant::now()
        {
            continue;
        }

        debug!("Started task to fetch certs");
        let cap = crate::task_pool::owner::CERTS_CAP;
        let account = account.clone();
        let task = crate::task_pool::owner::spawn_io(async_compat::Compat::new(async move {
            tokio::time::timeout(cap, account.fetch_certificates())
                .await
                .unwrap_or(Err(azalea_auth::certs::FetchCertificatesError::TimedOut(cap)))
        }));
        commands
            .entity(entity)
            .insert(RequestCertsTask(task))
            .remove::<OnlyRefreshCertsAfter>();
    }
}

/// A component that's present on players that should send their chat signing
/// certificates as soon as possible.
///
/// This is removed when the certificates get sent.
#[derive(Component)]
pub struct QueuedCertsToSend {
    pub certs: Certificates,
}

pub fn handle_queued_certs_to_send(
    mut commands: Commands,
    query: Query<
        (Entity, &QueuedCertsToSend),
        (With<IsAuthenticated>, With<crate::secure_chat::SecureChatState>),
    >,
) {
    for (entity, queued_certs) in &query {
        let certs = &queued_certs.certs;

        let session_id = Uuid::new_v4();

        let chat_session = RemoteChatSessionData {
            session_id,
            profile_public_key: ProfilePublicKeyData {
                expires_at: certs.expires_at.timestamp_millis() as u64,
                key: certs.public_key_der.clone(),
                key_signature: certs.signature_v2.clone(),
            },
        };

        debug!("Sending chat signing certs to server");

        commands.trigger(SendGamePacketEvent::new(
            entity,
            ServerboundChatSessionUpdate { chat_session },
        ));
        commands
            .entity(entity)
            .remove::<QueuedCertsToSend>()
            .insert(ChatSigningSession {
                session_id,
                messages_sent: 0,
                public_key_der: certs.public_key_der.clone(),
                private_key: certs.private_key.clone(),
                announced_at: Instant::now(),
            });
    }
}
