//! End-to-end UDP handshake, input and snapshot flow over loopback.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moba_core::FixVec2;
use moba_net::protocol::{MatchPhase, WireCommand, quantize_position};
use moba_net::{ClientEvent, NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

fn spawn_server(
    seconds: u64,
) -> (
    SocketAddr,
    std::thread::JoinHandle<(moba_net::ServerStats, u64)>,
) {
    let config = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        sim: SimConfig::default(),
        snapshot_interval: 1,
        max_clients: 10,
        countdown_ticks: 0,
        results_ticks: 0,
        client_timeout: Duration::from_millis(100),
        reconnect_grace: Duration::from_secs(2),
        pre_shared_key: None,
    };
    let mut server = NetServer::bind(config).expect("server binds");
    let addr = server.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || {
        server
            .run_for(Duration::from_secs(seconds))
            .expect("server runs");
        (server.stats(), server.simulation().state_hash())
    });
    (addr, handle)
}

#[test]
fn client_connects_receives_snapshots_and_moves() {
    let (addr, server) = spawn_server(2);
    let mut client = NetClient::connect(addr, Duration::from_secs(2)).expect("connect");
    assert_eq!(client.state(), moba_net::client::ClientState::Connected);
    assert!(client.player_slot() < 10);

    let mut saw_connected = false;
    let mut saw_snapshot = false;
    let start = Instant::now();
    let mut sent_move = false;
    let mut sent_buy = false;
    let mut start_position = None;
    while start.elapsed() < Duration::from_millis(1_500) {
        client.poll().expect("poll");
        client.update().expect("update");
        while let Some(event) = client.next_event() {
            match event {
                ClientEvent::Connected { .. } => saw_connected = true,
                ClientEvent::Snapshot { .. } => saw_snapshot = true,
                ClientEvent::Disconnected => panic!("unexpected disconnect"),
            }
        }
        if let Some(snapshot) = client.snapshot() {
            let own_hero = snapshot.entities.iter().find(|entity| {
                entity.hero_slot == client.player_slot()
                    && entity.team == client.team().index() as u8
            });
            if let Some(hero) = own_hero {
                let position = hero.position();
                if start_position.is_none() {
                    start_position = Some(position);
                }
                if !sent_buy {
                    client
                        .send_command(WireCommand::BuyItem { item: 0 })
                        .expect("shop input");
                    sent_buy = true;
                }
                if !sent_move {
                    let target = FixVec2::from_ints(120, 128);
                    let (x, y) = quantize_position(target);
                    client
                        .send_command(WireCommand::MoveTo { x, y })
                        .expect("input");
                    sent_move = true;
                } else if start_position
                    .is_some_and(|start| start.distance(position) > moba_core::Fix::from_i32(2))
                {
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(saw_connected, "connected event missing");
    assert!(saw_snapshot, "snapshot missing");
    let snapshot = client.snapshot().expect("snapshot state");
    assert_eq!(snapshot.phase, MatchPhase::Playing as u8);
    assert!(
        snapshot.entities.len() >= 16,
        "structures should be visible"
    );
    let own = snapshot
        .entities
        .iter()
        .find(|entity| {
            entity.hero_slot == client.player_slot() && entity.team == client.team().index() as u8
        })
        .expect("own hero");
    assert_eq!(own.progress.items[0], 0, "Ironblade should be in slot 0");
    assert!(own.progress.gold < 500, "purchase should spend gold");
    let stats = client.stats();
    assert!(stats.bytes_received > 0);
    assert_eq!(stats.state, Some(moba_net::client::ClientState::Connected));
    client.disconnect().expect("disconnect");
    let (server_stats, _) = server.join().expect("server thread");
    assert!(server_stats.clients >= 1 || server_stats.ticks > 0);
    assert!(server_stats.bytes_sent > 0);
}
