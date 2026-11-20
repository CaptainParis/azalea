//! Secure chat state (1.21.4): last-seen tracking, acknowledgements, the
//! signature cache for packed signatures, and the command tree that tells
//! which command arguments must be signed.
//!
//! Vanilla sources: `LastSeenMessagesTracker`, `MessageSignatureCache`,
//! `SignableCommand`, `ClientPacketListener` (`handlePlayerChat`,
//! `handleDeleteChat`, `markMessageAsProcessed`, `sendChatAcknowledgement`,
//! `sendChat`, `sendCommand`, `handleConfigurationStart`, `handleLogin`).
//!
//! The server keeps its own copy of the window (`LastSeenMessagesValidator`)
//! and kicks with `multiplayer.disconnect.chat_validation_failed` when an
//! update doesn't match it, and with `too_many_pending_chats` past 4096
//! unacknowledged messages, so this state must follow vanilla exactly.

use std::sync::Arc;

use azalea_core::bitset::FixedBitSet;
use azalea_crypto::signing::MessageSignature;
use azalea_protocol::packets::game::{
    ClientboundCommands,
    c_commands::{BrigadierNodeStub, BrigadierParser, BrigadierString, NodeType},
    c_player_chat::{PackedLastSeenMessages, PackedMessageSignature},
    s_chat::LastSeenMessagesUpdate,
};
use bevy_ecs::prelude::*;

/// Size of the last-seen window
/// (`LastSeenMessages.LAST_SEEN_MESSAGES_MAX_LENGTH`).
pub const LAST_SEEN_WINDOW: usize = 20;
/// Vanilla sends `ServerboundChatAck` once more than this many messages are
/// pending (`markMessageAsProcessed`).
pub const ACK_THRESHOLD: u32 = 64;
/// Entries of the packed-signature cache
/// (`MessageSignatureCache.createDefault`).
pub const SIGNATURE_CACHE_SIZE: usize = 128;

#[derive(Clone, Debug, PartialEq)]
struct Tracked {
    signature: MessageSignature,
    pending: bool,
}

/// `LastSeenMessagesTracker` with a window of 20.
#[derive(Clone, Debug, PartialEq)]
pub struct LastSeenTracker {
    entries: Vec<Option<Tracked>>,
    tail: usize,
    offset: u32,
    last_tracked: Option<MessageSignature>,
}

impl Default for LastSeenTracker {
    fn default() -> Self {
        Self {
            entries: vec![None; LAST_SEEN_WINDOW],
            tail: 0,
            offset: 0,
            last_tracked: None,
        }
    }
}

/// What a chat or signed command carries: the signatures it acknowledges
/// (signed into the message) and the update the server applies.
#[derive(Clone, Debug, PartialEq)]
pub struct LastSeenUpdate {
    pub last_seen: Vec<MessageSignature>,
    pub update: LastSeenMessagesUpdate,
}

impl LastSeenTracker {
    /// A player message arrived (`acknowledged`: it was shown). Returns false
    /// for a repeat of the last tracked signature, which vanilla ignores.
    pub fn add_pending(&mut self, signature: MessageSignature, acknowledged: bool) -> bool {
        if self.last_tracked.as_ref() == Some(&signature) {
            return false;
        }
        self.last_tracked = Some(signature.clone());
        let entry = acknowledged.then_some(Tracked {
            signature,
            pending: true,
        });
        let i = self.tail;
        // network-safe: constant non-zero divisor
        self.tail = (i + 1) % LAST_SEEN_WINDOW;
        self.offset = self.offset.saturating_add(1);
        // network-safe: tail is always below LAST_SEEN_WINDOW, the length
        self.entries[i] = entry;
        true
    }

    /// A deleted message is no longer acknowledged (if still pending).
    pub fn ignore_pending(&mut self, signature: &MessageSignature) {
        if let Some(e) = self.entries.iter_mut().find(|e| {
            e.as_ref()
                .is_some_and(|t| t.pending && t.signature == *signature)
        }) {
            *e = None;
        }
    }

    pub fn offset(&self) -> u32 {
        self.offset
    }

