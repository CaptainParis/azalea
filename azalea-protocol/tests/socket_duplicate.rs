//! The owned socket duplicate a client connection keeps closes the connection
//! for both halves from another thread (the WP2 watchdog's handle).

use std::{net::Shutdown, time::Duration};

use azalea_protocol::connect::Connection;
use tokio::{io::AsyncReadExt, net::TcpListener};

#[tokio::test]
async fn shutting_down_the_duplicate_closes_the_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (conn, accepted) = tokio::join!(Connection::new(&addr), listener.accept());
    let conn = conn.unwrap();
    let (mut server_side, _) = accepted.unwrap();
    let (read, _write) = conn.into_split_raw();
    let socket = read.socket.clone().expect("client connections keep a duplicate");

    // from another thread, as the watchdog does
    std::thread::spawn(move || socket.shutdown(Shutdown::Both).unwrap())
        .join()
        .unwrap();

    // the server sees the connection end (EOF) although both halves are alive
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(5), server_side.read(&mut buf))
        .await
        .expect("server saw the close in time")
        .unwrap();
    assert_eq!(n, 0);
    drop(read);
}
