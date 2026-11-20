//! Bad packet policy, frame errors: a frame length prefix wider than 3 bytes
//! means the byte stream is lost. The connection is closed at once (the
//! server sees it end), counted, alerted, and the client gets a
//! `DisconnectEvent` with the reason (the supervisor reconnects).

use std::time::{Duration, Instant};

use azalea_client::{
    bad_packet::{AlertReason, BadPacketAction, BadPacketAlert, BadPacketEvent},
    connection::{ConnectionSocket, RawConnection},
    test_utils::prelude::*,
};
use azalea_core::decode_errors::decode_errors;
use azalea_protocol::{connect::Connection, packets::ConnectionProtocol};
use azalea_world::WorldName;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[test]
fn frame_error_closes_the_connection() {
    let _lock = init_expecting_errors();
    let mut simulation = Simulation::new(ConnectionProtocol::Game);
    simulation.receive_packet(default_login_packet());
    simulation.tick();

    // a real socket under the simulated client
    let (mut server, raw_conn, socket) = simulation.rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let conn = Connection::new_from_stream(client)
            .await
            .unwrap()
            .login()
            .config()
            .game();
        let (read, write) = conn.into_split_raw();
        let socket = read.socket.clone().unwrap();
        (
            server,
            RawConnection::new(read, write, ConnectionProtocol::Game),
            socket,
        )
    });
    simulation
        .app
        .world_mut()
        .entity_mut(simulation.entity)
        .insert((raw_conn, ConnectionSocket(socket)));
    let events = simulation.record_messages::<BadPacketEvent>();
    let alerts = simulation.record_messages::<BadPacketAlert>();
    let disconnects = simulation.record_disconnects();
    let before = decode_errors();

    simulation.rt.block_on(async {
        server.write_all(&[0xff, 0xff, 0xff, 0x01]).await.unwrap();
        server.flush().await.unwrap();
    });
    let t0 = Instant::now();
    while decode_errors().frame_length == before.frame_length && t0.elapsed() < Duration::from_secs(2)
    {
        simulation.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    simulation.tick();
    simulation.tick();

    let after = decode_errors();
    assert_eq!(after.frame_length, before.frame_length + 1);
    assert_eq!(after.policy_close, before.policy_close + 1);
    {
        let events = events.lock();
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(matches!(events[0].action, BadPacketAction::Closed { .. }));
        assert!(events[0].raw.is_empty());
    }
    assert_eq!(alerts.lock()[0].reason, AlertReason::Closed);
    let disconnects = disconnects.lock();
    assert_eq!(disconnects.len(), 1, "{disconnects:?}");
    assert!(
        disconnects[0]
            .as_deref()
            .unwrap_or_default()
            .contains("frame error")
    );
    assert!(!simulation.has_component::<WorldName>());

    // the server sees the connection end at once
    let n = simulation.rt.block_on(async {
        let mut buf = [0u8; 64];
        tokio::time::timeout(Duration::from_secs(2), server.read(&mut buf))
            .await
            .expect("server saw the close within 2 s")
    });
    assert!(matches!(n, Ok(0) | Err(_)), "{n:?}");
}