    pub fn get_and_clear_offset(&mut self) -> u32 {
        std::mem::take(&mut self.offset)
    }

    /// `generateAndApplyUpdate`: the offset since the last update and the
    /// window in order from the oldest entry; every entry becomes
    /// acknowledged.
    pub fn generate_and_apply_update(&mut self) -> LastSeenUpdate {
        let offset = self.get_and_clear_offset();
        let mut acknowledged = FixedBitSet::<LAST_SEEN_WINDOW>::new();
        let mut last_seen = Vec::with_capacity(LAST_SEEN_WINDOW);
        for j in 0..LAST_SEEN_WINDOW {
            // network-safe: constant non-zero divisor
            let k = (self.tail + j) % LAST_SEEN_WINDOW;
            // network-safe: k is below LAST_SEEN_WINDOW, the length
            if let Some(t) = &mut self.entries[k] {
                acknowledged.set(j);
                last_seen.push(t.signature.clone());
                t.pending = false;
            }
        }
        LastSeenUpdate {
            last_seen,
            update: LastSeenMessagesUpdate {
                offset,
                acknowledged,
            },
        }
    }

    /// Entries in the window (acknowledged or pending).
    pub fn tracked(&self) -> usize {
        self.entries.iter().filter(|e| e.is_some()).count()
    }
}

/// `MessageSignatureCache` (128 entries): resolves packed signatures (ids)
/// in received last-seen lists and deletes.
#[derive(Clone, Debug, PartialEq)]
pub struct SignatureCache {
    entries: Vec<Option<MessageSignature>>,
}

impl Default for SignatureCache {
    fn default() -> Self {
        Self {
            entries: vec![None; SIGNATURE_CACHE_SIZE],
        }
    }
}

impl SignatureCache {
    pub fn unpack(&self, p: &PackedMessageSignature) -> Option<MessageSignature> {
        match p {
            PackedMessageSignature::Signature(s) => Some((**s).clone()),
            PackedMessageSignature::Id(id) => {
                self.entries.get(*id as usize).and_then(|e| e.clone())
            }
        }
    }

    /// Unpacks every entry; `None` if one id isn't cached (vanilla
    /// disconnects with "invalid packet" then).
    pub fn unpack_all(&self, p: &PackedLastSeenMessages) -> Option<Vec<MessageSignature>> {
        p.entries.iter().map(|e| self.unpack(e)).collect()
    }

    /// `push(SignedMessageBody, signature)`: the body's last-seen entries and
    /// then the message's own signature go to the front, older entries move
    /// back, duplicates are not kept twice.
    pub fn push(&mut self, last_seen: &[MessageSignature], signature: Option<&MessageSignature>) {
        let mut queue: std::collections::VecDeque<MessageSignature> =
            last_seen.iter().cloned().collect();
        if let Some(s) = signature {
            queue.push_back(s.clone());
        }
        let set: Vec<MessageSignature> = queue.iter().cloned().collect();
        let mut i = 0;
        while i < self.entries.len() {
            let Some(next) = queue.pop_back() else {
                break;
            };
            // network-safe: the loop runs while i < len
            let old = self.entries[i].replace(next);
            if let Some(old) = old
                && !set.contains(&old)
            {
                queue.push_front(old);
            }
            i += 1;
        }
    }
}

/// A bot's own fault injection into its secure chat state (tests prove the
/// rig catches a broken state: a wrong offset, acknowledgements off).
#[derive(Clone, Component, Debug, Default, PartialEq, Eq)]
pub struct SecureChatMutation {
    /// Added to every offset the client reports (chat, signed commands, acks).
    pub offset_delta: i32,
    /// The client acknowledges nothing: no `ServerboundChatAck`, and chat
    /// updates carry offset 0 and an empty window.
    pub acks_disabled: bool,
}

/// Per-connection secure chat state, reset on every play `Login` (as
/// vanilla's `handleLogin`).
#[derive(Clone, Component, Debug, Default)]
pub struct SecureChatState {
    pub tracker: LastSeenTracker,
    pub cache: SignatureCache,
    /// Player chat messages with a signature received.
    pub received_signed: u64,
    /// `ServerboundChatAck` packets sent.
    pub acks_sent: u64,
    /// Signed chat messages and signed commands sent.
    pub signed_sent: u64,
    /// Commands sent unsigned because no argument needs a signature.
    pub unsigned_commands: u64,
}

