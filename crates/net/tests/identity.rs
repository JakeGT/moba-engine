//! Pinned Ed25519 server identity authenticates the Welcome handshake.

use std::net::SocketAddr;
use std::time::Duration;

use moba_net::crypto::identity_public_key;
use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

const SEED: [u8; 32] = [0x5Au8; 32];

fn spawn_server() -> (SocketAddr, [u8; 32], std::thread::JoinHandle<()>) {
    let mut server = NetServer::bind(ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        sim: SimConfig::default(),
        snapshot_interval: 1,
        max_clients: 10,
        countdown_ticks: 0,
        results_ticks: 0,
        client_timeout: Duration::from_secs(2),
        reconnect_grace: Duration::from_secs(2),
        pre_shared_key: None,
        record: None,
        max_spectators: 4,
        max_packets_per_second: 120,
        require_ready: false,
    })
    .expect("bind");
    let public = server.set_server_identity(SEED);
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server.run_for(Duration::from_secs(4)).expect("server");
    });
    (addr, public, handle)
}

#[test]
fn pinned_identity_accepts_correct_key_and_rejects_wrong_key() {
    let (addr, public, server) = spawn_server();
    assert_eq!(public, identity_public_key(&SEED));

    let mut pinned = NetClient::connect_pinned(addr, Duration::from_secs(2), public)
        .expect("pinned server authenticates");
    assert_eq!(pinned.server_public_key(), Some(public));
    let _ = pinned.disconnect();

    let mut unpinned =
        NetClient::connect(addr, Duration::from_secs(2)).expect("unpinned connection works");
    let _ = unpinned.disconnect();

    let wrong = NetClient::connect_pinned(addr, Duration::from_millis(500), [0x11; 32]);
    assert!(wrong.is_err(), "wrong pinned key is rejected");
    server.join().expect("server thread");
}
