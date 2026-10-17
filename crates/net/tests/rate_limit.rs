//! Per-address UDP budgets reject packet floods before decoding.

use std::net::UdpSocket;
use std::time::Duration;

use moba_net::protocol::{Header, PacketKind, encode_packet};
use moba_net::{NetServer, ServerConfig};
use moba_sim::SimConfig;

#[test]
fn flood_is_rate_limited() {
    let mut server = NetServer::bind(ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        sim: SimConfig::default(),
        snapshot_interval: 1,
        max_clients: 10,
        countdown_ticks: 0,
        results_ticks: 0,
        client_timeout: Duration::from_secs(2),
        reconnect_grace: Duration::from_secs(2),
        pre_shared_key: None,
        record: None,
        max_spectators: 4,
        max_packets_per_second: 10,
        max_total_packets_per_second: 0,
        require_ready: false,
    })
    .expect("bind");
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server.run_for(Duration::from_secs(1)).expect("server");
        server.stats()
    });
    std::thread::sleep(Duration::from_millis(100));
    let flood = UdpSocket::bind("127.0.0.1:0").expect("flood socket");
    let header = Header {
        kind: PacketKind::Ping,
        flags: 0,
        session: 1,
        seq: 1,
        ack: 0,
        ack_bits: 0,
    };
    let mut datagram = Vec::new();
    encode_packet(&header, &[], &mut datagram).expect("encode");
    for _ in 0..300 {
        let _ = flood.send_to(&datagram, addr);
    }
    let stats = handle.join().expect("server thread");
    assert!(
        stats.rate_limited > 0,
        "flood packets were dropped: {stats:?}"
    );
}
