//! The server's `chat.disabled.chain_broken` / `invalid_signature` /
//! `out_of_order_chat` system message (vanilla: the chain is broken, every
//! signed message refused until a session with another key) is reported as a
//! `ChatChainBrokenEvent` (the bot alerts on it and the client starts a new
//! chat session at the next key); other system messages are not.

use azalea_chat::{FormattedText, translatable_component::TranslatableComponent};
use azalea_client::{chat_signing::ChatChainBrokenEvent, test_utils::prelude::*};
use azalea_protocol::packets::{ConnectionProtocol, game::ClientboundSystemChat};

fn system(key: &str) -> ClientboundSystemChat {
    ClientboundSystemChat {
        content: FormattedText::Translatable(TranslatableComponent::new(key.into(), vec![])),
        overlay: false,
    }
}

#[test]
fn broken_chain_messages_are_reported() {
    // the client logs the break as an error
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();
    let broken = simulation.record_messages::<ChatChainBrokenEvent>();
    simulation.receive_packet(system("chat.disabled.missingProfileKey"));
    simulation.receive_packet(ClientboundSystemChat {
        content: "chat.disabled.chain_broken".into(),
        overlay: false,
    });
    simulation.tick();
    assert!(
        broken.lock().is_empty(),
        "only the server's translatable keys"
    );
    for key in [
        "chat.disabled.chain_broken",
        "chat.disabled.invalid_signature",
        "chat.disabled.out_of_order_chat",
    ] {
        simulation.receive_packet(system(key));
        simulation.tick();
    }
    let got: Vec<String> = broken.lock().iter().map(|e| e.key.clone()).collect();
    assert_eq!(
        got,
        [
            "chat.disabled.chain_broken",
            "chat.disabled.invalid_signature",
            "chat.disabled.out_of_order_chat"
        ]
    );
}
