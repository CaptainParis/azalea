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
        app.add_message::<ChatChainBrokenEvent>().add_systems(
            Update,
            (
                detect_broken_chain,
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
                    // announced by request_certs_if_needed if the key differs
                    // from the session's (Mojang returns the same key until
                    // refreshedAfter: never announce it again); no new fetch
                    // before refreshedAfter, when Mojang gives a new key
                    commands.entity(entity).insert(OnlyRefreshCertsAfter {
                        refresh_at: refetch_at(certs.refresh_after, Utc::now(), Instant::now()),
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

/// The server's system messages for a broken signed chat chain (vanilla
/// `SignedMessageChain.DecodeException`: the chain is broken after these, and
/// every signed message is refused until a chat session with another key).
pub const CHAIN_BROKEN_KEYS: &[&str] = &[
    "chat.disabled.chain_broken",
    "chat.disabled.invalid_signature",
    "chat.disabled.out_of_order_chat",
];

/// The server said this connection's signed chat chain is broken.
#[derive(Clone, Debug, Message)]
pub struct ChatChainBrokenEvent {
    pub entity: Entity,
    /// The translation key of the server's message.
    pub key: String,
}

/// The key whose chat session the server broke on this connection: the
/// server ignores a session update with the same key (vanilla
/// `handleChatSessionUpdate`), so it is never announced again here; the next
/// session uses the next key (Mojang gives one at `refreshedAfter`), and a new
/// connection starts clean (removed on play `Login` and configuration).
#[derive(Clone, Component, Debug)]
pub struct BrokenChatKey {
    pub public_key_der: Vec<u8>,
}

/// A broken chain: drop the session (chat goes unsigned meanwhile, which the
/// server refuses as a message, never with a kick), remember its key, and
/// tell the bot (alert). A new session starts as soon as there's another key.
pub fn detect_broken_chain(
    mut commands: Commands,
    mut chat: MessageReader<client_chat::ChatReceivedEvent>,
    sessions: Query<&ChatSigningSession>,
    mut broken: MessageWriter<ChatChainBrokenEvent>,
) {
    for e in chat.read() {
        let client_chat::ChatPacket::System(p) = &e.packet else {
            continue;
        };
        let azalea_chat::FormattedText::Translatable(t) = &p.content else {
            continue;
        };
        if !CHAIN_BROKEN_KEYS.contains(&t.key.as_str()) {
            continue;
        }
        error!(
            "the server broke this connection's signed chat chain ({}): new chat session at the next key",
            t.key
        );
        if let Ok(s) = sessions.get(e.entity) {
            commands
                .entity(e.entity)
                .remove::<ChatSigningSession>()
                .insert(BrokenChatKey {
                    public_key_der: s.public_key_der.clone(),
                });
        }
        broken.write(ChatChainBrokenEvent {
            entity: e.entity,
            key: t.key.clone(),
        });
    }
}

/// The least time between two certificate fetches of one client.
pub const REFETCH_SPACING: Duration = Duration::from_secs(60);

/// When the next fetch may go: at the certificates' `refreshedAfter` (Mojang
/// answers with the same key before it), at least [`REFETCH_SPACING`] away.
fn refetch_at(
    refresh_after: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
    now_i: Instant,
) -> Instant {
    let until = (refresh_after - now).to_std().unwrap_or(Duration::ZERO);
    now_i + until.max(REFETCH_SPACING)
}

/// What the client does about its chat signing key now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CertAction {
    /// Send a chat session with the account's key.
    pub announce: bool,
    /// Fetch new certificates.
    pub fetch: bool,
}

/// The account's key as the decision sees it.
#[derive(Clone, Copy, Debug)]
pub struct KeyView<'a> {
    pub public_key_der: &'a [u8],
    pub expires_at: chrono::DateTime<Utc>,
    pub refresh_after: chrono::DateTime<Utc>,
}

/// The decision of [`request_certs_if_needed`]:
/// - announce a key that hasn't expired when the connection has no chat session
///   or the session has another key; never the session's own key again (the
///   server ignores a session update with an unchanged key and the client's new
///   session id would break every signed message after it);
/// - fetch (only when the account doesn't renew its own keys) when there is no
///   key, at `refreshedAfter` (vanilla's `dueRefresh`), or within `margin` of
///   expiry, unless a fetch is on hold (`hold`: before the last fetch's
///   `refreshedAfter`, or after a failure).
///
/// `broken`: the key of a session the server broke on this connection (never
/// announced again; a fetch for another key is due at once).
pub fn cert_action(
    owner: bool,
    key: Option<KeyView<'_>>,
    session_key: Option<&[u8]>,
    broken: Option<&[u8]>,
    margin: Duration,
    now: chrono::DateTime<Utc>,
    hold: bool,
) -> CertAction {
    let margin = chrono::Duration::from_std(margin).unwrap_or(chrono::Duration::MAX);
    let announce = key.is_some_and(|k| {
        now < k.expires_at
            && session_key != Some(k.public_key_der)
            && broken != Some(k.public_key_der)
    });
    let due = key.is_none_or(|k| {
        broken == Some(k.public_key_der)
            || now >= k.refresh_after
            || k.expires_at
                .checked_sub_signed(margin)
                .is_none_or(|t| now >= t)
    });
    CertAction {
        announce,
        fetch: !owner && due && !hold,
    }
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
            Option<&BrokenChatKey>,
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
    for (entity, account, only_refresh_certs_after, chat_signing_session, margin, broken) in
        query.iter_mut()
    {
        let margin = margin.copied().unwrap_or_default().0;
        let certs = account.certs();
        let hold = only_refresh_certs_after.is_some_and(|o| o.refresh_at > Instant::now());
        let action = cert_action(
            account.renews_certificates(),
            certs.as_ref().map(|c| KeyView {
                public_key_der: &c.public_key_der,
                expires_at: c.expires_at,
                refresh_after: c.refresh_after,
            }),
            chat_signing_session.map(|s| s.public_key_der.as_slice()),
            broken.map(|b| b.public_key_der.as_slice()),
            margin,
            Utc::now(),
            hold,
        );
        if action.announce
            && let Some(certs) = certs
        {
            // a new chat session with the account's key (renewed elsewhere,
            // or the connection's first), as vanilla's setKeyPair
            commands
                .entity(entity)
                .insert(QueuedCertsToSend { certs })
                .remove::<BrokenChatKey>();
        }
        if !action.fetch {
            continue;
        }

        debug!("Started task to fetch certs");
        let cap = crate::task_pool::owner::CERTS_CAP;
        let account = account.clone();
        let task = crate::task_pool::owner::spawn_io(async_compat::Compat::new(async move {
            tokio::time::timeout(cap, account.fetch_certificates())
                .await
                .unwrap_or(Err(azalea_auth::certs::FetchCertificatesError::TimedOut(
                    cap,
                )))
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
        (
            With<IsAuthenticated>,
            With<crate::secure_chat::SecureChatState>,
        ),
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

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn t(s: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000 + s, 0).unwrap()
    }
    const H: i64 = 3600;

    fn key(der: &[u8], expires: i64, refresh: i64) -> KeyView<'_> {
        KeyView {
            public_key_der: der,
            expires_at: t(expires),
            refresh_after: t(refresh),
        }
    }

    #[test]
    fn never_announces_the_sessions_own_key_again() {
        let six = Duration::from_secs(6 * 3600);
        let k = key(b"A", 48 * H, 40 * H);
        // first session of the connection
        assert!(cert_action(false, Some(k), None, None, six, t(0), false).announce);
        // the same key (Mojang before refreshedAfter): not again
        assert!(!cert_action(false, Some(k), Some(b"A"), None, six, t(0), false).announce);
        // a new key: a new session
        assert!(cert_action(false, Some(k), Some(b"B"), None, six, t(0), false).announce);
        // never an expired one
        assert!(!cert_action(false, Some(k), Some(b"B"), None, six, t(48 * H), false).announce);
    }

    #[test]
    fn fetch_at_refreshed_after_or_margin_and_never_for_an_owner() {
        let six = Duration::from_secs(6 * 3600);
        let k = key(b"A", 48 * H, 40 * H);
        assert!(!cert_action(false, Some(k), Some(b"A"), None, six, t(39 * H), false).fetch);
        assert!(
            cert_action(false, Some(k), Some(b"A"), None, six, t(40 * H), false).fetch,
            "refreshedAfter"
        );
        // short-lived keys: within the margin from the start, but a fetch on
        // hold (before the last fetch's refreshedAfter) doesn't loop
        let short = key(b"A", 2 * H, H);
        assert!(cert_action(false, Some(short), Some(b"A"), None, six, t(0), false).fetch);
        assert!(!cert_action(false, Some(short), Some(b"A"), None, six, t(0), true).fetch);
        // no key: fetch, unless the account renews its own
        assert!(cert_action(false, None, None, None, six, t(0), false).fetch);
        assert!(!cert_action(true, None, None, None, six, t(0), false).fetch);
        assert!(!cert_action(true, Some(short), Some(b"A"), None, six, t(2 * H), false).fetch);
        assert!(
            cert_action(
                true,
                Some(key(b"N", 48 * H, 40 * H)),
                Some(b"A"),
                None,
                six,
                t(0),
                false
            )
            .announce
        );
    }

    #[test]
    fn a_broken_key_is_never_announced_again() {
        let six = Duration::from_secs(6 * 3600);
        let k = key(b"A", 48 * H, 40 * H);
        // session dropped after chain_broken: same key not announced, a
        // fetch for another key at once (unless on hold)
        let a = cert_action(false, Some(k), None, Some(b"A"), six, t(0), false);
        assert_eq!(
            a,
            CertAction {
                announce: false,
                fetch: true
            }
        );
        let a = cert_action(false, Some(k), None, Some(b"A"), six, t(0), true);
        assert_eq!(
            a,
            CertAction {
                announce: false,
                fetch: false
            }
        );
        // the owner renews; once its key differs, a new session
        let a = cert_action(true, Some(k), None, Some(b"A"), six, t(0), false);
        assert_eq!(
            a,
            CertAction {
                announce: false,
                fetch: false
            }
        );
        let n = key(b"N", 50 * H, 42 * H);
        assert!(cert_action(true, Some(n), None, Some(b"A"), six, t(0), false).announce);
    }

    #[test]
    fn next_fetch_waits_for_refreshed_after() {
        let now_i = Instant::now();
        assert_eq!(
            refetch_at(t(H), t(0), now_i),
            now_i + Duration::from_secs(3600)
        );
        assert_eq!(
            refetch_at(t(-5), t(0), now_i),
            now_i + REFETCH_SPACING,
            "spaced even when already due"
        );
    }
}