impl SecureChatState {
    /// The offset to report, after the mutation.
    pub fn mutate_offset(offset: u32, m: Option<&SecureChatMutation>) -> u32 {
        match m {
            Some(m) if m.acks_disabled => 0,
            Some(m) => {
                (i64::from(offset) + i64::from(m.offset_delta)).clamp(0, i64::from(u32::MAX)) as u32
            }
            None => offset,
        }
    }

    /// The update for an outgoing chat or signed command.
    pub fn next_update(&mut self, m: Option<&SecureChatMutation>) -> LastSeenUpdate {
        if m.is_some_and(|m| m.acks_disabled) {
            return LastSeenUpdate {
                last_seen: Vec::new(),
                update: LastSeenMessagesUpdate::default(),
            };
        }
        let mut u = self.tracker.generate_and_apply_update();
        u.update.offset = Self::mutate_offset(u.update.offset, m);
        u
    }

    /// The offset of a `ServerboundChatAck` to send now, if any.
    pub fn take_ack(&mut self, m: Option<&SecureChatMutation>) -> Option<u32> {
        if m.is_some_and(|m| m.acks_disabled) {
            return None;
        }
        let o = self.tracker.get_and_clear_offset();
        (o > 0).then(|| {
            self.acks_sent += 1;
            Self::mutate_offset(o, m)
        })
    }
}

/// The last command tree the server sent (`ClientboundCommands`).
#[derive(Clone, Component, Debug)]
pub struct CommandTree(pub Arc<ClientboundCommands>);

/// One argument of a command that must be signed (a `minecraft:message`
/// argument, vanilla's only `SignedArgument`), with its raw text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignableArgument {
    pub name: String,
    pub value: String,
}

/// The arguments of `command` (without the leading `/`) that need a
/// signature, found by walking the server's command tree as brigadier would
/// parse it. `None`: the command doesn't parse against the tree.
///
/// Argument parsers are approximated by how much input they take (one word,
/// a quotable string, two or three coordinates, a bracketed selector or NBT,
/// the rest of the input); a command whose parse is ambiguous to this walk is
/// treated as unparsed and the caller sends it unsigned, which the server
/// answers by refusing the message, never by a kick.
pub fn signable_arguments(
    tree: &ClientboundCommands,
    command: &str,
) -> Option<Vec<SignableArgument>> {
    let root = tree.entries.get(tree.root_index as usize)?;
    let mut out = Vec::new();
    walk(tree, root, command, 0, &mut out, 0).then_some(out)
}

const MAX_DEPTH: usize = 64;

fn walk(
    tree: &ClientboundCommands,
    node: &BrigadierNodeStub,
    input: &str,
    pos: usize,
    out: &mut Vec<SignableArgument>,
    depth: usize,
) -> bool {
    if pos >= input.len() {
        return true;
    }
    if depth > MAX_DEPTH {
        return false;
    }
    // After a redirect, parsing continues at the target's children.
    let children: &[u32] = match node.redirect_node {
        Some(r) => match tree.entries.get(r as usize) {
            Some(t) => &t.children,
            None => return false,
        },
        None => &node.children,
    };
    // network-safe: our own command text; pos < len (checked above) and always
    // at a char boundary (after an ASCII space or at a length `consume` gave)
    let rest = &input[pos..];
    let word_end = rest.find(' ').unwrap_or(rest.len());
    // network-safe: word_end is a found index or the length
    let word = &rest[..word_end];
    // Brigadier: a literal that matches the next word hides the arguments.
    let literal = children.iter().find_map(|&c| {
        let n = tree.entries.get(c as usize)?;
        matches!(&n.node_type, NodeType::Literal { name } if name == word).then_some(n)
    });
    if let Some(n) = literal {
        return descend(tree, n, input, pos + word_end, out, depth);
    }
    for &c in children {
        let Some(n) = tree.entries.get(c as usize) else {
            continue;
        };
        let NodeType::Argument { name, parser, .. } = &n.node_type else {
            continue;
        };
        let Some(len) = consume(parser, rest) else {
            continue;
        };
        let mark = out.len();
        if matches!(parser, BrigadierParser::Message) {
            out.push(SignableArgument {
                name: name.clone(),
                // network-safe: len comes from `consume`, at most rest.len(),
                // at a char boundary
                value: rest[..len].to_owned(),
            });
        }
        if descend(tree, n, input, pos + len, out, depth) {
            return true;
        }
        out.truncate(mark);
    }
    false
}

