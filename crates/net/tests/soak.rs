//! Lossy 150 ms RTT soak: ten clients, five percent loss, 30 ms jitter, bandwidth check.
//!
//! Timing robustness: the clients handshake concurrently and every client is pumped from
//! the start, and the server runs until the test stops it rather than for a fixed time. A
//! slow or overloaded runner then makes the test slower, not flaky. The handshake's own
//! loss recovery is covered deterministically in `handshake_loss.rs`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use moba_core::FixVec2;
use moba_net::protocol::{WireCommand, quantize_position};
use moba_net::{ClientEvent, NetClient, NetServer, ServerConfig};
use moba_netsim::{LinkConfig, LinkProxy};
use moba_sim::SimConfig;

/// Generous on purpose: a 150 ms RTT handshake takes about 0.2 s; this bounds a stalled CI
/// runner, not the handshake.
const CONNECT_DEADLINE: Duration = Duration::from_secs(20);

#[test]
#[allow(clippy::too_many_lines)] // End-to-end soak setup and assertions.
fn ten_clients_over_a_lossy_150ms_link() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().unwrap())
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(0)
        .with_client_timeout(Duration::from_secs(5))
        .with_reconnect_grace(Duration::from_secs(60))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("server binds");
    let server_addr = server.local_addr().expect("addr");
    // Upper bound only: the test sets `shutdown` once it has measured everything.
    let shutdown = Arc::new(AtomicBool::new(false));
    let server_shutdown = Arc::clone(&shutdown);
    let server_thread = std::thread::spawn(move || {
        server
            .run_until(Duration::from_secs(60), &server_shutdown)
            .expect("server runs");
        (server.stats(), server.simulation().state_hash())
    });

    let proxies: Vec<LinkProxy> = (0..10)
        .map(|index| {
            LinkProxy::start(
                "127.0.0.1:0".parse().unwrap(),
                server_addr,
                LinkConfig {
                    seed: 0x1234 + index,
                    ..LinkConfig::default()
                },
            )
            .expect("proxy starts")
        })
        .collect();

    // All ten handshake at once, as players joining a lobby do, and every client is pumped
    // throughout, so none idles towards the server's client timeout while others connect.
    let mut clients: Vec<NetClient> = proxies
        .iter()
        .map(|proxy| NetClient::start(proxy.local_addr()).expect("client starts"))
        .collect();
    let connect_start = Instant::now();
    let connected = |clients: &[NetClient]| {
        clients
            .iter()
            .filter(|client| client.state() == moba_net::client::ClientState::Connected)
            .count()
    };
    while connected(&clients) < clients.len() && connect_start.elapsed() < CONNECT_DEADLINE {
        pump(&mut clients);
        std::thread::sleep(Duration::from_millis(2));
    }
    if connected(&clients) < clients.len() {
        for (index, (client, proxy)) in clients.iter().zip(&proxies).enumerate() {
            let link = proxy.stats();
            eprintln!(
                "client {index}: state={:?} link forwarded={} dropped={}",
                client.state(),
                link.forwarded(),
                link.dropped()
            );
        }
    }
    assert_eq!(
        connected(&clients),
        10,
        "not every client connected within {CONNECT_DEADLINE:?}"
    );
    eprintln!("all clients connected in {:?}", connect_start.elapsed());

    // Warm up snapshots/deltas before measuring bandwidth.
    let warm = Instant::now();
    while warm.elapsed() < Duration::from_millis(1_500) {
        pump(&mut clients);
        std::thread::sleep(Duration::from_millis(8));
    }
    let baseline: Vec<(u64, u64)> = clients
        .iter()
        .map(|client| {
            let stats = client.stats();
            (stats.bytes_received, stats.bytes_sent)
        })
        .collect();
    let measure_start = Instant::now();
    let mut last_command = Instant::now();
    while measure_start.elapsed() < Duration::from_secs(3) {
        pump(&mut clients);
        if last_command.elapsed() >= Duration::from_millis(500) {
            for (index, client) in clients.iter_mut().enumerate() {
                let target = FixVec2::from_ints(96 + i32::try_from(index).unwrap_or(0) * 8, 128);
                let (x, y) = quantize_position(target);
                let _ = client.send_command(WireCommand::MoveTo { x, y });
            }
            last_command = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    let measured_seconds = measure_start.elapsed().as_secs_f64();

    for (index, client) in clients.iter_mut().enumerate() {
        let stats = client.stats();
        let bytes = stats.bytes_received.saturating_sub(baseline[index].0);
        let sent = stats.bytes_sent.saturating_sub(baseline[index].1);
        let bytes_per_second = bytes as f64 / measured_seconds;
        let sent_per_second = sent as f64 / measured_seconds;
        eprintln!(
            "client {index}: slot={} snapshots={} down {:.1} KB/s up {:.2} KB/s rtt={:?}",
            client.player_slot(),
            stats.snapshots,
            bytes_per_second / 1024.0,
            sent_per_second / 1024.0,
            stats.rtt_ms
        );
        assert_eq!(stats.state, Some(moba_net::client::ClientState::Connected));
        assert!(stats.snapshots > 10, "client received too few snapshots");
        assert!(
            bytes_per_second < 40_000.0,
            "client {index} exceeded the 40 KB/s teamfight budget: {bytes_per_second:.0} B/s"
        );
        assert!(
            sent_per_second < 4_000.0,
            "client {index} exceeded the 4 KB/s upstream budget: {sent_per_second:.0} B/s"
        );
        let snapshot = client.snapshot().expect("snapshot");
        assert!(snapshot.entities.len() >= 16);
        while let Some(event) = client.next_event() {
            assert!(
                !matches!(event, ClientEvent::Disconnected),
                "client {index} disconnected"
            );
        }
    }

    for mut client in clients {
        let _ = client.disconnect();
    }
    shutdown.store(true, Ordering::Relaxed);
    let (stats, hash) = server_thread.join().expect("server thread");
    eprintln!(
        "server: clients={} ticks={} snapshots={} sent={} KiB recv={} KiB hash={:#x}",
        stats.clients,
        stats.ticks,
        stats.snapshots_sent,
        stats.bytes_sent / 1024,
        stats.bytes_received / 1024,
        hash
    );
    assert!(stats.snapshots_sent > 500);
    assert!(stats.bytes_sent > 0);
}

fn pump(clients: &mut [NetClient]) {
    for client in clients {
        client.poll().expect("poll");
        client.update().expect("update");
        while client.next_event().is_some() {}
    }
}
