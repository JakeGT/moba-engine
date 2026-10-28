//! Pre-shared-key handshake and encrypted snapshot flow.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

const KEY: [u8; 32] = [0x42; 32];

fn spawn_server(seconds: u64) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().unwrap())
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(0)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(Some(KEY))
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("server binds");
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server
            .run_for(Duration::from_secs(seconds))
            .expect("server");
    });
    (addr, handle)
}

#[test]
fn encrypted_handshake_and_snapshots() {
    let (addr, server) = spawn_server(3);
    let mut client =
        NetClient::connect_with_key(addr, Duration::from_secs(2), KEY).expect("encrypted connect");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        client.poll().expect("poll");
        client.update().expect("update");
        if client.snapshot().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let snapshot = client.snapshot().expect("encrypted snapshot");
    assert_ne!(snapshot.entities.len(), 0);

    let mut spectator = NetClient::connect_spectator_with_key(addr, Duration::from_secs(2), KEY)
        .expect("encrypted spectator");
    assert!(spectator.is_spectator());
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) && spectator.snapshot().is_none() {
        spectator.poll().expect("spectator poll");
        spectator.update().expect("spectator update");
        std::thread::sleep(Duration::from_millis(5));
    }
    let full = spectator.snapshot().expect("spectator snapshot");
    assert!(
        full.entities
            .iter()
            .any(|entity| entity.kind == 0 && entity.team == 0)
    );
    assert!(
        full.entities
            .iter()
            .any(|entity| entity.kind == 0 && entity.team == 1)
    );

    let wrong = NetClient::connect_with_key(addr, Duration::from_millis(500), [0x24; 32]);
    assert!(wrong.is_err(), "wrong key must not connect");
    let _ = spectator.disconnect();
    let _ = client.disconnect();
    server.join().expect("server thread");
}
