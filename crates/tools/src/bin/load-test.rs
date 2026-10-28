//! Multi-match load test: N concurrent matches × C clients against one in-process host.
//!
//! Every match runs in a [`MatchHost`] with its own UDP port; every client is a real
//! [`NetClient`] over loopback (optionally through a `netsim` link). Exits non-zero when a
//! budget is missed:
//! - per-match tick p99 above `--tick-budget-us` (default 2000 µs, the M7 target);
//! - more than 1 % of worker frames overrun the 30 Hz schedule;
//! - any client disconnects;
//! - a client averages more than 40 KB/s down or 4 KB/s up, or the mean downstream
//!   exceeds 20 KB/s (decimal units, including 28 B of IPv4 + UDP header per datagram);
//! - a client goes more than 1 s without an applied snapshot.
//!
//! Usage: `load-test [--matches N] [--clients C] [--seconds S] [--workers K]
//! [--client-threads T] [--input-hz N] [--rtt MS --jitter MS --loss PCT]
//! [--flood-port K --flood-pps N [--flood-sources S]] [--host-max-pps N]`. Clients send
//! `--input-hz` commands per second (default 10, a busy player's click rate).
//!
//! **Flood mode** (`--flood-port K --flood-pps N`): a separate thread sends N undecodable
//! datagrams per second to match K from S source ports (default 1), starting before the
//! clients connect so handshakes run under the flood. The tick budget then applies to the
//! p99 of *each* other match (the worst one is gated) and **no** worker frame may be dropped. The attacked match itself is
//! reported but not gated: its own receive work, handshakes and client results (a
//! multi-source flood can legitimately starve its handshakes through the per-match budget).
//! Client checks (connection, bandwidth, snapshot gaps) still gate every other match.
//!
//! Note: the clients run in the same process, so they compete with the host for CPU; the
//! result is therefore a conservative bound for a dedicated server box.

#![forbid(unsafe_code)]

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use moba_core::FixVec2;
use moba_net::client::ClientState;
use moba_net::protocol::{WireCommand, quantize_position};
use moba_net::{ClientStats, HostReport, MatchHost, NetClient, ServerConfig, TickHistogram};
use moba_netsim::{LinkConfig, LinkProxy};

struct Options {
    matches: usize,
    clients: usize,
    seconds: u64,
    workers: usize,
    client_threads: usize,
    rtt_ms: u64,
    jitter_ms: u64,
    loss_percent: u8,
    tick_budget_us: u64,
    input_hz: u32,
    flood_match: Option<usize>,
    flood_pps: u32,
    flood_sources: usize,
    host_max_pps: u32,
}

struct ClientResult {
    match_index: usize,
    connected: bool,
    down_bytes_per_sec: f64,
    up_bytes_per_sec: f64,
    max_snapshot_gap: Duration,
}

/// Junk flood statistics.
struct FloodReport {
    sent: u64,
    send_errors: u64,
    seconds: f64,
}

/// IPv4 (20 B) + UDP (8 B) header bytes per datagram.
const UDP_IPV4_OVERHEAD: u64 = 28;
/// Longest tolerated gap between applied snapshots (the stream is 30 Hz).
const MAX_SNAPSHOT_GAP: Duration = Duration::from_secs(1);