fn descend(
    tree: &ClientboundCommands,
    n: &BrigadierNodeStub,
    input: &str,
    end: usize,
    out: &mut Vec<SignableArgument>,
    depth: usize,
) -> bool {
    if end >= input.len() {
        return true;
    }
    // network-safe: end < len checked above
    if input.as_bytes()[end] != b' ' {
        return false;
    }
    walk(tree, n, input, end + 1, out, depth + 1)
}

/// Bytes the parser takes from the start of `s` (never 0).
fn consume(parser: &BrigadierParser, s: &str) -> Option<usize> {
    use BrigadierParser as P;
    let words = |n: usize| -> Option<usize> {
        let mut end = 0;
        for i in 0..n {
            if i > 0 {
                if s.as_bytes().get(end) != Some(&b' ') {
                    return None;
                }
                end += 1;
            }
            // network-safe: end is 0 or just past an ASCII space, so <= len and
            // at a char boundary
            let w = s[end..].find(' ').unwrap_or(s.len() - end);
            if w == 0 {
                return None;
            }
            end += w;
        }
        Some(end)
    };
    let len = match parser {
        P::Message | P::String(BrigadierString::GreedyPhrase) => s.len(),
        P::String(BrigadierString::QuotablePhrase) => quoted(s).or_else(|| words(1))?,
        P::Vec3 | P::BlockPos => words(3)?,
        P::Vec2 | P::ColumnPos | P::Rotation => words(2)?,
        P::Entity(_) | P::GameProfile | P::ScoreHolder { .. } => bracketed(s)?,
        P::NbtCompoundTag
        | P::NbtTag
        | P::NbtPath
        | P::FormattedText
        | P::Style
        | P::ItemStack
        | P::ItemPredicate
        | P::BlockState
        | P::BlockPredicate
        | P::Particle => bracketed(s)?,
        _ => words(1)?,
    };
    (len > 0).then_some(len)
}

/// A `"..."` or `'...'` string with `\` escapes.
fn quoted(s: &str) -> Option<usize> {
    let q = *s.as_bytes().first()?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    let mut esc = false;
    for (i, &b) in s.as_bytes().iter().enumerate().skip(1) {
        if esc {
            esc = false;
        } else if b == b'\\' {
            esc = true;
        } else if b == q {
            return Some(i + 1);
        }
    }
    None
}

/// Up to the first space outside brackets, braces and quotes.
fn bracketed(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut esc = false;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if let Some(q) = quote {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == q {
                quote = None;
            }
            continue;
        }
        match b {
            b'"' | b'\'' => quote = Some(b),
            b'[' | b'{' | b'(' => depth += 1,
            b']' | b'}' | b')' => depth -= 1,
            b' ' if depth <= 0 => return (i > 0).then_some(i),
            _ => {}
        }
    }
    (!s.is_empty() && depth <= 0).then_some(s.len())
}

#[cfg(test)]
mod tests {
    use azalea_protocol::packets::game::c_commands::EntityParser;

    use super::*;

    fn sig(b: u8) -> MessageSignature {
        MessageSignature { bytes: [b; 256] }
    }

