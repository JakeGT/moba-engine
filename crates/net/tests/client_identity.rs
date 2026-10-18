//! Ed25519 client identity: server challenge, allowlist and rejection paths.

use std::net::SocketAddr;
use std::time::Duration;

use moba_net::client::ClientAuth;
use moba_net::crypto::identity_public_key;
use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

const SEED: [u8; 32] = [0x33; 32];

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
        max_total_packets_per_second: 2_000,
        require_ready: false,
    })
    .expect("bind");
    let public = identity_public_key(&SEED);
    server.require_client_identity([public]);
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server.run_for(Duration::from_secs(4)).expect("server");
    });
    (addr, public, handle)
}

#[test]
fn client_identity_challenge_accepts_allowlisted_key() {
    let (addr, public, server) = spawn_server();
    let auth = ClientAuth {
        identity_seed: Some(SEED),
        ..ClientAuth::default()
    };
    let mut client = NetClient::connect_with_auth(addr, Duration::from_secs(2), auth)
        .expect("identity accepted");
    assert_eq!(client.client_public_key(), Some(public));
    let _ = client.disconnect();

    let missing =
        NetClient::connect_with_auth(addr, Duration::from_millis(500), ClientAuth::default());
    assert!(missing.is_err(), "missing identity is rejected");

    let wrong = NetClient::connect_with_auth(
        addr,
        Duration::from_millis(500),
        ClientAuth {
            identity_seed: Some([0x44; 32]),
            ..ClientAuth::default()
        },
    );
    assert!(wrong.is_err(), "unlisted identity is rejected");
    server.join().expect("server thread");
}
