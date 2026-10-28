//! Ed25519 client identity: server challenge, allowlist and rejection paths.

use std::net::SocketAddr;
use std::time::Duration;

use moba_net::client::ClientAuth;
use moba_net::crypto::identity_public_key;
use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

const SEED: [u8; 32] = [0x33; 32];

fn spawn_server() -> (SocketAddr, [u8; 32], std::thread::JoinHandle<()>) {
    let mut server = NetServer::bind(
        ServerConfig::default()
            .with_bind("127.0.0.1:0".parse().unwrap())
            .with_sim(SimConfig::default())
            .with_snapshot_interval(1)
            .with_max_clients(10)
            .with_countdown_ticks(0)
            .with_results_ticks(0)
            .with_client_timeout(Duration::from_secs(3))
            .with_reconnect_grace(Duration::from_secs(2))
            .with_pre_shared_key(None)
            .with_record(None)
            .with_max_spectators(4)
            .with_max_packets_per_second(120)
            .with_max_total_packets_per_second(2_000)
            .with_require_ready(false),
    )
    .expect("bind");
    let public = identity_public_key(&SEED);
    server.require_client_identity([public]);
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server.run_for(Duration::from_secs(4)).expect("server");
    });
    (addr, public, handle)
}

fn spawn_combined_server() -> (SocketAddr, [u8; 32], std::thread::JoinHandle<()>) {
    let token = [0x55; 32];
    let mut server = NetServer::bind(
        ServerConfig::default()
            .with_bind("127.0.0.1:0".parse().unwrap())
            .with_sim(SimConfig::default())
            .with_snapshot_interval(1)
            .with_max_clients(10)
            .with_countdown_ticks(0)
            .with_results_ticks(0)
            .with_client_timeout(Duration::from_secs(3))
            .with_reconnect_grace(Duration::from_secs(2))
            .with_pre_shared_key(None)
            .with_record(None)
            .with_max_spectators(4)
            .with_max_packets_per_second(120)
            .with_max_total_packets_per_second(2_000)
            .with_require_ready(false),
    )
    .expect("bind");
    server.add_client_key(token);
    server.require_client_identity([identity_public_key(&SEED)]);
    let server_public = server.set_server_identity([0x77; 32]);
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server.run_for(Duration::from_secs(4)).expect("server");
    });
    (addr, server_public, handle)
}

#[test]
fn identity_and_token_modes_compose() {
    let (addr, server_public, server) = spawn_combined_server();
    let token = [0x55; 32];
    let both = NetClient::connect_with_auth(
        addr,
        Duration::from_secs(2),
        ClientAuth::default()
            .with_pre_shared_key(Some(token))
            .with_expected_server(Some(server_public))
            .with_identity_seed(Some(SEED)),
    );
    assert!(both.is_ok(), "identity plus token connects");
    let identity_only = NetClient::connect_with_auth(
        addr,
        Duration::from_millis(500),
        ClientAuth::default().with_identity_seed(Some(SEED)),
    );
    assert!(identity_only.is_err(), "configured token is still required");
    let token_only = NetClient::connect_with_auth(
        addr,
        Duration::from_millis(500),
        ClientAuth::default().with_pre_shared_key(Some(token)),
    );
    assert!(token_only.is_err(), "required identity is still required");
    server.join().expect("server thread");
}

#[test]
fn client_identity_challenge_accepts_allowlisted_key() {
    let (addr, public, server) = spawn_server();
    let auth = ClientAuth::default().with_identity_seed(Some(SEED));
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
        ClientAuth::default().with_identity_seed(Some([0x44; 32])),
    );
    assert!(wrong.is_err(), "unlisted identity is rejected");
    server.join().expect("server thread");
}
