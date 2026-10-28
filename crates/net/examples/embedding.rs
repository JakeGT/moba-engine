//! Minimal `NetServer` embedding with an extension system.
//!
#![allow(clippy::print_stdout, clippy::print_stderr)]

//! Run with: `cargo run -p moba-net --example embedding`

use std::time::Duration;

use moba_net::{NetServer, ServerConfig};
use moba_sim::SimConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse()?)
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(30)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
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
