//! Network soak tool: ten clients through lossy 150 ms links against a local server.
//!
//! Defaults to five minutes (the M5 exit requirement); use `--seconds` for a quick run.
//! Exits non-zero when a client disconnects or exceeds the bandwidth budget.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use moba_core::FixVec2;
use moba_net::client::ClientState;
use moba_net::protocol::{WireCommand, quantize_position};
use moba_net::{NetClient, NetServer, ServerConfig};
use moba_netsim::{LinkConfig, LinkProxy};
use moba_sim::SimConfig;

struct Options {
    seconds: u64,
    clients: usize,
    rtt_ms: u64,
    jitter_ms: u64,
    loss_percent: u8,
}

#[allow(clippy::too_many_lines)] // CLI orchestration is linear and auditable.
fn main() {
    let mut options = Options {
        seconds: 300,
        clients: 10,
        rtt_ms: 150,
        jitter_ms: 30,
        loss_percent: 5,
    };
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--seconds" => options.seconds = parse(&mut args, options.seconds),
            "--clients" => options.clients = parse(&mut args, options.clients),
            "--rtt" => options.rtt_ms = parse(&mut args, options.rtt_ms),
            "--jitter" => options.jitter_ms = parse(&mut args, options.jitter_ms),
            "--loss" => options.loss_percent = parse(&mut args, options.loss_percent),
            "--help" => {
                eprintln!(
                    "usage: soak [--seconds N] [--clients N] [--rtt MS] [--jitter MS] [--loss PCT]"
                );
                return;
            }
            other => eprintln!("unknown argument: {other}"),
        }
    }

    eprintln!(
        "soak: {} clients for {}s, {}ms RTT, {}ms jitter, {}% loss",
        options.clients, options.seconds, options.rtt_ms, options.jitter_ms, options.loss_percent
    );

    let mut server = match NetServer::bind(ServerConfig {
        bind: "127.0.0.1:0".parse().expect("bind address"),
        sim: SimConfig::default(),
        snapshot_interval: 1,
        max_clients: options.clients.min(10),
        countdown_ticks: 0,
        results_ticks: 0,
        client_timeout: Duration::from_secs(5),
        reconnect_grace: Duration::from_secs(60),
    }) {
        Ok(server) => server,
        Err(error) => fail(&format!("cannot bind server: {error}")),
    };
    let server_addr = server.local_addr().expect("server address");
    let server_seconds = options.seconds + 4;
    let server_thread = std::thread::spawn(move || {
        server
            .run_for(Duration::from_secs(server_seconds))
            .expect("server loop");
        (server.stats(), server.simulation().state_hash())
    });

    let proxies: Vec<LinkProxy> = (0..options.clients)
        .map(|index| {
            LinkProxy::start(
                "127.0.0.1:0".parse().expect("bind address"),
                server_addr,
                LinkConfig {
                    latency: Duration::from_millis(options.rtt_ms / 2),
                    jitter: Duration::from_millis(options.jitter_ms),
                    loss_percent: options.loss_percent,
                    seed: 0x5EA5_0000 + index as u64,
                },
            )
            .expect("proxy starts")
        })
        .collect();

    let mut clients: Vec<NetClient> = proxies
        .iter()
        .map(
            |proxy| match NetClient::connect(proxy.local_addr(), Duration::from_secs(5)) {
                Ok(client) => client,
                Err(error) => fail(&format!("connect failed: {error}")),
            },
        )
        .collect();
    eprintln!(
        "connected {} clients (slots {:?})",
        clients.len(),
        clients
            .iter()
            .map(NetClient::player_slot)
            .collect::<Vec<_>>()
    );

    let baseline: Vec<(u64, u64)> = clients
        .iter()
        .map(|client| {
            let stats = client.stats();
            (stats.bytes_received, stats.bytes_sent)
        })
        .collect();
    let mut disconnected = false;
    let mut last_command = Instant::now();
    let measure_start = Instant::now();
    let mut last_report = Instant::now();

    while measure_start.elapsed() < Duration::from_secs(options.seconds) {
        for (index, client) in clients.iter_mut().enumerate() {
            client.poll().expect("poll");
            client.update().expect("update");
            while client.next_event().is_some() {}
            if client.state() != ClientState::Connected {
                disconnected = true;
            }
            if last_command.elapsed() >= Duration::from_millis(500) {
                let target = FixVec2::from_ints(80 + i32::try_from(index).unwrap_or(0) * 12, 128);
                let (x, y) = quantize_position(target);
                let _ = client.send_command(WireCommand::MoveTo { x, y });
            }
        }
        if last_command.elapsed() >= Duration::from_millis(500) {
            last_command = Instant::now();
        }
        if last_report.elapsed() >= Duration::from_secs(15) {
            last_report = Instant::now();
            eprintln!(
                "  {}s connected={} clients={}",
                measure_start.elapsed().as_secs(),
                !disconnected,
                clients.len()
            );
        }
        std::thread::sleep(Duration::from_millis(8));
    }

    let seconds = measure_start.elapsed().as_secs_f64().max(0.001);
    let mut failed = disconnected;
    for (index, client) in clients.iter_mut().enumerate() {
        let stats = client.stats();
        let downstream = stats.bytes_received.saturating_sub(baseline[index].0) as f64 / seconds;
        let upstream = stats.bytes_sent.saturating_sub(baseline[index].1) as f64 / seconds;
        eprintln!(
            "client {index}: slot={} state={:?} snapshots={} down={:.1} KiB/s up={:.2} KiB/s rtt={:?}",
            client.player_slot(),
            stats.state,
            stats.snapshots,
            downstream / 1024.0,
            upstream / 1024.0,
            stats.rtt_ms
        );
        if stats.state != Some(ClientState::Connected) {
            failed = true;
        }
        if downstream > 40_000.0 || upstream > 4_000.0 {
            eprintln!("client {index} exceeded the bandwidth budget");
            failed = true;
        }
        let _ = client.disconnect();
    }
    let (server_stats, hash) = server_thread.join().expect("server thread");
    eprintln!(
        "server: ticks={} snapshots={} sent={} KiB recv={} KiB hash={:#x}",
        server_stats.ticks,
        server_stats.snapshots_sent,
        server_stats.bytes_sent / 1024,
        server_stats.bytes_received / 1024,
        hash
    );
    if failed {
        std::process::exit(1);
    }
}

fn parse<T: std::str::FromStr>(args: &mut impl Iterator<Item = String>, default: T) -> T {
    args.next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn fail(message: &str) -> ! {
    eprintln!("soak failed: {message}");
    std::process::exit(1);
}
