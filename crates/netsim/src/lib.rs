//! Link conditioner: a UDP proxy that adds latency, jitter, loss and reordering.
//!
//! One proxy instance forwards one client's traffic to a server. Tests place a proxy in
//! front of every client to emulate a real network (150 ms RTT, 5% loss, 30 ms jitter) while
//! the server and clients use ordinary UDP sockets.

#![forbid(unsafe_code)]

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use moba_core::Rng;

/// Link characteristics applied in each direction.
#[derive(Clone, Copy, Debug)]
pub struct LinkConfig {
    /// One-way base latency.
    pub latency: Duration,
    /// Extra uniform jitter added to each packet.
    pub jitter: Duration,
    /// Packet drop probability in percent (0-100).
    pub loss_percent: u8,
    /// RNG seed so a soak run is reproducible.
    pub seed: u64,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            latency: Duration::from_millis(75),
            jitter: Duration::from_millis(30),
            loss_percent: 5,
            seed: 0xC0FF_EE00,
        }
    }
}

/// Counters shared with the proxy thread.
#[derive(Debug, Default)]
pub struct LinkStats {
    forwarded: AtomicU64,
    dropped: AtomicU64,
    bytes: AtomicU64,
}

impl LinkStats {
    /// Packets forwarded in both directions.
    #[must_use]
    pub fn forwarded(&self) -> u64 {
        self.forwarded.load(Ordering::Relaxed)
    }

    /// Packets dropped by the conditioner.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Bytes forwarded.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }
}

struct Scheduled {
    due: Instant,
    to_server: bool,
    data: Vec<u8>,
}

/// A running proxy. Dropping it stops the thread.
#[derive(Debug)]
pub struct LinkProxy {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    stats: Arc<LinkStats>,
    thread: Option<thread::JoinHandle<()>>,
}

impl LinkProxy {
    /// Starts a proxy listening on `listen` and forwarding to `upstream`.
    ///
    /// # Errors
    /// Returns an OS error when either socket cannot be bound.
    pub fn start(listen: SocketAddr, upstream: SocketAddr, config: LinkConfig) -> io::Result<Self> {
        let client_socket = UdpSocket::bind(listen)?;
        client_socket.set_nonblocking(true)?;
        let addr = client_socket.local_addr()?;
        let server_socket = UdpSocket::bind("0.0.0.0:0")?;
        server_socket.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(LinkStats::default());
        let thread_stop = Arc::clone(&stop);
        let thread_stats = Arc::clone(&stats);
        let handle = thread::Builder::new()
            .name(format!("moba-link-{}", addr.port()))
            .spawn(move || {
                run_proxy(
                    client_socket,
                    server_socket,
                    upstream,
                    config,
                    thread_stop,
                    thread_stats,
                );
            })?;
        Ok(Self {
            addr,
            stop,
            stats,
            thread: Some(handle),
        })
    }

    /// Address clients should connect to.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Shared counters.
    #[must_use]
    pub fn stats(&self) -> Arc<LinkStats> {
        Arc::clone(&self.stats)
    }

    /// Stops the proxy and waits for its thread.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LinkProxy {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[allow(clippy::needless_pass_by_value)] // Sockets and handles are owned by the proxy thread.
fn run_proxy(
    client_socket: UdpSocket,
    server_socket: UdpSocket,
    upstream: SocketAddr,
    config: LinkConfig,
    stop: Arc<AtomicBool>,
    stats: Arc<LinkStats>,
) {
    let mut rng = Rng::from_seed(config.seed);
    let mut client: Option<SocketAddr> = None;
    let mut queue: Vec<Scheduled> = Vec::with_capacity(256);
    let mut buffer = [0u8; 2_048];
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        // Client -> server.
        while let Ok((length, from)) = client_socket.recv_from(&mut buffer) {
            if client.is_none() {
                client = Some(from);
            }
            if client != Some(from) {
                continue;
            }
            if drop_packet(&mut rng, &config, &stats) {
                continue;
            }
            queue.push(Scheduled {
                due: now + delay(&mut rng, &config),
                to_server: true,
                data: buffer[..length].to_vec(),
            });
        }
        // Server -> client.
        while let Ok((length, from)) = server_socket.recv_from(&mut buffer) {
            if from != upstream {
                continue;
            }
            if drop_packet(&mut rng, &config, &stats) {
                continue;
            }
            queue.push(Scheduled {
                due: now + delay(&mut rng, &config),
                to_server: false,
                data: buffer[..length].to_vec(),
            });
        }
        queue.sort_unstable_by_key(|packet| packet.due);
        let mut index = 0;
        while index < queue.len() {
            if queue[index].due > now {
                index += 1;
                continue;
            }
            let packet = queue.swap_remove(index);
            let sent = if packet.to_server {
                server_socket.send_to(&packet.data, upstream)
            } else if let Some(client) = client {
                client_socket.send_to(&packet.data, client)
            } else {
                Ok(0)
            };
            if sent.is_ok() {
                stats.forwarded.fetch_add(1, Ordering::Relaxed);
                stats
                    .bytes
                    .fetch_add(packet.data.len() as u64, Ordering::Relaxed);
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn drop_packet(rng: &mut Rng, config: &LinkConfig, stats: &LinkStats) -> bool {
    if config.loss_percent > 0 && rng.gen_range_u32(100) < u32::from(config.loss_percent.min(100)) {
        stats.dropped.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

fn delay(rng: &mut Rng, config: &LinkConfig) -> Duration {
    let jitter_ms = u32::try_from(config.jitter.as_millis()).unwrap_or(u32::MAX);
    let extra = if jitter_ms == 0 {
        0
    } else {
        rng.gen_range_u32(jitter_ms + 1)
    };
    config.latency + Duration::from_millis(u64::from(extra))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_delays_and_drops() {
        let upstream = UdpSocket::bind("127.0.0.1:0").unwrap();
        upstream.set_nonblocking(true).unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let proxy = LinkProxy::start(
            "127.0.0.1:0".parse().unwrap(),
            upstream_addr,
            LinkConfig {
                latency: Duration::from_millis(40),
                jitter: Duration::ZERO,
                loss_percent: 0,
                seed: 1,
            },
        )
        .unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.set_nonblocking(true).unwrap();
        let start = Instant::now();
        client.send_to(b"hello", proxy.local_addr()).unwrap();
        let mut buffer = [0u8; 16];
        let mut received = None;
        for _ in 0..200 {
            if let Ok((length, _)) = upstream.recv_from(&mut buffer) {
                received = Some(buffer[..length].to_vec());
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(received.as_deref(), Some(b"hello".as_slice()));
        assert!(start.elapsed() >= Duration::from_millis(35));
    }

    #[test]
    fn total_loss_drops_everything() {
        let upstream = UdpSocket::bind("127.0.0.1:0").unwrap();
        upstream.set_nonblocking(true).unwrap();
        let proxy = LinkProxy::start(
            "127.0.0.1:0".parse().unwrap(),
            upstream.local_addr().unwrap(),
            LinkConfig {
                latency: Duration::from_millis(1),
                jitter: Duration::ZERO,
                loss_percent: 100,
                seed: 2,
            },
        )
        .unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.send_to(b"lost", proxy.local_addr()).unwrap();
        thread::sleep(Duration::from_millis(30));
        let mut buffer = [0u8; 16];
        assert!(upstream.recv_from(&mut buffer).is_err());
        assert!(proxy.stats().dropped() >= 1);
    }
}
