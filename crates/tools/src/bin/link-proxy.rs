#![allow(clippy::print_stdout, clippy::print_stderr)]
//! A standalone link conditioner in front of a real server (R4b exit evidence).
//!
//! ```text
//! link-proxy --upstream 127.0.0.1:7777 [--listen 127.0.0.1:7778] [--rtt 150] [--jitter 30]
//!            [--loss 5] [--seconds 120] [--seed 1]
//! ```
//!
//! Forwards one client's UDP traffic to `--upstream` through `moba_netsim::LinkProxy`, adding
//! `rtt / 2` latency each way, uniform jitter and independent loss: the conditions the
//! engine's networking budgets name (150 ms RTT, 30 ms jitter, 5 % loss). Point a client at
//! it with `MOBA_SERVER=<listen>`. Prints the forwarded and dropped packet counts at exit.

use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use moba_netsim::{LinkConfig, LinkProxy};

struct Options {
    listen: SocketAddr,
    upstream: SocketAddr,
    rtt_ms: u64,
    jitter_ms: u64,
    loss_percent: u8,
    seconds: u64,
    seed: u64,
}

fn parse() -> Result<Options, String> {
    let mut options = Options {
        listen: "127.0.0.1:7778".parse().map_err(|_| "listen")?,
        upstream: "127.0.0.1:7777".parse().map_err(|_| "upstream")?,
        rtt_ms: 150,
        jitter_ms: 30,
        loss_percent: 5,
        seconds: 120,
        seed: 1,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--listen" => options.listen = value()?.parse().map_err(|_| "bad --listen")?,
            "--upstream" => options.upstream = value()?.parse().map_err(|_| "bad --upstream")?,
            "--rtt" => options.rtt_ms = value()?.parse().map_err(|_| "bad --rtt")?,
            "--jitter" => options.jitter_ms = value()?.parse().map_err(|_| "bad --jitter")?,
            "--loss" => {
                options.loss_percent = value()?
                    .parse()
                    .ok()
                    .filter(|loss| *loss <= 100)
                    .ok_or("bad --loss (0-100)")?;
            }
            "--seconds" => options.seconds = value()?.parse().map_err(|_| "bad --seconds")?,
            "--seed" => options.seed = value()?.parse().map_err(|_| "bad --seed")?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(options)
}

fn main() -> ExitCode {
    let options = match parse() {
        Ok(options) => options,
        Err(error) => {
            eprintln!(
                "link-proxy: {error}\nusage: link-proxy --upstream ADDR [--listen ADDR] [--rtt MS] \
                 [--jitter MS] [--loss PCT] [--seconds N] [--seed N]"
            );
            return ExitCode::from(2);
        }
    };
    let config = LinkConfig {
        latency: Duration::from_millis(options.rtt_ms / 2),
        jitter: Duration::from_millis(options.jitter_ms),
        loss_percent: options.loss_percent,
        seed: options.seed,
    };
    let proxy = match LinkProxy::start(options.listen, options.upstream, config) {
        Ok(proxy) => proxy,
        Err(error) => {
            eprintln!("link-proxy: cannot bind {}: {error}", options.listen);
            return ExitCode::FAILURE;
        }
    };
    println!(
        "link-proxy: {} -> {} with {} ms RTT, {} ms jitter, {} % loss for {} s",
        proxy.local_addr(),
        options.upstream,
        options.rtt_ms,
        options.jitter_ms,
        options.loss_percent,
        options.seconds
    );
    let stats = proxy.stats();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(options.seconds) {
        std::thread::sleep(Duration::from_millis(250));
    }
    proxy.stop();
    println!(
        "link-proxy: forwarded {} packets ({} bytes), dropped {}",
        stats.forwarded(),
        stats.bytes(),
        stats.dropped()
    );
    ExitCode::SUCCESS
}
