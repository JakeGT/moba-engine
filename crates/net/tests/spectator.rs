//! Read-only spectator connections receive full-map snapshots.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

fn spawn_server(seconds: u64) -> (SocketAddr, std::thread::JoinHandle<()>) {
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
    })
    .expect("server binds");
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server
            .run_for(Duration::from_secs(seconds))
            .expect("server");
    });
    (addr, handle)
}

#[test]
fn spectator_sees_both_teams_without_a_slot() {
    let (addr, server) = spawn_server(3);
    let mut spectator =
        NetClient::connect_spectator(addr, Duration::from_secs(2)).expect("spectator connect");
    assert!(spectator.is_spectator());
    assert_eq!(spectator.player_slot(), 0xFF);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        spectator.poll().expect("poll");
        spectator.update().expect("update");
        if spectator.snapshot().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let snapshot = spectator.snapshot().expect("spectator snapshot");
    let blue_heroes = snapshot
        .entities
        .iter()
        .filter(|entity| entity.kind == 0 && entity.team == 0)
        .count();
    let red_heroes = snapshot
        .entities
        .iter()
        .filter(|entity| entity.kind == 0 && entity.team == 1)
        .count();
    assert_eq!(blue_heroes, 5);
    assert_eq!(red_heroes, 5, "spectator is not fog-filtered");
    assert!(snapshot.entities.len() >= 26, "full map, both teams");
    let _ = spectator.disconnect();
    server.join().expect("server thread");
}
