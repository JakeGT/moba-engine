//! Reconnects and handshakes are bound to the credential and server that created them.
//!
//! - A disconnected player's slot can be resumed only with the token it was opened with.
//! - Session ids are random, not a guessable counter.
//! - A keyed Hello must answer this server's address-bound challenge, so a captured Hello
//!   is useless against another server sharing the same keys.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use moba_net::crypto::hello_tag;
use moba_net::protocol::{Hello, PROTOCOL_VERSION, PacketKind, decode_packet, encode_hello};
use moba_net::{Header, NetClient, NetServer, ServerConfig, encode_packet};
use moba_sim::SimConfig;

const TOKEN_A: [u8; 32] = [0x41; 32];
const TOKEN_B: [u8; 32] = [0x42; 32];

fn config(grace: Duration) -> ServerConfig {
    ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().unwrap())
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(0)
        .with_client_timeout(Duration::from_secs(1))
        .with_reconnect_grace(grace)
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(4)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false)
}

fn spawn(seconds: u64) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let mut server = NetServer::bind(config(Duration::from_secs(8))).expect("bind");
    server.add_client_key(TOKEN_A);
    server.add_client_key(TOKEN_B);
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server
            .run_for(Duration::from_secs(seconds))
            .expect("server");
    });
    (addr, handle)
}

fn settle(client: &mut NetClient) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        client.poll().expect("poll");
        client.update().expect("update");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn another_token_cannot_take_over_a_reserved_slot() {
    let (addr, server) = spawn(10);
    let mut owner =
        NetClient::connect_with_key(addr, Duration::from_secs(2), TOKEN_A).expect("owner");
    let slot = owner.player_slot();
    let session = owner.session();
    settle(&mut owner);
    drop(owner);
    // Past the client timeout: the slot is now reserved for reconnection.
    std::thread::sleep(Duration::from_millis(1_500));

    let hijack =
        NetClient::connect_with_session_key(addr, Duration::from_secs(1), session, Some(TOKEN_B));
    let error = hijack.expect_err("a different valid token must not resume someone else's session");
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::ConnectionAborted,
        "rejected outright, not merely timed out"
    );

    let resumed =
        NetClient::connect_with_session_key(addr, Duration::from_secs(2), session, Some(TOKEN_A))
            .expect("the owner's token resumes its slot");
    assert_eq!(resumed.player_slot(), slot);
    assert_eq!(resumed.session(), session);
    drop(resumed);
    server.join().expect("server thread");
}

#[test]
fn session_ids_are_not_sequential() {
    let (addr, server) = spawn(4);
    let sessions: Vec<u32> = (0..4)
        .map(|_| {
            NetClient::connect_with_key(addr, Duration::from_secs(2), TOKEN_A)
                .expect("connect")
                .session()
        })
        .collect();
    let (other_addr, other) = spawn(4);
    let other_session = NetClient::connect_with_key(other_addr, Duration::from_secs(2), TOKEN_A)
        .expect("connect")
        .session();
    for window in sessions.windows(2) {
        assert_ne!(
            window[1],
            window[0].wrapping_add(1),
            "sessions: {sessions:x?}"
        );
    }
    assert!(!sessions.contains(&other_session));
    assert!(sessions.iter().all(|session| *session != 0));
    server.join().expect("server");
    other.join().expect("server");
}

/// Sends one Hello from `socket` and returns the kind of the first reply, if any.
fn exchange(
    socket: &UdpSocket,
    server: SocketAddr,
    hello: &Hello,
) -> Option<(PacketKind, Vec<u8>)> {
    let header = Header {
        kind: PacketKind::Hello,
        flags: 0,
        session: 0,
        seq: 1,
        ack: 0,
        ack_bits: 0,
    };
    let mut datagram = Vec::new();
    encode_packet(&header, &encode_hello(hello), &mut datagram).expect("encode");
    socket.send_to(&datagram, server).expect("send");
    let mut buffer = [0u8; 2_048];
    let (length, _) = socket.recv_from(&mut buffer).ok()?;
    let (reply, payload) = decode_packet(&buffer[..length]).ok()?;
    Some((reply.kind, payload.to_vec()))
}

#[test]
fn a_hello_for_one_server_is_not_accepted_by_another() {
    let (first, first_server) = spawn(3);
    let (second, second_server) = spawn(3);
    let socket = UdpSocket::bind("127.0.0.1:0").expect("socket");
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("timeout");
    let protocol = u16::from(PROTOCOL_VERSION);
    let mut hello = Hello {
        protocol,
        nonce: 0x0BAD_5EED,
        reconnect: 0,
        auth: [0; 32],
        spectate: false,
        identity: false,
        client_public: [0; 32],
        client_signature: [0; 64],
        server_challenge: 0,
    };

    // Server one answers an unchallenged keyed Hello with a challenge, not a slot.
    let (kind, payload) = exchange(&socket, first, &hello).expect("challenge reply");
    assert_eq!(kind, PacketKind::Challenge);
    let challenge = u64::from_le_bytes(payload[..8].try_into().expect("8-byte challenge"));
    hello.server_challenge = challenge;
    hello.auth = hello_tag(&TOKEN_A, protocol, hello.nonce, challenge, 0, false);

    // The same authenticated Hello sent to server two only earns a fresh challenge.
    let (kind, _) = exchange(&socket, second, &hello).expect("reply from second server");
    assert_eq!(
        kind,
        PacketKind::Challenge,
        "a Hello bound to another server's challenge must not be welcomed"
    );

    // Server one accepts it, which proves the Hello itself was valid. The Welcome carries
    // the full catalogue, so it may arrive fragmented.
    let (kind, _) = exchange(&socket, first, &hello).expect("welcome");
    assert!(
        matches!(kind, PacketKind::Welcome | PacketKind::Fragment),
        "expected a Welcome, got {kind:?}"
    );
    first_server.join().expect("server");
    second_server.join().expect("server");
}
