//! A disconnected player keeps their slot for a grace period and can resume it.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

fn spawn_server(seconds: u64) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let config = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        sim: SimConfig::default(),
        snapshot_interval: 1,
        max_clients: 10,
        countdown_ticks: 0,
        results_ticks: 0,
        client_timeout: Duration::from_millis(100),
        reconnect_grace: Duration::from_secs(2),
    };
    let mut server = NetServer::bind(config).expect("server binds");
    let addr = server.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || {
        server
            .run_for(Duration::from_secs(seconds))
            .expect("server");
    });
    (addr, handle)
}

#[test]
fn reconnect_resumes_the_same_slot() {
    let (addr, server) = spawn_server(3);
    let mut first = NetClient::connect(addr, Duration::from_secs(2)).expect("first connect");
    let slot = first.player_slot();
    let session = first.session();
    for _ in 0..50 {
        first.poll().expect("poll");
        first.update().expect("update");
        if first.snapshot().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(first.snapshot().is_some());
    drop(first);

    // Wait past the short client timeout so the server reserves the slot for a reconnect.
    std::thread::sleep(Duration::from_millis(200));
    let mut second =
        NetClient::connect_with_session(addr, Duration::from_secs(2), session).expect("resume");
    assert_eq!(second.player_slot(), slot);
    assert_eq!(second.session(), session);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        second.poll().expect("poll");
        second.update().expect("update");
        if second.snapshot().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let snapshot = second.snapshot().expect("snapshot after reconnect");
    assert!(
        snapshot
            .entities
            .iter()
            .any(|entity| entity.hero_slot == slot)
    );
    let _ = second.disconnect();
    server.join().expect("server thread");
}
