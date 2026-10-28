//! Stale protocol clients receive an explicit disconnect instead of being ignored.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use moba_net::protocol::{
    Header, Hello, PROTOCOL_VERSION, PacketKind, decode_packet, encode_hello, encode_packet,
};
use moba_net::{NetServer, ServerConfig};

#[test]
fn mismatched_protocol_is_rejected_with_disconnect() {
    let server = NetServer::bind(
        ServerConfig::default().with_bind("127.0.0.1:0".parse().expect("bind address")),
    )
    .expect("bind");
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        let mut server = server;
        server
            .run_for(Duration::from_millis(800))
            .expect("server runs");
    });
    let socket = UdpSocket::bind("127.0.0.1:0").expect("socket");
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("timeout");
    let hello = encode_hello(&Hello {
        protocol: u16::from(PROTOCOL_VERSION).saturating_sub(1),
        nonce: 7,
        reconnect: 0,
        auth: [0; 32],
        spectate: false,
        identity: false,
        client_public: [0; 32],
        client_signature: [0; 64],
        server_challenge: 0,
    });
    let header = Header {
        kind: PacketKind::Hello,
        flags: 0,
        session: 0,
        seq: 1,
        ack: 0,
        ack_bits: 0,
    };
    let mut packet = Vec::new();
    encode_packet(&header, &hello, &mut packet).expect("encode hello");
    socket.send_to(&packet, addr).expect("send hello");

    let mut buffer = [0u8; 1_200];
    let start = Instant::now();
    let mut rejected = false;
    while start.elapsed() < Duration::from_secs(2) {
        let Ok((length, _)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        let Ok((header, payload)) = decode_packet(&buffer[..length]) else {
            continue;
        };
        if header.kind == PacketKind::Disconnect {
            assert_eq!(payload.first(), Some(&1), "protocol rejection reason");
            rejected = true;
            break;
        }
    }
    assert!(rejected, "server sent a protocol rejection");
    handle.join().expect("server thread");
}
