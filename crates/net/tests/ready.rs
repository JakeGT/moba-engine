//! Ready-up gating and surrender voting over real UDP.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moba_net::protocol::{MatchPhase, WireCommand};
use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

fn spawn_server(require_ready: bool) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let mut server = NetServer::bind(ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        sim: SimConfig::default(),
        snapshot_interval: 1,
        max_clients: 10,
        countdown_ticks: 30,
        results_ticks: 30,
        client_timeout: Duration::from_secs(3),
        reconnect_grace: Duration::from_secs(2),
        pre_shared_key: None,
        record: None,
        max_spectators: 4,
        max_packets_per_second: 120,
        max_total_packets_per_second: 2_000,
        require_ready,
    })
    .expect("bind");
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server.run_for(Duration::from_secs(5)).expect("server");
    });
    (addr, handle)
}

fn pump(client: &mut NetClient, seconds: f32) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs_f32(seconds) {
        client.poll().expect("poll");
        client.update().expect("update");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn ready_gate_and_surrender_vote() {
    let (addr, server) = spawn_server(true);
    let mut client = NetClient::connect(addr, Duration::from_secs(2)).expect("connect");
    pump(&mut client, 0.2);
    let state = client.snapshot().expect("lobby snapshot");
    assert_eq!(state.phase, MatchPhase::Lobby as u8);
    assert_eq!(state.player_mask, 1);
    assert_eq!(state.ready_mask, 0);

    client
        .send_command(WireCommand::Ready { ready: true })
        .expect("ready");
    let start = Instant::now();
    let mut playing = false;
    while start.elapsed() < Duration::from_secs(3) {
        pump(&mut client, 0.05);
        if let Some(state) = client.snapshot()
            && state.phase == MatchPhase::Playing as u8
        {
            playing = true;
            assert_eq!(state.ready_mask & 1, 1);
            break;
        }
    }
    assert!(playing, "ready starts countdown and play");

    client
        .send_command(WireCommand::Surrender)
        .expect("surrender");
    let start = Instant::now();
    let mut outcome = 0;
    while start.elapsed() < Duration::from_secs(2) {
        pump(&mut client, 0.05);
        if let Some(state) = client.snapshot()
            && state.outcome != 0
        {
            outcome = state.outcome;
            break;
        }
    }
    assert_eq!(outcome, 2, "blue surrender gives red the win");
    let _ = client.disconnect();
    server.join().expect("server thread");
}
