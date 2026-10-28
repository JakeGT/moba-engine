//! A human purchase is queued through `TickInputs` and appears in the replay.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moba_net::protocol::WireCommand;
use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::{Replay, SimConfig};

fn replay_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "moba-purchase-{}-{}.mbr",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ))
}

fn spawn_server(path: &std::path::Path) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().unwrap())
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(0)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(Some(path.to_path_buf()))
        .with_max_spectators(4)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let server = NetServer::bind(config).expect("bind");
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        let mut server = server;
        server.run_for(Duration::from_secs(7)).expect("server");
    });
    (addr, handle)
}

#[test]
fn human_purchase_is_recorded_and_replay_verifies() {
    let path = replay_path();
    let (addr, server) = spawn_server(&path);
    let mut client = NetClient::connect(addr, Duration::from_secs(2)).expect("connect");
    // Spawn is inside the blue shop radius; wait for the first snapshot so the server has
    // accepted the session, then buy and end the match.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) && client.snapshot().is_none() {
        client.poll().expect("poll");
        client.update().expect("update");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        client.snapshot().is_some(),
        "joined and received a snapshot"
    );
    client
        .send_command(WireCommand::BuyItem { item: 0 })
        .expect("buy");
    client
        .send_command(WireCommand::Surrender)
        .expect("surrender");

    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.exists() && Instant::now() < deadline {
        client.poll().expect("poll");
        client.update().expect("update");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(path.exists(), "server flushed a replay after surrender");
    let data = std::fs::read(&path).expect("read replay");
    let replay = Replay::decode(&data).expect("decode replay");
    assert!(
        replay
            .frames
            .iter()
            .any(|frame| frame.purchases[0].is_some()),
        "purchase was recorded in TickInputs"
    );
    replay.play().expect("replay verifies");
    let _ = client.disconnect();
    server.join().expect("server thread");
    let _ = std::fs::remove_file(path);
}
