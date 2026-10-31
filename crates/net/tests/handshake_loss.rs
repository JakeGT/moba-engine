//! Deterministic handshake-loss regressions: a scripted relay drops exactly the datagram
//! under test, so the handshake's retransmission is checked without random loss or timing.

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use moba_net::client::ClientState;
use moba_net::protocol::{PacketKind, decode_packet};
use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

/// Which direction a datagram travels through the relay.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    ToServer,
    ToClient,
}

/// A loss-free UDP relay that drops the datagrams its rule selects.
struct Relay {
    addr: SocketAddr,
    dropped: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Relay {
    fn start(
        upstream: SocketAddr,
        mut drop_rule: impl FnMut(Direction, &[u8]) -> bool + Send + 'static,
    ) -> Self {
        let client_side = UdpSocket::bind("127.0.0.1:0").expect("relay binds");
        client_side.set_nonblocking(true).expect("nonblocking");
        let server_side = UdpSocket::bind("127.0.0.1:0").expect("relay binds");
        server_side.set_nonblocking(true).expect("nonblocking");
        let addr = client_side.local_addr().expect("relay addr");
        let dropped = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_dropped, thread_stop) = (Arc::clone(&dropped), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            let mut client: Option<SocketAddr> = None;
            let mut buffer = [0u8; 2_048];
            while !thread_stop.load(Ordering::Acquire) {
                while let Ok((length, from)) = client_side.recv_from(&mut buffer) {
                    client.get_or_insert(from);
                    if drop_rule(Direction::ToServer, &buffer[..length]) {
                        thread_dropped.fetch_add(1, Ordering::Relaxed);
                    } else {
                        let _ = server_side.send_to(&buffer[..length], upstream);
                    }
                }
                while let Ok((length, from)) = server_side.recv_from(&mut buffer) {
                    let Some(client) = client.filter(|_| from == upstream) else {
                        continue;
                    };
                    if drop_rule(Direction::ToClient, &buffer[..length]) {
                        thread_dropped.fetch_add(1, Ordering::Relaxed);
                    } else {
                        let _ = client_side.send_to(&buffer[..length], client);
                    }
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            addr,
            dropped,
            stop,
            thread: Some(thread),
        }
    }

    fn dropped(&self) -> u32 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A plaintext server running until the returned flag is set.
fn start_server() -> (SocketAddr, Arc<AtomicBool>, JoinHandle<()>) {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().unwrap())
        .with_sim(SimConfig::default())
        .with_countdown_ticks(0)
        .with_results_ticks(0)
        .with_pre_shared_key(None)
        .with_record(None)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("server binds");
    let addr = server.local_addr().expect("addr");
    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shutdown);
    let thread = thread::spawn(move || {
        server
            .run_until(Duration::from_secs(30), &flag)
            .expect("server runs");
    });
    (addr, shutdown, thread)
}

/// Drops only the first datagram the client sends: its first hello.
fn first_hello_rule() -> impl FnMut(Direction, &[u8]) -> bool + Send + 'static {
    let mut seen_hello = false;
    move |direction, datagram| {
        if direction != Direction::ToServer || seen_hello {
            return false;
        }
        let Ok((header, _)) = decode_packet(datagram) else {
            return false;
        };
        seen_hello = header.kind == PacketKind::Hello;
        seen_hello
    }
}

/// Drops fragment 1 of the first fragmented Welcome, so that copy can never reassemble.
fn first_welcome_fragment_rule() -> impl FnMut(Direction, &[u8]) -> bool + Send + 'static {
    let mut dropped = false;
    move |direction, datagram| {
        if direction != Direction::ToClient || dropped {
            return false;
        }
        let Ok((header, payload)) = decode_packet(datagram) else {
            return false;
        };
        // Fragment payload: message id (2), index (2), total (2), inner kind (1), seq (4).
        if header.kind != PacketKind::Fragment || payload.len() < 7 {
            return false;
        }
        let index = u16::from_le_bytes([payload[2], payload[3]]);
        dropped = payload[6] == PacketKind::Welcome as u8 && index == 1;
        dropped
    }
}

/// Drives a non-blocking client the way a game loop does, until it connects or times out.
fn pump_until_connected(client: &mut NetClient, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        client.poll().expect("poll");
        client.update().expect("update");
        if client.state() == ClientState::Connected {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

#[test]
fn non_blocking_start_retransmits_a_lost_hello() {
    let (server, shutdown, thread) = start_server();
    let relay = Relay::start(server, first_hello_rule());
    let mut client = NetClient::start(relay.addr).expect("client starts");
    // Without a hello retransmission in `update`, this client would wait forever.
    assert!(
        pump_until_connected(&mut client, Duration::from_secs(3)),
        "the client never retransmitted its lost hello"
    );
    assert_eq!(relay.dropped(), 1);
    drop(relay);
    shutdown.store(true, Ordering::Relaxed);
    thread.join().expect("server thread");
}

#[test]
fn non_blocking_start_recovers_a_lost_welcome_fragment() {
    let (server, shutdown, thread) = start_server();
    let relay = Relay::start(server, first_welcome_fragment_rule());
    let mut client = NetClient::start(relay.addr).expect("client starts");
    assert!(
        pump_until_connected(&mut client, Duration::from_secs(3)),
        "a lost Welcome fragment was never recovered"
    );
    assert_eq!(relay.dropped(), 1, "the Welcome must be fragmented");
    drop(relay);
    shutdown.store(true, Ordering::Relaxed);
    thread.join().expect("server thread");
}

#[test]
fn blocking_connect_recovers_a_lost_welcome_fragment() {
    let (server, shutdown, thread) = start_server();
    let relay = Relay::start(server, first_welcome_fragment_rule());
    let client = NetClient::connect(relay.addr, Duration::from_secs(3)).expect("connect");
    assert_eq!(client.state(), ClientState::Connected);
    assert_eq!(relay.dropped(), 1, "the Welcome must be fragmented");
    drop(relay);
    shutdown.store(true, Ordering::Relaxed);
    thread.join().expect("server thread");
}
