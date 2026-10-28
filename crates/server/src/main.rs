//! Headless authoritative dedicated server.
//!
//! Runs the deterministic simulation at 30 Hz with server-side bots and reports tick budget,
//! wave progress, peak unit count and the final state hash. Networking attaches in M5.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use moba_game::GameContent;
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
    let mut secret: Option<[u8; 32]> = std::env::var("MOBA_SECRET")
        .ok()
        .map(|value| parse_secret(&value));
    let mut record: Option<std::path::PathBuf> = None;
    let mut items_paths: Vec<std::path::PathBuf> = Vec::new();
    let mut server_key: Option<std::path::PathBuf> = None;
    let mut reload_content = false;
    let mut require_attestation = false;
    let mut content_publisher: Option<[u8; 32]> = None;
    let mut client_tokens: Vec<[u8; 32]> = Vec::new();
    let mut client_identity_keys: Vec<[u8; 32]> = Vec::new();
    let mut rotate_client_keys: Vec<([u8; 32], [u8; 32])> = Vec::new();
    let mut revoke_client_keys: Vec<[u8; 32]> = Vec::new();
    let mut results_log: Option<std::path::PathBuf> = None;
    let mut require_ready = false;
    let mut max_packets_per_second = 120u32;
    let mut max_total_packets_per_second = 2_000u32;
    let mut host_max_packets_per_second = 0u32;
    let mut match_count = 1usize;
    let mut workers: Option<usize> = None;
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let config_path = raw_args
        .iter()
        .position(|argument| argument == "--config")
        .and_then(|index| raw_args.get(index + 1))
        .map(std::path::PathBuf::from);
    if let Some(path) = config_path.as_ref() {
        let file = load_server_config(path);
        if let Some(value) = file.listen {
            listen = Some(value);
        }
        if let Some(value) = file.seconds {
            seconds = value;
        }
        if let Some(value) = file.countdown {
            countdown_secs = value;
        }
        if let Some(value) = file.results {
            results_secs = value;
        }
        if let Some(value) = file.seed {
            seed = value;
        }
        if let Some(value) = file.ticks {
            ticks = value;
        }
        if let Some(value) = file.secret {
            secret = Some(parse_secret(&value));
        }
        if let Some(value) = file.record {
            record = Some(value);
        }
        if let Some(value) = file.results_log {
            results_log = Some(value);
        }
        if let Some(value) = file.items {
            items_paths.push(value);
        }
        if let Some(values) = file.items_rotation {
            items_paths.extend(values);
        }
        if let Some(value) = file.server_key {
            server_key = Some(value);
        }
        if let Some(value) = file.reload_content {
            reload_content = value;
        }
        if let Some(value) = file.require_attestation {
            require_attestation = value;
        }
        if let Some(value) = file.content_publisher.as_deref() {
            content_publisher = Some(parse_secret(value));
        }
        if let Some(value) = file.ready {
            require_ready = value;
        }
        if let Some(value) = file.max_pps {
            max_packets_per_second = value;
        }
        if let Some(tokens) = file.client_tokens {
            client_tokens = tokens.iter().map(|token| parse_secret(token)).collect();
        }
        if let Some(keys) = file.client_identity_keys {
            client_identity_keys = keys.iter().map(|key| parse_secret(key)).collect();
        }
        if let Some(keys) = file.rotate_client_keys {
            rotate_client_keys = keys.iter().map(|pair| parse_key_pair(pair)).collect();
        }
        if let Some(keys) = file.revoke_client_keys {
            revoke_client_keys = keys.iter().map(|key| parse_secret(key)).collect();
        }
        if let Some(value) = file.max_total_pps {
            max_total_packets_per_second = value;
        }
        if let Some(value) = file.host_max_pps {
            host_max_packets_per_second = value;
        }
        if let Some(value) = file.matches {
            match_count = value;
        }
        if file.workers.is_some() {
            workers = file.workers;
        }
    }
    let mut args = raw_args.into_iter();
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
            "--secret" => {
                let Some(value) = args.next() else {
                    eprintln!("--secret needs 64 hex characters");
                    std::process::exit(2);
                };
                secret = Some(parse_secret(&value));
            }
            "--record" => {
                record = args.next().map(std::path::PathBuf::from);
            }
            "--results-log" => results_log = args.next().map(std::path::PathBuf::from),
            "--ready" => require_ready = true,
            "--matches" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    match_count = value;
                }
            }
            "--workers" => workers = args.next().and_then(|value| value.parse().ok()),
            "--max-pps" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    max_packets_per_second = value;
                }
            }
            "--max-total-pps" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    max_total_packets_per_second = value;
                }
            }
            "--host-max-pps" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    host_max_packets_per_second = value;
                }
            }
            "--items" => {
                if let Some(value) = args.next() {
                    items_paths.push(std::path::PathBuf::from(value));
                }
            }
            "--server-key" => server_key = args.next().map(std::path::PathBuf::from),
            "--allow-client-key" => {
                let Some(value) = args.next() else {
                    eprintln!("--allow-client-key needs 64 hex characters");
                    std::process::exit(2);
                };
                client_identity_keys.push(parse_secret(&value));
            }
            "--allow-token" => {
                let Some(value) = args.next() else {
                    eprintln!("--allow-token needs 64 hex characters");
                    std::process::exit(2);
                };
                client_tokens.push(parse_secret(&value));
            }
            "--rotate-client-key" => {
                let Some(value) = args.next() else {
                    eprintln!("--rotate-client-key needs OLD:NEW 64-hex keys");
                    std::process::exit(2);
                };
                rotate_client_keys.push(parse_key_pair(&value));
            }
            "--revoke-client-key" => {
                let Some(value) = args.next() else {
                    eprintln!("--revoke-client-key needs 64 hex characters");
                    std::process::exit(2);
                };
                revoke_client_keys.push(parse_secret(&value));
            }
            "--reload-content" => reload_content = true,
            "--require-content-attestation" => require_attestation = true,
            "--content-publisher" => {
                let Some(value) = args.next() else {
                    eprintln!("--content-publisher needs 64 hex characters");
                    std::process::exit(2);
                };
                content_publisher = Some(parse_secret(&value));
            }
            "--config" => {
                let _ = args.next();
            }
            "--help" => {
                eprintln!(
                    "usage: moba-server [--ticks N] [--seed N] \
                     [--listen ADDR --seconds N --countdown S --results S --secret HEX --record FILE] [--results-log FILE] [--ready] [--items FILE]... [--max-pps N] [--max-total-pps N] [--host-max-pps N] [--matches N] [--workers K] [--server-key FILE] [--reload-content] [--require-content-attestation] [--content-publisher HEX] [--allow-token HEX] [--allow-client-key HEX] [--rotate-client-key OLD:NEW] [--revoke-client-key HEX] [--config FILE]"
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

    if content_publisher.is_some() {
        // A pinned publisher only has meaning with attestation enforcement.
        require_attestation = true;
    }
    if let Some(listen) = listen {
        let bind = listen.parse().unwrap_or_else(|error| {
            eprintln!("invalid --listen address {listen}: {error}");
            std::process::exit(2);
        });
        let content = items_paths.first().map(|path| load_content(path));
        if require_attestation {
            if content_publisher.is_none() {
                eprintln!("--require-content-attestation requires --content-publisher");
                std::process::exit(2);
            }
            let Some(content) = content.as_ref() else {
                eprintln!("--require-content-attestation requires --items FILE");
                std::process::exit(2);
            };
            if !moba_net::crypto::verify_pack(content) {
                eprintln!("content attestation is missing or invalid");
                std::process::exit(1);
            }
            if let Some(expected) = content_publisher
                && content
                    .attestation
                    .as_ref()
                    .map(|attestation| attestation.publisher)
                    != Some(expected)
            {
                eprintln!("content publisher does not match the trusted key");
                std::process::exit(1);
            }
        }
        let server_config = ServerConfig {
            bind,
            sim: config,
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: u32::try_from(countdown_secs.saturating_mul(30)).unwrap_or(u32::MAX),
            results_ticks: u32::try_from(results_secs.saturating_mul(30)).unwrap_or(u32::MAX),
            client_timeout: Duration::from_secs(5),
            reconnect_grace: Duration::from_secs(60),
            pre_shared_key: secret,
            record,
            max_spectators: 8,
            max_packets_per_second,
            max_total_packets_per_second,
            require_ready,
        };
        if let Some(content) = content.as_ref() {
            eprintln!(
                "content: {} {} ({} items, hash {:#018x})",
                content.manifest.name,
                content.manifest.version,
                content.items.len(),
                content.hash()
            );
        }
        if reload_content && items_paths.is_empty() {
            eprintln!("--reload-content requires --items FILE");
            std::process::exit(2);
        }
        // Fail loudly on keys `verify_strict` would reject on every handshake.
        for key in client_identity_keys
            .iter()
            .chain(rotate_client_keys.iter().map(|(_, new)| new))
        {
            if !moba_net::crypto::is_valid_identity_key(key) {
                eprintln!(
                    "client identity key is not a valid Ed25519 public key (malformed or weak)"
                );
                std::process::exit(2);
            }
        }
        let identity_seed = server_key.as_ref().map(|path| load_identity_seed(path));
        let content = content.map(Arc::new);
        if match_count == 0 {
            eprintln!("--matches must be at least 1");
            std::process::exit(2);
        }
        let multi = match_count > 1;
        let mut servers = Vec::with_capacity(match_count);
        for index in 0..match_count {
            // Match 0 keeps the configured port, seed and paths, so `--matches 1` is
            // exactly the single-match server.
            let mut match_config = server_config.clone();
            match_config.bind = offset_port(bind, index);
            match_config.sim.seed = config
                .seed
                .wrapping_add((index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            if multi {
                match_config.record = match_config
                    .record
                    .as_deref()
                    .map(|path| moba_net::host::suffixed_path(path, index));
            }
            let mut server = if let Some(content) = content.as_ref() {
                NetServer::bind_with_content(match_config, Arc::clone(content))
            } else {
                NetServer::bind(match_config)
            }
            .unwrap_or_else(|error| {
                eprintln!("cannot bind {}: {error}", offset_port(bind, index));
                std::process::exit(2);
            });
            server.set_content_trust(require_attestation, content_publisher);
            if items_paths.len() > 1 {
                server.watch_rotation(items_paths.clone());
            }
            if !client_identity_keys.is_empty() {
                server.require_client_identity(client_identity_keys.iter().copied());
            }
            for (old, new) in &rotate_client_keys {
                if !server.rotate_client_key(*old, *new) && index == 0 {
                    eprintln!("client key rotation skipped: old key is not in the allowlist");
                }
            }
            for key in &revoke_client_keys {
                if !server.revoke_client_key(*key) && index == 0 {
                    eprintln!("client key revocation skipped: key is not in the allowlist");
                }
            }
            if index == 0 && (!rotate_client_keys.is_empty() || !revoke_client_keys.is_empty()) {
                eprintln!(
                    "client identity allowlist: {} key(s)",
                    server.allowed_client_key_count()
                );
            }
            if let Some(path) = results_log.as_deref() {
                server.set_results_log(if multi {
                    moba_net::host::suffixed_path(path, index)
                } else {
                    path.to_path_buf()
                });
            }
            for token in &client_tokens {
                server.add_client_key(*token);
            }
            let mut identity_note = String::new();
            if let Some(seed) = identity_seed {
                let public = server.set_server_identity(seed);
                identity_note = format!(" identity={}", hex(&public));
            }
            eprintln!(
                "network server{} listening on {} for {seconds}s (30 Hz, 10 bot slots, {countdown_secs}s countdown, {results_secs}s results, {}){identity_note}",
                if multi {
                    format!(" match {index}")
                } else {
                    String::new()
                },
                server.local_addr().expect("bound address"),
                if secret.is_some() || !client_tokens.is_empty() {
                    "encrypted"
                } else {
                    "plaintext LAN mode"
                }
            );
            servers.push(server);
        }
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal_flag = Arc::clone(&shutdown);
        if let Err(error) = ctrlc::set_handler(move || {
            signal_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }) {
            eprintln!("signal handler: {error}");
        }
        let worker_count = workers.unwrap_or_else(|| {
            std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
        });
        let mut host = moba_net::MatchHost::new(servers, worker_count);
        if reload_content && let Some(path) = items_paths.first().cloned() {
            // One loader for the whole process: every match adopts the same pack.
            host.watch_content(Arc::new(moba_net::ContentWatch::new(
                path,
                require_attestation,
                content_publisher,
            )));
        }
        if host_max_packets_per_second > 0 {
            host.set_host_packet_budget(host_max_packets_per_second);
            eprintln!("host-wide datagram budget: {host_max_packets_per_second} packets/s");
        }
        if multi {
            eprintln!(
                "hosting {match_count} matches on {} worker thread(s)",
                host.workers()
            );
        }
        let report = host.run_until(Duration::from_secs(seconds), &shutdown);
        eprintln!("network shutdown requested; replay flushed if one was in progress");
        for (index, stats) in report.matches.iter().enumerate() {
            eprintln!(
                "network done{}: clients={} ticks={} snapshots={} sent={} KiB recv={} KiB rate_limited={} replayed_hellos={} avg_tick_us={} max_tick_us={}",
                if multi {
                    format!(" match {index}")
                } else {
                    String::new()
                },
                stats.clients,
                stats.ticks,
                stats.snapshots_sent,
                stats.bytes_sent / 1024,
                stats.bytes_received / 1024,
                stats.rate_limited,
                stats.replayed_hellos,
                stats.tick_avg_nanos / 1_000,
                stats.tick_max_nanos / 1_000
            );
        }
        if multi {
            let ticks = report.ticks();
            eprintln!(
                "host: frames={} overruns={} tick_p50_us={} tick_p99_us={} tick_max_us={}",
                report.frames(),
                report.overruns(),
                ticks.percentile_nanos(50.0) / 1_000,
                ticks.percentile_nanos(99.0) / 1_000,
                ticks.max_nanos() / 1_000
            );
        }
        if host_max_packets_per_second > 0 {
            eprintln!(
                "host budget: rejected={} datagrams",
                report.host_budget_rejected
            );
        }
        let errors: u64 = report.workers.iter().map(|worker| worker.errors).sum();
        if errors > 0 {
            eprintln!("network server: {errors} socket error(s) during the run");
            std::process::exit(1);
        }
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

fn parse_secret(value: &str) -> [u8; 32] {
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        eprintln!("secret must be 64 hex characters");
        std::process::exit(2);
    }
    let mut key = [0u8; 32];
    for (index, chunk) in bytes.as_chunks::<2>().0.iter().enumerate() {
        if !chunk.iter().all(u8::is_ascii_hexdigit) {
            eprintln!("secret contains non-hex characters");
            std::process::exit(2);
        }
        let text = std::str::from_utf8(chunk).unwrap_or_default();
        key[index] = u8::from_str_radix(text, 16).unwrap_or_else(|_| {
            eprintln!("secret contains non-hex characters");
            std::process::exit(2);
        });
    }
    key
}

fn load_content(path: &std::path::Path) -> GameContent {
    moba_game::load_content_file(path).unwrap_or_else(|error| {
        eprintln!("invalid content {}: {error}", path.display());
        std::process::exit(2);
    })
}

fn load_identity_seed(path: &std::path::Path) -> [u8; 32] {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| {
        eprintln!("cannot read server key {}: {error}", path.display());
        std::process::exit(2);
    });
    let text = text.trim();
    let mut seed = [0u8; 32];
    if text.len() != 64 {
        eprintln!("server key must be 64 hex characters");
        std::process::exit(2);
    }
    for (index, chunk) in text.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let value = std::str::from_utf8(chunk).unwrap_or_default();
        seed[index] = u8::from_str_radix(value, 16).unwrap_or_else(|_| {
            eprintln!("server key contains non-hex characters");
            std::process::exit(2);
        });
    }
    seed
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[derive(serde::Deserialize, Default)]
struct FileServerConfig {
    listen: Option<String>,
    seconds: Option<u64>,
    countdown: Option<u64>,
    results: Option<u64>,
    seed: Option<u64>,
    ticks: Option<u32>,
    secret: Option<String>,
    record: Option<std::path::PathBuf>,
    results_log: Option<std::path::PathBuf>,
    items: Option<std::path::PathBuf>,
    items_rotation: Option<Vec<std::path::PathBuf>>,
    server_key: Option<std::path::PathBuf>,
    reload_content: Option<bool>,
    require_attestation: Option<bool>,
    content_publisher: Option<String>,
    ready: Option<bool>,
    max_pps: Option<u32>,
    max_total_pps: Option<u32>,
    host_max_pps: Option<u32>,
    matches: Option<usize>,
    workers: Option<usize>,
    client_tokens: Option<Vec<String>>,
    client_identity_keys: Option<Vec<String>>,
    rotate_client_keys: Option<Vec<String>>,
    revoke_client_keys: Option<Vec<String>>,
}

/// `addr` with its port increased by `index` (exits on overflow; port 0 stays ephemeral).
fn offset_port(addr: std::net::SocketAddr, index: usize) -> std::net::SocketAddr {
    if addr.port() == 0 || index == 0 {
        return addr;
    }
    let Some(port) = u16::try_from(usize::from(addr.port()) + index).ok() else {
        eprintln!(
            "--matches {index}+ overflows the port range from {}",
            addr.port()
        );
        std::process::exit(2);
    };
    let mut addr = addr;
    addr.set_port(port);
    addr
}

fn parse_key_pair(value: &str) -> ([u8; 32], [u8; 32]) {
    let Some((old, new)) = value.split_once(':') else {
        eprintln!("key rotation must be OLD:NEW");
        std::process::exit(2);
    };
    (parse_secret(old), parse_secret(new))
}

fn load_server_config(path: &std::path::Path) -> FileServerConfig {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| {
        eprintln!("cannot read config {}: {error}", path.display());
        std::process::exit(2);
    });
    toml::from_str(&text).unwrap_or_else(|error| {
        eprintln!("invalid config {}: {error}", path.display());
        std::process::exit(2);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_parses_deployment_fields() {
        let path = std::env::temp_dir().join(format!(
            "moba-server-config-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::write(
            &path,
            "listen = \"127.0.0.1:7777\"\nseconds = 120\ncountdown = 2\nready = true\nmax_pps = 60\n",
        )
        .expect("write config");
        let file = load_server_config(&path);
        assert_eq!(file.listen.as_deref(), Some("127.0.0.1:7777"));
        assert_eq!(file.seconds, Some(120));
        assert_eq!(file.countdown, Some(2));
        assert_eq!(file.ready, Some(true));
        assert_eq!(file.max_pps, Some(60));
        let _ = std::fs::remove_file(&path);
    }
}
