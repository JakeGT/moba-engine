//! Per-address UDP budgets reject packet floods before decoding.

use std::net::UdpSocket;
use std::time::Duration;

use moba_net::protocol::{Header, PacketKind, encode_packet};
use moba_net::{NetServer, ServerConfig};
use moba_sim::SimConfig;

#[test]
fn flood_is_rate_limited() {
    let mut server = NetServer::bind(
        ServerConfig::default()
            .with_bind("127.0.0.1:0".parse().unwrap())
            .with_sim(SimConfig::default())
            .with_snapshot_interval(1)
            .with_max_clients(10)
            .with_countdown_ticks(0)
            .with_results_ticks(0)
            .with_client_timeout(Duration::from_secs(3))
            .with_reconnect_grace(Duration::from_secs(2))
            .with_pre_shared_key(None)
            .with_record(None)
            .with_max_spectators(4)
            .with_max_packets_per_second(10)
            .with_max_total_packets_per_second(0)
            .with_require_ready(false),
    )
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

fn junk_server(max_packets_per_second: u32) -> NetServer {
    NetServer::bind(
        ServerConfig::default()
            .with_bind("127.0.0.1:0".parse().unwrap())
            .with_countdown_ticks(0)
            .with_max_packets_per_second(max_packets_per_second)
            .with_max_total_packets_per_second(0),
    )
    .expect("bind")
}

/// Polls until the server stops finding datagrams (loopback delivery is not instantaneous on
/// every platform).
fn drain(server: &mut NetServer) {
    let mut idle = 0;
    let mut last = server.stats().rate_limited;
    while idle < 20 {
        server.poll().expect("poll");
        let now = server.stats().rate_limited;
        if now == last {
            idle += 1;
            std::thread::sleep(Duration::from_millis(5));
        } else {
            idle = 0;
            last = now;
        }
    }
}

#[test]
fn undecodable_datagrams_count_against_the_address_budget() {
    let mut server = junk_server(5);
    let addr = server.local_addr().expect("addr");
    let flood = UdpSocket::bind("127.0.0.1:0").expect("flood socket");
    // Junk that fails header decoding (bad magic); it used to be dropped without ever
    // reaching the limiter.
    for _ in 0..40 {
        let _ = flood.send_to(&[0xEE; 48], addr);
    }
    drain(&mut server);
    let limited = server.stats().rate_limited;
    // The burst of 5 (plus a token or two of refill) is admitted; the rest is charged.
    assert!(
        (30..=35).contains(&limited),
        "undecodable junk was not charged: rate_limited={limited}"
    );
}

#[test]
fn budgeted_poll_leaves_the_rest_queued() {
    let mut server = junk_server(1);
    let addr = server.local_addr().expect("addr");
    let flood = UdpSocket::bind("127.0.0.1:0").expect("flood socket");
    for _ in 0..30 {
        let _ = flood.send_to(&[0xEE; 48], addr);
    }
    std::thread::sleep(Duration::from_millis(50));
    let exhausted = server
        .poll_budgeted(8, Duration::from_secs(1))
        .expect("poll");
    assert!(exhausted, "eight datagrams cannot drain thirty");
    // One admitted (then dropped as undecodable), seven charged.
    assert_eq!(server.stats().rate_limited, 7);
    assert_eq!(server.stats().poll_budget_exhausted, 1);
    drain(&mut server);
    assert!(server.stats().rate_limited >= 28);
}