    #[test]
    fn tracker_follows_vanilla() {
        let mut t = LastSeenTracker::default();
        assert!(t.add_pending(sig(1), true));
        assert!(!t.add_pending(sig(1), true), "a repeat is ignored");
        assert!(t.add_pending(sig(2), true));
        assert!(t.add_pending(sig(3), false), "hidden: counted, not tracked");
        assert_eq!(t.offset(), 3);
        let u = t.generate_and_apply_update();
        assert_eq!(u.update.offset, 3);
        // window order from the oldest slot: tail is 3, slots 0..3 hold 1, 2,
        // hidden; j = (k - tail) mod 20 = 17, 18 for the two tracked ones
        assert_eq!(u.last_seen, vec![sig(1), sig(2)]);
        assert!(u.update.acknowledged.index(17));
        assert!(u.update.acknowledged.index(18));
        assert!(!u.update.acknowledged.index(19));
        assert_eq!(t.offset(), 0);
        // a second update re-sends the acknowledged window with offset 0
        let u2 = t.generate_and_apply_update();
        assert_eq!(u2.update.offset, 0);
        assert_eq!(u2.last_seen.len(), 2);
    }

    #[test]
    fn window_keeps_the_last_twenty() {
        let mut t = LastSeenTracker::default();
        for i in 0..45u8 {
            t.add_pending(sig(i), true);
        }
        let u = t.generate_and_apply_update();
        assert_eq!(u.update.offset, 45);
        assert_eq!(u.last_seen, (25..45).map(sig).collect::<Vec<_>>());
        for j in 0..20 {
            assert!(u.update.acknowledged.index(j));
        }
    }

    #[test]
    fn delete_ignores_only_pending() {
        let mut t = LastSeenTracker::default();
        t.add_pending(sig(1), true);
        t.add_pending(sig(2), true);
        t.ignore_pending(&sig(1));
        assert_eq!(t.tracked(), 1);
        let _ = t.generate_and_apply_update();
        t.ignore_pending(&sig(2));
        assert_eq!(t.tracked(), 1, "acknowledged entries stay");
    }

    /// Offsets reported over a run match what vanilla's server validator
    /// expects (a simulation of `LastSeenMessagesValidator`).
    #[test]
    fn validator_accepts_our_updates() {
        struct Validator {
            tracked: Vec<Option<(MessageSignature, bool)>>,
        }
        impl Validator {
            fn new() -> Self {
                Self {
                    tracked: vec![None; LAST_SEEN_WINDOW],
                }
            }
            fn add(&mut self, s: MessageSignature) {
                self.tracked.push(Some((s, true)));
            }
            fn apply_offset(&mut self, o: u32) -> bool {
                let max = self.tracked.len() - LAST_SEEN_WINDOW;
                if o as usize > max {
                    return false;
                }
                self.tracked.drain(0..o as usize);
                true
            }
            fn apply(&mut self, u: &LastSeenUpdate) -> Option<Vec<MessageSignature>> {
                if !self.apply_offset(u.update.offset) {
                    return None;
                }
                let mut out = Vec::new();
                for i in 0..LAST_SEEN_WINDOW {
                    let bit = u.update.acknowledged.index(i);
                    let e = self.tracked[i].clone();
                    if bit {
                        let (s, _) = e?;
                        self.tracked[i] = Some((s.clone(), false));
                        out.push(s);
                    } else {
                        if matches!(e, Some((_, false))) {
                            return None;
                        }
                        self.tracked[i] = None;
                    }
                }
                Some(out)
            }
        }
        let mut st = SecureChatState::default();
        let mut v = Validator::new();
        let mut n = 0u32;
        let mut acks = 0;
        for round in 0..300u32 {
            for _ in 0..(round % 7) {
                n += 1;
                let mut bytes = [0u8; 256];
                bytes[..4].copy_from_slice(&n.to_be_bytes());
                let s = MessageSignature { bytes };
                st.tracker.add_pending(s.clone(), true);
                v.add(s);
                if st.tracker.offset() > ACK_THRESHOLD {
                    let o = st.take_ack(None).unwrap();
                    assert!(v.apply_offset(o));
                    acks += 1;
                }
            }
            if round % 29 == 0 {
                let u = st.next_update(None);
                let seen = v.apply(&u).expect("validator accepts the update");
                assert_eq!(seen, u.last_seen, "same signatures on both sides");
            }
        }
        assert!(acks > 0, "acks were exercised");
        // a wrong offset is refused
        st.tracker.add_pending(sig(200), true);
        v.add(sig(200));
        let bad = SecureChatMutation {
            offset_delta: 1,
            acks_disabled: false,
        };
        let u = st.next_update(Some(&bad));
        assert!(v.apply(&u).is_none(), "offset + 1 must fail validation");
    }

