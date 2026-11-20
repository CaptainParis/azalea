//! A player chat or chat delete naming a signature that isn't cached: vanilla
//! 1.21.4 disconnects (`multiplayer.disconnect.invalid_packet`), so the
//! client's cache and window never part from the server's. Azalea closes the
//! connection the same way (the supervisor reconnects); a chat whose
//! references are all cached is tracked as before.

use azalea_client::{
    packet::game::UNCACHED_SIGNATURE_CLOSE, secure_chat::SecureChatState, test_utils::prelude::*,
};
use azalea_crypto::signing::MessageSignature;
use azalea_protocol::packets::{
    ConnectionProtocol,
    game::{
        ClientboundDeleteChat, ClientboundPlayerChat,
        c_player_chat::{
            ChatTypeBound, FilterMask, PackedLastSeenMessages, PackedMessageSignature,
            PackedSignedMessageBody,
        },
    },
};
use azalea_registry::{DataRegistry, Holder, data::ChatKind};

fn sig(n: u8) -> MessageSignature {
    MessageSignature { bytes: [n; 256] }
}

fn chat(signature: u8, last_seen: Vec<PackedMessageSignature>) -> ClientboundPlayerChat {
    ClientboundPlayerChat {
        sender: uuid::Uuid::from_u128(7),
        index: 0,
        signature: Some(sig(signature)),
        body: PackedSignedMessageBody {
            content: "hi".into(),
            timestamp: 0,
            salt: 0,
            last_seen: PackedLastSeenMessages { entries: last_seen },
        },
        unsigned_content: None,
        filter_mask: FilterMask::PassThrough,
        chat_type: ChatTypeBound {
            chat_type: Holder::Reference(ChatKind::new_raw(0)),
            name: "someone".into(),
            target_name: None,
        },
    }
}

fn tracked(s: &Simulation) -> u64 {
    s.component::<SecureChatState>().received_signed
}

#[test]
fn uncached_packed_signature_closes_like_vanilla() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    let disconnects = simulation.record_disconnects();

    // cached references: tracked, no close
    simulation.receive_packet(chat(1, vec![]));
    simulation.tick();
    simulation.receive_packet(chat(2, vec![PackedMessageSignature::Id(0)]));
    simulation.tick();
    assert_eq!(tracked(&simulation), 2);
    assert!(disconnects.lock().is_empty());

    // an id the cache doesn't have: closed
    simulation.receive_packet(chat(3, vec![PackedMessageSignature::Id(100)]));
    simulation.tick();
    simulation.tick();
    assert_eq!(tracked(&simulation), 2, "not tracked");
    let d = disconnects.lock().clone();
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].as_deref()
            .is_some_and(|r| r.contains("invalid_packet")),
        "{d:?}"
    );
    assert!(UNCACHED_SIGNATURE_CLOSE.starts_with("multiplayer.disconnect.invalid_packet"));
}

#[test]
fn uncached_delete_closes_like_vanilla() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    let disconnects = simulation.record_disconnects();
    simulation.receive_packet(chat(1, vec![]));
    simulation.tick();
    simulation.receive_packet(ClientboundDeleteChat {
        signature: PackedMessageSignature::Id(0),
    });
    simulation.tick();
    assert!(disconnects.lock().is_empty(), "a cached delete is fine");
    simulation.receive_packet(ClientboundDeleteChat {
        signature: PackedMessageSignature::Id(90),
    });
    simulation.tick();
    simulation.tick();
    assert_eq!(disconnects.lock().len(), 1);
}