#[allow(clippy::too_many_lines)] // CLI orchestration and the report are one linear flow.
fn main() {
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let mut options = Options {
        matches: 50,
        clients: 10,
        seconds: 300,
        workers: (cores / 2).max(1),
        client_threads: (cores / 2).max(1),
        rtt_ms: 0,
        jitter_ms: 0,
        loss_percent: 0,
        tick_budget_us: 2_000,
        input_hz: 10,
        flood_match: None,
        flood_pps: 0,
        flood_sources: 1,
        host_max_pps: 0,
    };
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--matches" => options.matches = parse(&mut args, options.matches),
            "--clients" => options.clients = parse(&mut args, options.clients).min(10),
            "--seconds" => options.seconds = parse(&mut args, options.seconds),
            "--workers" => options.workers = parse(&mut args, options.workers),
            "--client-threads" => {
                options.client_threads = parse(&mut args, options.client_threads);
            }
            "--rtt" => options.rtt_ms = parse(&mut args, options.rtt_ms),
            "--jitter" => options.jitter_ms = parse(&mut args, options.jitter_ms),
            "--loss" => options.loss_percent = parse(&mut args, options.loss_percent),
            "--input-hz" => options.input_hz = parse(&mut args, options.input_hz).max(1),
            "--tick-budget-us" => options.tick_budget_us = parse(&mut args, options.tick_budget_us),
            "--flood-port" => options.flood_match = Some(parse(&mut args, 0)),
            "--flood-pps" => options.flood_pps = parse(&mut args, options.flood_pps),
            "--flood-sources" => {
                options.flood_sources = parse(&mut args, options.flood_sources).clamp(1, 1_024);
            }
            "--host-max-pps" => options.host_max_pps = parse(&mut args, options.host_max_pps),
            "--help" => {
                eprintln!(
                    "usage: load-test [--matches N] [--clients C] [--seconds S] [--workers K] [--client-threads T] [--rtt MS] [--jitter MS] [--loss PCT] [--input-hz N] [--tick-budget-us US] [--flood-port K --flood-pps N [--flood-sources S]] [--host-max-pps N]"
                );
                return;
            }
            other => fail(&format!("unknown argument: {other}")),
        }
    }
    if let Some(index) = options.flood_match {
        if index >= options.matches {
            fail(&format!(
                "--flood-port {index} is not a match index (0..{})",
                options.matches
            ));
        }
        if options.flood_pps == 0 {
            fail("--flood-port needs --flood-pps N");
        }
    }
    let flood_match = options.flood_match;
    let conditioned = options.rtt_ms > 0 || options.jitter_ms > 0 || options.loss_percent > 0;
    eprintln!(
        "load-test: {} matches x {} clients for {}s at {} commands/s, {} host workers, {} client threads, {} logical cores, link {}",
        options.matches,
        options.clients,
        options.seconds,
        options.input_hz,
        options.workers,
        options.client_threads,
        cores,
        if conditioned {
            format!(
                "{} ms RTT / {} ms jitter / {}% loss",
                options.rtt_ms, options.jitter_ms, options.loss_percent
            )
        } else {
            String::from("direct loopback")
        }
    );
    if let Some(index) = flood_match {
        eprintln!(
            "flood: {} junk datagrams/s to match {index} from {} source(s); host budget {}",
            options.flood_pps,
            options.flood_sources,
            if options.host_max_pps > 0 {
                format!("{} pps", options.host_max_pps)
            } else {
                String::from("off")
            }
        );
    }

    let template = ServerConfig {
        bind: "127.0.0.1:0".parse().expect("bind address"),
        countdown_ticks: 0,
        results_ticks: 0,
        max_clients: options.clients,
        // One host serves every client from loopback; per-address limits stay meaningful,
        // the global per-match budget must cover ten clients.
        max_total_packets_per_second: 4_000,
        ..ServerConfig::default()
    };
    let content = Arc::new(moba_game::GameContent::default());
    let mut host = MatchHost::bind(&template, &content, options.matches, options.workers)
        .unwrap_or_else(|error| fail(&format!("cannot bind matches: {error}")));
    host.set_host_packet_budget(options.host_max_pps);
    let addrs = host
        .local_addrs()
        .unwrap_or_else(|error| fail(&format!("cannot read match addresses: {error}")));
    let shutdown = Arc::new(AtomicBool::new(false));
    let host_shutdown = Arc::clone(&shutdown);
    // The host runs until the clients finish; the duration is only a safety net.
    let host_limit = Duration::from_secs(options.seconds + 120);
    let host_thread = std::thread::spawn(move || host.run_until(host_limit, &host_shutdown));

    // Optional junk flood against one match, started before any client connects.
    let flood_thread = flood_match.map(|index| {
        let target = addrs[index];
        let pps = options.flood_pps;
        let sources = options.flood_sources;
        let stop = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name(String::from("load-flood"))
            .spawn(move || flood(target, pps, sources, &stop))
            .expect("spawn flood thread")
    });
    if flood_thread.is_some() {
        // Let the flood reach steady state before the handshakes start.
        std::thread::sleep(Duration::from_millis(500));
    }

    // Optional link conditioning: one proxy per client.
    let mut proxies: Vec<LinkProxy> = Vec::new();
    let mut targets = Vec::with_capacity(options.matches * options.clients);
    for (match_index, addr) in addrs.iter().enumerate() {
        for client_index in 0..options.clients {
            let target = if conditioned {
                let proxy = LinkProxy::start(
                    "127.0.0.1:0".parse().expect("bind address"),
                    *addr,
                    LinkConfig {
                        latency: Duration::from_millis(options.rtt_ms / 2),
                        jitter: Duration::from_millis(options.jitter_ms),
                        loss_percent: options.loss_percent,
                        seed: 0x10AD_0000 + (match_index * 16 + client_index) as u64,
                    },
                )
                .unwrap_or_else(|error| fail(&format!("proxy failed: {error}")));
                let local = proxy.local_addr();
                proxies.push(proxy);
                local
            } else {
                *addr
            };
            targets.push((match_index, target));
        }
    }

    // Connect every client in parallel: `connect` blocks for a round trip or two, and a
    // sequential connect over a slow link would leave early clients unpolled long enough
    // to time out before the measurement starts.
    let connect_start = Instant::now();
    // Slowest handshake: (flooded match, every other match).
    let mut slowest_connect = (Duration::ZERO, Duration::ZERO);
    let mut flooded_connect_failures = 0usize;
    let mut clients: Vec<(usize, NetClient)> = std::thread::scope(|scope| {
        let handles: Vec<_> = targets
            .into_iter()
            .map(|(match_index, target)| {
                // Clients that already connected sit unpolled until every connect returns,
                // so the attacked match must give up before the 5 s server timeout.
                let timeout = if Some(match_index) == flood_match {
                    Duration::from_secs(3)
                } else {
                    Duration::from_secs(10)
                };
                scope.spawn(move || {
                    let started = Instant::now();
                    let result = NetClient::connect(target, timeout);
                    (match_index, result, started.elapsed())
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| {
                let (match_index, result, took) = handle.join().expect("connect thread panicked");
                let flooded = Some(match_index) == flood_match;
                let slot = if flooded {
                    &mut slowest_connect.0
                } else {
                    &mut slowest_connect.1
                };
                *slot = (*slot).max(took);
                match result {
                    Ok(client) => Some((match_index, client)),
                    // The attacked match is reported, not gated (see the module docs).
                    Err(_) if flooded => {
                        flooded_connect_failures += 1;
                        None
                    }
                    Err(error) => fail(&format!("client for match {match_index} failed: {error}")),
                }
            })
            .collect()
    });
    eprintln!(
        "connected {} clients in {:.1}s",
        clients.len(),
        connect_start.elapsed().as_secs_f64()
    );
    if flood_match.is_some() {
        eprintln!(
            "handshake under flood: slowest connect flooded_match={} ms other_matches={} ms flooded_match_connect_failures={flooded_connect_failures}",
            slowest_connect.0.as_millis(),
            slowest_connect.1.as_millis()
        );
    }

    // Split clients across driver threads and run them for the measurement window.
    let per_thread = clients.len().div_ceil(options.client_threads.max(1)).max(1);
    let mut groups: Vec<Vec<(usize, NetClient)>> = Vec::new();
    while !clients.is_empty() {
        let take = per_thread.min(clients.len());
        groups.push(clients.drain(..take).collect());
    }
    let measure = Duration::from_secs(options.seconds);
    let spawns = (content.map.blue_spawn, content.map.red_spawn);
    let team_size = i32::from(content.rules.team_size);
    let input_hz = options.input_hz;
    let drivers: Vec<_> = groups
        .into_iter()
        .enumerate()
        .map(|(thread_index, group)| {
            std::thread::Builder::new()
                .name(format!("load-clients-{thread_index}"))
                .spawn(move || drive_clients(group, measure, spawns, team_size, input_hz))
                .expect("spawn client driver")
        })
        .collect();
    let mut results: Vec<ClientResult> = Vec::new();
    for driver in drivers {
        results.extend(driver.join().expect("client driver panicked"));
    }
    shutdown.store(true, Ordering::Relaxed);
    let report = host_thread.join().expect("host thread panicked");
    let flood_report = flood_thread.map(|thread| thread.join().expect("flood thread panicked"));
    drop(proxies);

    // Report.
    let ticks = report.ticks();
    let p50 = ticks.percentile_nanos(50.0) / 1_000;
    let p99 = ticks.percentile_nanos(99.0) / 1_000;
    let max = ticks.max_nanos() / 1_000;
    let frames = report.frames().max(1);
    let overruns = report.overruns();
    #[allow(clippy::cast_precision_loss)] // Reporting only.
    let overrun_rate = overruns as f64 / frames as f64;
    eprintln!(
        "host: matches={} workers={} frames={} overruns={} ({:.2}%) match_ticks={} tick_p50_us={p50} tick_p99_us={p99} tick_max_us={max}",
        report.matches.len(),
        report.workers.len(),
        report.frames(),
        overruns,
        overrun_rate * 100.0,
        ticks.count(),
    );
    for (index, worker) in report.workers.iter().enumerate() {
        eprintln!(
            "  worker {index}: matches={} utilisation={:.1}% max_lateness_us={} errors={} p99_us={}",
            worker.matches,
            worker.utilisation() * 100.0,
            worker.max_lateness_nanos / 1_000,
            worker.errors,
            worker.ticks.percentile_nanos(99.0) / 1_000
        );
    }
    if let Some(flooded) = flood_match {
        // The attacked match's clients are reported; every other client is gated below.
        let (victims, others): (Vec<ClientResult>, Vec<ClientResult>) = results
            .into_iter()
            .partition(|result| result.match_index == flooded);
        let victims_connected = victims.iter().filter(|result| result.connected).count();
        let victims_gap = victims
            .iter()
            .map(|result| result.max_snapshot_gap)
            .max()
            .unwrap_or_default();
        eprintln!(
            "flood: flooded match clients connected={victims_connected}/{} (plus {flooded_connect_failures} that never connected) max_snapshot_gap_ms={}",
            victims.len(),
            victims_gap.as_millis()
        );
        results = others;
    }
    let connected = results.iter().filter(|result| result.connected).count();
    let mut down: Vec<f64> = results
        .iter()
        .map(|result| result.down_bytes_per_sec)
        .collect();
    let up_max = results
        .iter()
        .map(|result| result.up_bytes_per_sec)
        .fold(0.0, f64::max);
    down.sort_by(f64::total_cmp);
    #[allow(clippy::cast_precision_loss)] // Reporting only.
    let down_mean = down.iter().sum::<f64>() / down.len().max(1) as f64;
    let down_max = down.last().copied().unwrap_or(0.0);
    let max_gap = results
        .iter()
        .map(|result| result.max_snapshot_gap)
        .max()
        .unwrap_or_default();
    eprintln!(
        "clients: connected={connected}/{} down_mean={:.2} KB/s down_max={:.2} KB/s up_max={:.2} KB/s (incl. IP/UDP headers) max_snapshot_gap_ms={}",
        results.len(),
        down_mean / 1000.0,
        down_max / 1000.0,
        up_max / 1000.0,
        max_gap.as_millis()
    );
    let rate_limited: u64 = report.matches.iter().map(|stats| stats.rate_limited).sum();
    let match_avg_us = report
        .matches
        .iter()
        .map(|stats| stats.tick_avg_nanos)
        .max()
        .unwrap_or(0)
        / 1_000;
    eprintln!("server: worst match avg_tick_us={match_avg_us} rate_limited={rate_limited}");

    // The gated p99 covers every match normally; in flood mode it is the worst per-match p99
    // among the other matches (the flooded match's own receive work is reported separately).
    let gated_p99 = match flood_match {
        Some(flooded) => report_flood(&report, flooded, flood_report.as_ref()),
        None => p99,
    };

    let mut failures = Vec::new();
    if gated_p99 > options.tick_budget_us {
        failures.push(format!(
            "tick p99 {gated_p99} us > budget {} us",
            options.tick_budget_us
        ));
    }
    if flood_match.is_some() && overruns > 0 {
        failures.push(format!("{overruns} frames dropped under flood"));
    } else if overrun_rate > 0.01 {
        failures.push(format!("{:.2}% of frames overran", overrun_rate * 100.0));
    }
    if connected != results.len() {
        failures.push(format!(
            "{} clients disconnected",
            results.len() - connected
        ));
        let mut per_match = vec![0usize; options.matches];
        for result in results.iter().filter(|result| !result.connected) {
            per_match[result.match_index] += 1;
        }
        let affected: Vec<String> = per_match
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(index, count)| format!("m{index}:{count}"))
            .collect();
        eprintln!("  disconnected per match: {}", affected.join(" "));
    }
    if down_max > 40_000.0 || up_max > 4_000.0 || down_mean > 20_000.0 {
        failures.push(String::from("bandwidth budget exceeded"));
    }
    if max_gap > MAX_SNAPSHOT_GAP {
        failures.push(format!(
            "a client went {} ms without a snapshot",
            max_gap.as_millis()
        ));
    }
    if failures.is_empty() {
        eprintln!("load-test passed");
    } else {
        for failure in &failures {
            eprintln!("load-test FAILED: {failure}");
        }
        std::process::exit(1);
    }
}

/// Drives a group of clients: poll, consume events and fight.
///
/// At `input_hz` each client alternates between pushing towards the enemy base and casting
/// an ability ahead of itself, so both teams meet in lane fights (abilities, deaths and
/// projectiles), which is what drives snapshot size. The largest gap between applied
/// snapshots is tracked per client to catch stalls shorter than the disconnect timeout.
fn drive_clients(
    mut clients: Vec<(usize, NetClient)>,
    measure: Duration,
    (blue_spawn, red_spawn): (FixVec2, FixVec2),
    team_size: i32,
    input_hz: u32,
) -> Vec<ClientResult> {
    let baseline: Vec<ClientStats> = clients.iter().map(|(_, client)| client.stats()).collect();
    let mut connected = vec![true; clients.len()];
    let mut last_snapshot: Vec<(u64, Instant)> = baseline
        .iter()
        .map(|stats| (stats.snapshots, Instant::now()))
        .collect();
    let mut max_gap = vec![Duration::ZERO; clients.len()];
    let interval = Duration::from_secs_f64(1.0 / f64::from(input_hz.max(1)));
    let start = Instant::now();
    let mut last_command = Instant::now();
    let mut round = 0u32;
    while start.elapsed() < measure {
        let send = last_command.elapsed() >= interval;
        for (index, (_, client)) in clients.iter_mut().enumerate() {
            if client.poll().is_err() || client.update().is_err() {
                connected[index] = false;
            }
            while client.next_event().is_some() {}
            if client.state() != ClientState::Connected {
                connected[index] = false;
            }
            let snapshots = client.stats().snapshots;
            let now = Instant::now();
            if snapshots == last_snapshot[index].0 {
                max_gap[index] = max_gap[index].max(now - last_snapshot[index].1);
            } else {
                last_snapshot[index] = (snapshots, now);
            }
            if send {
                let slot = i32::from(client.player_slot());
                let target = if slot < team_size {
                    red_spawn
                } else {
                    blue_spawn
                };
                let (x, y) = quantize_position(target);
                let command = if (round + index as u32).is_multiple_of(2) {
                    WireCommand::MoveTo { x, y }
                } else {
                    // Out-of-range or on-cooldown casts are rejected by the server, which
                    // is realistic load too.
                    WireCommand::CastAbility {
                        slot: u8::try_from((round as usize + index) % 3).unwrap_or(0),
                        x,
                        y,
                    }
                };
                let _ = client.send_command(command);
            }
        }
        if send {
            last_command = Instant::now();
            round = round.wrapping_add(1);
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    let seconds = start.elapsed().as_secs_f64().max(0.001);
    clients
        .iter_mut()
        .enumerate()
        .map(|(index, (match_index, client))| {
            let stats = client.stats();
            let before = baseline[index];
            // Count the 28-byte IPv4 + UDP header of every datagram, so the budget is
            // checked against what the network actually carries.
            let down = stats.bytes_received.saturating_sub(before.bytes_received)
                + UDP_IPV4_OVERHEAD
                    * stats
                        .datagrams_received
                        .saturating_sub(before.datagrams_received);
            let up = stats.bytes_sent.saturating_sub(before.bytes_sent)
                + UDP_IPV4_OVERHEAD * stats.datagrams_sent.saturating_sub(before.datagrams_sent);
            #[allow(clippy::cast_precision_loss)] // Reporting only.
            let result = ClientResult {
                match_index: *match_index,
                connected: connected[index] && stats.state == Some(ClientState::Connected),
                down_bytes_per_sec: down as f64 / seconds,
                up_bytes_per_sec: up as f64 / seconds,
                max_snapshot_gap: max_gap[index],
            };
            let _ = client.disconnect();
            result
        })
        .collect()
}

/// Prints the flood-mode report and returns the worst per-match tick p99 (µs) among every
/// match except the flooded one.
fn report_flood(report: &HostReport, flooded: usize, flood: Option<&FloodReport>) -> u64 {
    let mut others = TickHistogram::default();
    let mut worst = (0usize, 0u64);
    for (index, histogram) in report.match_ticks.iter().enumerate() {
        if index == flooded {
            continue;
        }
        others.merge(histogram);
        let p99 = histogram.percentile_nanos(99.0) / 1_000;
        if p99 > worst.1 {
            worst = (index, p99);
        }
    }
    let worker = report.match_workers[flooded];
    let neighbours: Vec<String> = report
        .match_workers
        .iter()
        .enumerate()
        .filter(|(index, owner)| *index != flooded && **owner == worker)
        .map(|(index, _)| {
            format!(
                "m{index}:{}",
                report.match_ticks[index].percentile_nanos(99.0) / 1_000
            )
        })
        .collect();
    eprintln!(
        "flood: other matches tick_p50_us={} tick_p99_us={} worst_match_p99_us={} (m{})",
        others.percentile_nanos(50.0) / 1_000,
        others.percentile_nanos(99.0) / 1_000,
        worst.1,
        worst.0,
    );
    eprintln!(
        "flood: same-worker neighbours p99_us {}",
        neighbours.join(" ")
    );
    let stats = &report.matches[flooded];
    let ticks = &report.match_ticks[flooded];
    eprintln!(
        "flood: flooded match m{flooded} (worker {worker}) tick_p99_us={} tick_max_us={} rate_limited={} poll_capped={} recv={} KiB host_budget_rejected={}",
        ticks.percentile_nanos(99.0) / 1_000,
        ticks.max_nanos() / 1_000,
        stats.rate_limited,
        stats.poll_budget_exhausted,
        stats.bytes_received / 1024,
        report.host_budget_rejected,
    );
    if let Some(flood) = flood {
        #[allow(clippy::cast_precision_loss)] // Reporting only.
        let achieved = flood.sent as f64 / flood.seconds.max(0.001);
        eprintln!(
            "flood: sent={} datagrams in {:.1}s ({achieved:.0}/s) send_errors={}",
            flood.sent, flood.seconds, flood.send_errors
        );
    }
    worst.1
}

/// Sends `pps` undecodable datagrams per second to `target` from `sources` sockets until
/// `stop` is set. Paced in 1 ms batches; payloads vary in length and content but never
/// carry the protocol magic.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Pacing and junk bytes.
fn flood(target: SocketAddr, pps: u32, sources: usize, stop: &AtomicBool) -> FloodReport {
    let sockets: Vec<UdpSocket> = (0..sources)
        .map(|_| {
            UdpSocket::bind("127.0.0.1:0")
                .unwrap_or_else(|error| fail(&format!("flood socket: {error}")))
        })
        .collect();
    let mut junk = [0u8; 256];
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut report = FloodReport {
        sent: 0,
        send_errors: 0,
        seconds: 0.0,
    };
    let start = Instant::now();
    let mut next_socket = 0usize;
    while !stop.load(Ordering::Relaxed) {
        // Datagrams owed so far, so pacing self-corrects after a late wake-up.
        let due = (start.elapsed().as_secs_f64() * f64::from(pps)) as u64;
        while report.sent < due {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let length = 32 + (state % 200) as usize;
            for (index, byte) in junk[..length].iter_mut().enumerate() {
                *byte = (state >> ((index % 8) * 8)) as u8 ^ index as u8;
            }
            // Never the protocol magic, so every datagram fails header decoding.
            junk[0] = 0xFF;
            if sockets[next_socket]
                .send_to(&junk[..length], target)
                .is_err()
            {
                report.send_errors += 1;
            }
            next_socket = (next_socket + 1) % sockets.len();
            report.sent += 1;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    report.seconds = start.elapsed().as_secs_f64();
    report
}

fn parse<T: std::str::FromStr>(args: &mut impl Iterator<Item = String>, default: T) -> T {
    args.next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn fail(message: &str) -> ! {
    eprintln!("load-test failed: {message}");
    std::process::exit(1);
}