    #[test]
    fn cache_push_and_unpack() {
        let mut c = SignatureCache::default();
        c.push(&[sig(1), sig(2)], Some(&sig(3)));
        assert_eq!(c.unpack(&PackedMessageSignature::Id(0)), Some(sig(3)));
        assert_eq!(c.unpack(&PackedMessageSignature::Id(1)), Some(sig(2)));
        assert_eq!(c.unpack(&PackedMessageSignature::Id(2)), Some(sig(1)));
        c.push(&[sig(2)], Some(&sig(4)));
        // 4, 2 at the front; 3 and 1 moved back, 2 not kept twice
        let got: Vec<_> = (0..4)
            .map(|i| c.unpack(&PackedMessageSignature::Id(i)))
            .collect();
        assert_eq!(
            got,
            vec![Some(sig(4)), Some(sig(2)), Some(sig(3)), Some(sig(1))]
        );
        assert_eq!(c.unpack(&PackedMessageSignature::Id(500)), None);
    }

    fn node(children: &[u32], redirect: Option<u32>, node_type: NodeType) -> BrigadierNodeStub {
        BrigadierNodeStub {
            is_executable: true,
            children: children.to_vec(),
            redirect_node: redirect,
            node_type,
        }
    }
    fn lit(name: &str, children: &[u32]) -> BrigadierNodeStub {
        node(children, None, NodeType::Literal { name: name.into() })
    }
    fn arg(name: &str, parser: BrigadierParser, children: &[u32]) -> BrigadierNodeStub {
        node(
            children,
            None,
            NodeType::Argument {
                name: name.into(),
                parser,
                suggestions_type: None,
            },
        )
    }

    /// A tree shaped like vanilla's: `msg <targets> <message>`, `me <action>`,
    /// `teleport <location>`, `execute as <targets> run -> root`.
    fn tree() -> ClientboundCommands {
        let ent = BrigadierParser::Entity(EntityParser {
            single: false,
            players_only: true,
        });
        let entries = vec![
            node(&[1, 4, 6, 8], None, NodeType::Root),     // 0
            lit("msg", &[2]),                              // 1
            arg("targets", ent.clone(), &[3]),             // 2
            arg("message", BrigadierParser::Message, &[]), // 3
            lit("me", &[5]),                               // 4
            arg("action", BrigadierParser::Message, &[]),  // 5
            lit("teleport", &[7]),                         // 6
            arg("location", BrigadierParser::Vec3, &[]),   // 7
            lit("execute", &[9]),                          // 8
            lit("as", &[10]),                              // 9
            arg("targets", ent, &[11]),                    // 10
            node(&[], Some(0), NodeType::Literal { name: "run".into() }), // 11
        ];
        ClientboundCommands {
            entries,
            root_index: 0,
        }
    }

    #[test]
    fn finds_message_arguments() {
        let t = tree();
        let a = |s: &str| signable_arguments(&t, s);
        assert_eq!(
            a("msg Bob hello there"),
            Some(vec![SignableArgument {
                name: "message".into(),
                value: "hello there".into()
            }])
        );
        assert_eq!(
            a("msg @a[name=\"x y\",limit=1] hi"),
            Some(vec![SignableArgument {
                name: "message".into(),
                value: "hi".into()
            }])
        );
        assert_eq!(
            a("me waves"),
            Some(vec![SignableArgument {
                name: "action".into(),
                value: "waves".into()
            }])
        );
        assert_eq!(a("teleport 1 2 3"), Some(vec![]));
        assert_eq!(
            a("execute as Bob run msg Al hi"),
            Some(vec![SignableArgument {
                name: "message".into(),
                value: "hi".into()
            }]),
            "redirects are followed"
        );
        assert_eq!(a("teleport 1 2"), None);
        assert_eq!(a("nosuch thing"), None);
    }
}
