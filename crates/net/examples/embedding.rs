//! Minimal `NetServer` embedding with an extension system.
//!
#![allow(clippy::print_stdout, clippy::print_stderr)]

//! Run with: `cargo run -p moba-net --example embedding`

use std::time::Duration;

use moba_net::{NetServer, ServerConfig};
use moba_sim::SimConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ServerConfig {
        bind: "127.0.0.1:0".parse()?,
        sim: SimConfig::default(),
        snapshot_interval: 1,
        max_clients: 10,
        countdown_ticks: 0,
        results_ticks: 30,
        client_timeout: Duration::from_secs(3),
        reconnect_grace: Duration::from_secs(2),
        pre_shared_key: None,
        record: None,
        max_spectators: 8,
        max_packets_per_second: 120,
        max_total_packets_per_second: 2_000,
        require_ready: false,
    };
    let mut server = NetServer::bind(config)?.with_system(|_simulation, phase| {
        eprintln!("server phase: {phase:?}");
    });
    let addr = server.local_addr()?;
    println!("listening on {addr}");
    for _ in 0..10 {
        server.tick()?;
    }
    Ok(())
}
