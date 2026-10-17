//! Headless authoritative dedicated server.
//!
//! Runs the deterministic simulation at 30 Hz with server-side bots and reports tick budget,
//! wave progress, peak unit count and the final state hash. Networking attaches in M5.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use moba_net::{NetServer, ServerConfig};
use moba_sim::{SimConfig, Simulation, UnitKind};

#[allow(clippy::too_many_lines)] // CLI orchestration is linear and auditable.
fn main() {
    let mut ticks = 18_000u32;
    let mut seed = SimConfig::default().seed;
    let mut listen: Option<String> = None;
    let mut seconds = 60u64;
    let mut countdown_secs = 3u64;
    let mut results_secs = 15u64;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--ticks" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    ticks = value;
                }
            }
            "--seed" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    seed = value;
                }
            }
            "--listen" => {
                listen = args.next();
            }
            "--seconds" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    seconds = value;
                }
            }
            "--countdown" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    countdown_secs = value;
                }
            }
            "--results" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    results_secs = value;
                }
            }
            "--help" => {
                eprintln!(
                    "usage: moba-server [--ticks N] [--seed N] \
                     [--listen ADDR --seconds N --countdown S --results S]"
                );
                return;
            }
            other => eprintln!("unknown argument: {other}"),
        }
    }

    let config = SimConfig {
        seed,
        bot_mask: 0x03FF,
        ..SimConfig::default()
    };

    if let Some(listen) = listen {
        let bind = listen.parse().unwrap_or_else(|error| {
            eprintln!("invalid --listen address {listen}: {error}");
            std::process::exit(2);
        });
        let mut server = NetServer::bind(ServerConfig {
            bind,
            sim: config,
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: u32::try_from(countdown_secs.saturating_mul(30)).unwrap_or(u32::MAX),
            results_ticks: u32::try_from(results_secs.saturating_mul(30)).unwrap_or(u32::MAX),
        })
        .unwrap_or_else(|error| {
            eprintln!("cannot bind {bind}: {error}");
            std::process::exit(2);
        });
        eprintln!(
            "network server listening on {} for {seconds}s (30 Hz, 10 bot slots, {countdown_secs}s countdown, {results_secs}s results)",
            server.local_addr().expect("bound address")
        );
        server
            .run_for(Duration::from_secs(seconds))
            .unwrap_or_else(|error| {
                eprintln!("network server error: {error}");
                std::process::exit(1);
            });
        let stats = server.stats();
        eprintln!(
            "network done: clients={} ticks={} snapshots={} sent={} KiB recv={} KiB",
            stats.clients,
            stats.ticks,
            stats.snapshots_sent,
            stats.bytes_sent / 1024,
            stats.bytes_received / 1024
        );
        return;
    }

    let mut simulation = Simulation::new(config);
    let target = Duration::from_secs_f64(1.0 / 30.0);
    let mut peak_units = simulation.unit_count();
    let mut total_ns = 0u128;
    let mut max_ns = 0u128;
    let mut measured = 0u32;
    let wall_start = Instant::now();

    eprintln!(
        "server: seed={seed:#x} target={:.2} ms/tick bots=10",
        target.as_secs_f64() * 1_000.0
    );

    for index in 0..ticks {
        let tick_start = Instant::now();
        simulation.tick_with_bots();
        let elapsed_ns = tick_start.elapsed().as_nanos();
        total_ns += elapsed_ns;
        max_ns = max_ns.max(elapsed_ns);
        measured += 1;
        peak_units = peak_units.max(simulation.unit_count());

        if (index + 1) % 300 == 0 {
            let average_us = total_ns as f64 / f64::from(measured) / 1_000.0;
            eprintln!(
                "tick {:>6} wave {:>3} units {:>3} avg {average_us:>7.1} us",
                simulation.current_tick().get(),
                simulation.wave(),
                simulation.unit_count()
            );
        }
        simulation.clear_events();

        if let Some(winner) = simulation.outcome() {
            eprintln!(
                "match over at tick {}: {winner:?} wins",
                simulation.current_tick().get()
            );
            break;
        }
    }

    let mut towers = 0u32;
    let mut nexuses = 0u32;
    let mut minions = 0u32;
    for entity in simulation.world().entities() {
        if let Some(kind) = simulation.world().get::<UnitKind>(entity) {
            match kind {
                UnitKind::Tower => towers += 1,
                UnitKind::Nexus => nexuses += 1,
                UnitKind::MeleeMinion | UnitKind::RangedMinion => minions += 1,
                UnitKind::Hero => {}
            }
        }
    }
    let average_us = total_ns as f64 / f64::from(measured.max(1)) / 1_000.0;
    eprintln!(
        "done: ticks={} wall={:.2}s avg={average_us:.1}us max={:.1}us peak_units={peak_units} towers={towers} nexuses={nexuses} minions={minions} hash={:#018x} outcome={:?}",
        simulation.current_tick().get(),
        wall_start.elapsed().as_secs_f64(),
        max_ns as f64 / 1_000.0,
        simulation.state_hash(),
        simulation.outcome()
    );
    if simulation.outcome().is_none() {
        eprintln!("note: match did not finish within the tick limit");
    }
}
