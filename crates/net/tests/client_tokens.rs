//! Per-client token key ring: each token authenticates and encrypts its own session.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

const TOKEN_A: [u8; 32] = [0x11; 32];
const TOKEN_B: [u8; 32] = [0x22; 32];

fn spawn_server() -> (SocketAddr, std::thread::JoinHandle<()>) {
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
    server.add_client_key(TOKEN_A);
    server.add_client_key(TOKEN_B);
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server.run_for(Duration::from_secs(4)).expect("server");
    });
    (addr, handle)
}

fn poll_snapshot(client: &mut NetClient) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        client.poll().expect("poll");
        client.update().expect("update");
        if client.snapshot().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no snapshot");
}

#[test]
fn each_client_token_decrypts_its_own_session() {
    let (addr, server) = spawn_server();
    let mut first =
        NetClient::connect_with_key(addr, Duration::from_secs(2), TOKEN_A).expect("token A");
    poll_snapshot(&mut first);
    let mut second =
        NetClient::connect_with_key(addr, Duration::from_secs(2), TOKEN_B).expect("token B");
    poll_snapshot(&mut second);
    assert!(first.snapshot().is_some());
    assert!(second.snapshot().is_some());

    let wrong = NetClient::connect_with_key(addr, Duration::from_millis(500), [0x33; 32]);
    assert!(wrong.is_err(), "unlisted token is rejected");
    let _ = first.disconnect();
    let _ = second.disconnect();
    server.join().expect("server thread");
}
