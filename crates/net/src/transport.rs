//! Non-blocking UDP transport with application-level fragmentation.

use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::crypto::SessionCrypto;
use crate::protocol::{
    FLAG_ENCRYPTED, HEADER_LEN, Header, MAX_DATAGRAM, MAX_PAYLOAD, PacketKind, decode_packet,
    encode_packet, sequence_after,
};

/// Fragment payload overhead: message id, index, count, inner kind, inner seq.
pub const FRAGMENT_HEADER_LEN: usize = 2 + 2 + 2 + 1 + 4;
/// Maximum payload carried by one fragment datagram.
pub const MAX_FRAGMENT_CHUNK: usize = MAX_PAYLOAD - FRAGMENT_HEADER_LEN;
/// Maximum fragments in one message.
pub const MAX_FRAGMENTS: usize = 255;
/// Largest message the transport will reassemble.
pub const MAX_MESSAGE: usize = MAX_FRAGMENT_CHUNK * MAX_FRAGMENTS;

fn transient_udp_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused
    ) || matches!(error.raw_os_error(), Some(10054 | 10061))
}

/// A complete message received from a peer.
#[derive(Debug)]
pub struct ReceivedMessage {
    /// Sender address.
    pub addr: SocketAddr,
    /// Reconstructed header.
    pub header: Header,
    /// Reconstructed payload.
    pub payload: Vec<u8>,
}

#[derive(Debug)]
struct Assembly {
    addr: SocketAddr,
    message_id: u16,
    kind: PacketKind,
    seq: u32,
    flags: u16,
    session: u32,
    total: usize,
    parts: Vec<Option<Vec<u8>>>,
    received: usize,
    started: Instant,
}

/// Whether a packet kind may be plaintext before a session key is installed.
fn plaintext_handshake(kind: PacketKind) -> bool {
    matches!(
        kind,
        PacketKind::Hello | PacketKind::Welcome | PacketKind::Challenge
    )
}

/// Application packets are plaintext only for peers without a session key.
fn plaintext_allowed(has_key: bool, kind: PacketKind, flags: u16) -> bool {
    !has_key || plaintext_handshake(kind) || flags & FLAG_ENCRYPTED != 0
}

/// Non-blocking UDP endpoint.
#[derive(Debug)]
pub struct UdpTransport {
    socket: UdpSocket,
    buffer: Vec<u8>,
    fragment_id: u16,
    assemblies: Vec<Assembly>,
    peers: Vec<(SocketAddr, SessionCrypto)>,
    pub(crate) bytes_sent: u64,
    pub(crate) bytes_received: u64,
}

impl UdpTransport {
    /// Binds a non-blocking UDP socket.
    ///
    /// # Errors
    /// Returns the OS bind error on failure.
    pub fn bind(addr: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            buffer: vec![0; MAX_DATAGRAM],
            fragment_id: 0,
            assemblies: Vec::new(),
            peers: Vec::new(),
            bytes_sent: 0,
            bytes_received: 0,
        })
    }

    /// Local address.
    ///
    /// # Errors
    /// Returns the OS error if the address cannot be queried.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Installs the AEAD state for a peer. Subsequent non-handshake messages are sealed.
    pub fn set_peer_key(&mut self, addr: SocketAddr, crypto: SessionCrypto) {
        if let Some(existing) = self
            .peers
            .iter_mut()
            .find(|(peer, _)| *peer == addr)
            .map(|(_, crypto)| crypto)
        {
            *existing = crypto;
        } else {
            self.peers.push((addr, crypto));
        }
    }

    /// Removes the AEAD state for a peer.
    pub fn remove_peer_key(&mut self, addr: SocketAddr) {
        self.peers.retain(|(peer, _)| *peer != addr);
    }

    /// Sends one logical message, fragmenting it when necessary.
    ///
    /// When a peer key is installed the payload is AEAD-sealed and the encrypted flag is set.
    ///
    /// # Errors
    /// Returns the first OS send error; a partially sent fragmented message is dropped by
    /// the receiver when it times out, and the next snapshot supersedes it.
    pub fn send(&mut self, addr: SocketAddr, header: &Header, payload: &[u8]) -> io::Result<()> {
        if let Some(crypto) = self
            .peers
            .iter()
            .find(|(peer, _)| *peer == addr)
            .map(|(_, crypto)| crypto.clone())
        {
            let sealed_header = Header {
                flags: header.flags | FLAG_ENCRYPTED,
                ..*header
            };
            let sealed = crypto
                .seal(&sealed_header, payload)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            self.send_payload(addr, &sealed_header, &sealed)
        } else {
            self.send_payload(addr, header, payload)
        }
    }

    /// Sends a handshake message without encryption.
    ///
    /// # Errors
    /// Returns the first OS send error.
    pub fn send_plaintext(
        &mut self,
        addr: SocketAddr,
        header: &Header,
        payload: &[u8],
    ) -> io::Result<()> {
        self.send_payload(addr, header, payload)
    }

    fn send_payload(
        &mut self,
        addr: SocketAddr,
        header: &Header,
        payload: &[u8],
    ) -> io::Result<()> {
        if payload.len() > MAX_MESSAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "message exceeds transport maximum",
            ));
        }
        if payload.len() <= MAX_PAYLOAD {
            let mut datagram = Vec::with_capacity(HEADER_LEN + payload.len());
            encode_packet(header, payload, &mut datagram)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            if self.send_datagram(&datagram, addr)? {
                self.bytes_sent += datagram.len() as u64;
            }
            return Ok(());
        }

        let total = payload.len().div_ceil(MAX_FRAGMENT_CHUNK);
        let message_id = self.fragment_id;
        self.fragment_id = self.fragment_id.wrapping_add(1);
        let mut datagram = Vec::with_capacity(MAX_DATAGRAM);
        for (index, chunk) in payload.chunks(MAX_FRAGMENT_CHUNK).enumerate() {
            let mut fragment = Vec::with_capacity(FRAGMENT_HEADER_LEN + chunk.len());
            fragment.extend_from_slice(&message_id.to_le_bytes());
            fragment.extend_from_slice(
                &u16::try_from(index)
                    .expect("fragment count bounded by MAX_FRAGMENTS")
                    .to_le_bytes(),
            );
            fragment.extend_from_slice(
                &u16::try_from(total)
                    .expect("fragment count fits")
                    .to_le_bytes(),
            );
            fragment.push(header.kind as u8);
            fragment.extend_from_slice(&header.seq.to_le_bytes());
            fragment.extend_from_slice(chunk);
            let outer = Header {
                kind: PacketKind::Fragment,
                ..*header
            };
            encode_packet(&outer, &fragment, &mut datagram)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            if self.send_datagram(&datagram, addr)? {
                self.bytes_sent += datagram.len() as u64;
            }
        }
        Ok(())
    }

    fn send_datagram(&self, datagram: &[u8], addr: SocketAddr) -> io::Result<bool> {
        match self.socket.send_to(datagram, addr) {
            Ok(_) => Ok(true),
            Err(error) if transient_udp_error(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Receives one complete message, or `None` if no complete message is pending.
    ///
    /// Malformed datagrams are ignored. Fragment assemblies older than three seconds are
    /// discarded.
    ///
    /// # Errors
    /// Returns OS errors other than `WouldBlock`.
    pub fn recv(&mut self) -> io::Result<Option<ReceivedMessage>> {
        let deadline = Instant::now() + Duration::from_millis(2);
        loop {
            match self.socket.recv_from(&mut self.buffer) {
                Ok((length, addr)) => {
                    self.bytes_received += length as u64;
                    if let Some(message) = self.handle_datagram(addr, length) {
                        self.expire_assemblies();
                        return Ok(Some(message));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.expire_assemblies();
                    return Ok(None);
                }
                Err(error) if transient_udp_error(&error) => {
                    // Windows reports ICMP port-unreachable as ConnectionReset on UDP. It is
                    // harmless: keep draining other datagrams until the deadline.
                    self.expire_assemblies();
                }
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                self.expire_assemblies();
                return Ok(None);
            }
        }
    }

    fn peer_has_key(&self, addr: SocketAddr) -> bool {
        self.peers.iter().any(|(peer, _)| *peer == addr)
    }

    fn handle_datagram(&mut self, addr: SocketAddr, length: usize) -> Option<ReceivedMessage> {
        let (header, payload) = decode_packet(&self.buffer[..length]).ok()?;
        if !plaintext_allowed(self.peer_has_key(addr), header.kind, header.flags) {
            return None;
        }
        if header.kind != PacketKind::Fragment {
            let payload = if header.flags & FLAG_ENCRYPTED != 0 {
                self.decrypt(addr, &header, payload)?
            } else {
                payload.to_vec()
            };
            return Some(ReceivedMessage {
                addr,
                header,
                payload,
            });
        }
        if payload.len() < FRAGMENT_HEADER_LEN {
            return None;
        }
        let message_id = u16::from_le_bytes([payload[0], payload[1]]);
        let index = usize::from(u16::from_le_bytes([payload[2], payload[3]]));
        let total = usize::from(u16::from_le_bytes([payload[4], payload[5]]));
        let kind = PacketKind::from_u8(payload[6])?;
        let seq = u32::from_le_bytes([payload[7], payload[8], payload[9], payload[10]]);
        if total == 0 || total > MAX_FRAGMENTS || index >= total {
            return None;
        }
        let chunk = payload[FRAGMENT_HEADER_LEN..].to_vec();
        let position = self
            .assemblies
            .iter()
            .position(|assembly| assembly.addr == addr && assembly.message_id == message_id);
        let assembly = if let Some(position) = position {
            &mut self.assemblies[position]
        } else {
            if self.assemblies.len() >= 32 {
                return None;
            }
            self.assemblies.push(Assembly {
                addr,
                message_id,
                kind,
                seq,
                flags: header.flags,
                session: header.session,
                total,
                parts: vec![None; total],
                received: 0,
                started: Instant::now(),
            });
            self.assemblies.last_mut()?
        };
        if assembly.total != total || assembly.parts[index].is_some() {
            return None;
        }
        assembly.parts[index] = Some(chunk);
        assembly.received += 1;
        if assembly.received != assembly.total {
            return None;
        }
        let assembly = self
            .assemblies
            .iter()
            .position(|candidate| candidate.addr == addr && candidate.message_id == message_id)?;
        let completed = self.assemblies.swap_remove(assembly);
        let mut payload = Vec::new();
        for part in completed.parts {
            payload.extend_from_slice(part.as_ref()?);
        }
        let reassembled = Header {
            kind: completed.kind,
            flags: completed.flags,
            session: completed.session,
            seq: completed.seq,
            ack: header.ack,
            ack_bits: header.ack_bits,
        };
        if !plaintext_allowed(self.peer_has_key(addr), reassembled.kind, reassembled.flags) {
            return None;
        }
        let payload = if reassembled.flags & FLAG_ENCRYPTED != 0 {
            self.decrypt(addr, &reassembled, &payload)?
        } else {
            payload
        };
        Some(ReceivedMessage {
            addr,
            header: reassembled,
            payload,
        })
    }

    fn decrypt(&self, addr: SocketAddr, header: &Header, payload: &[u8]) -> Option<Vec<u8>> {
        let (_, crypto) = self.peers.iter().find(|(peer, _)| *peer == addr)?;
        crypto.open(header, payload).ok()
    }

    fn expire_assemblies(&mut self) {
        let now = Instant::now();
        self.assemblies
            .retain(|assembly| now.duration_since(assembly.started) < Duration::from_secs(3));
    }
}

/// A simple acknowledgment tracker for sequence numbers.
#[derive(Debug, Default)]
pub struct AckTracker {
    /// Highest received sequence, if any.
    pub latest: Option<u32>,
    /// Bitmask of the 32 preceding sequences.
    pub bits: u32,
}

impl AckTracker {
    /// Records a received sequence.
    pub fn record(&mut self, sequence: u32) {
        match self.latest {
            None => self.latest = Some(sequence),
            Some(latest) if sequence == latest => {}
            Some(latest) if sequence_after(sequence, latest) => {
                let shift = sequence.wrapping_sub(latest);
                if shift < 32 {
                    self.bits = (self.bits << shift) | (1 << (shift - 1));
                } else {
                    self.bits = 0;
                }
                self.latest = Some(sequence);
            }
            Some(latest) => {
                let back = latest.wrapping_sub(sequence);
                if back > 0 && back <= 32 {
                    self.bits |= 1 << (back - 1);
                }
            }
        }
    }

    /// Returns `true` when `sequence` was recorded (or is the latest).
    #[must_use]
    pub fn contains(&self, sequence: u32) -> bool {
        match self.latest {
            Some(latest) if sequence == latest => true,
            Some(latest) if !sequence_after(sequence, latest) => {
                let back = latest.wrapping_sub(sequence);
                back <= 32 && (self.bits & (1 << (back - 1))) != 0
            }
            _ => false,
        }
    }
}

/// Small FIFO used for fixed-capacity event queues.
pub type EventQueue<T> = VecDeque<T>;

#[cfg(test)]
mod transient_tests {
    use super::*;

    #[test]
    fn transient_udp_errors_are_ignored() {
        assert!(transient_udp_error(&io::Error::from(
            io::ErrorKind::ConnectionReset
        )));
        assert!(transient_udp_error(&io::Error::from(
            io::ErrorKind::ConnectionRefused
        )));
        assert!(transient_udp_error(&io::Error::from_raw_os_error(10054)));
        assert!(!transient_udp_error(&io::Error::from(
            io::ErrorKind::WouldBlock
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ack_tracker_handles_order_and_gaps() {
        let mut tracker = AckTracker::default();
        tracker.record(10);
        tracker.record(12);
        assert!(tracker.contains(12));
        assert!(tracker.contains(10));
        assert!(!tracker.contains(11));
        tracker.record(11);
        assert!(tracker.contains(11));
        tracker.record(100);
        assert!(tracker.contains(100));
        assert!(!tracker.contains(12), "far jump clears the window");
        // Wrap-around: latest is near u32::MAX and a small sequence follows it.
        let mut wrap = AckTracker::default();
        wrap.record(u32::MAX - 1);
        wrap.record(u32::MAX);
        wrap.record(0);
        wrap.record(1);
        assert!(wrap.contains(0));
        assert!(wrap.contains(1));
        assert!(wrap.contains(u32::MAX));
        assert!(!wrap.contains(5));
    }

    #[test]
    fn encrypted_messages_round_trip_and_wrong_key_is_dropped() {
        let key = [5u8; 32];
        let mut sender = UdpTransport::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut receiver = UdpTransport::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let sender_addr = sender.local_addr().unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        sender.set_peer_key(receiver_addr, SessionCrypto::new(key, 9, 0));
        receiver.set_peer_key(sender_addr, SessionCrypto::new(key, 9, 1));
        let header = Header {
            kind: PacketKind::Input,
            flags: 0,
            session: 9,
            seq: 1,
            ack: 0,
            ack_bits: 0,
        };
        let payload: Vec<u8> = (0..3_000u32).map(|value| (value % 251) as u8).collect();
        sender.send(receiver_addr, &header, &payload).unwrap();
        let mut received = None;
        for _ in 0..100 {
            if let Some(message) = receiver.recv().unwrap() {
                received = Some(message);
                break;
            }
            std::thread::yield_now();
        }
        let message = received.expect("encrypted message arrives");
        assert_eq!(message.payload, payload);
        assert!(message.header.flags & FLAG_ENCRYPTED != 0);

        let mut wrong = UdpTransport::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        wrong.set_peer_key(sender_addr, SessionCrypto::new([6u8; 32], 9, 1));
        assert!(wrong.recv().unwrap().is_none());
    }

    #[test]
    fn fragmented_messages_round_trip_over_loopback() {
        let mut sender = UdpTransport::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut receiver = UdpTransport::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = receiver.local_addr().unwrap();
        let payload: Vec<u8> = (0..5_000u32).map(|value| (value % 251) as u8).collect();
        let header = Header {
            kind: PacketKind::Snapshot,
            flags: 0,
            session: 7,
            seq: 9,
            ack: 3,
            ack_bits: 0,
        };
        sender.send(addr, &header, &payload).unwrap();
        let mut received = None;
        for _ in 0..100 {
            if let Some(message) = receiver.recv().unwrap() {
                received = Some(message);
                break;
            }
            std::thread::yield_now();
        }
        let message = received.expect("fragmented message arrives");
        assert_eq!(message.header.kind, PacketKind::Snapshot);
        assert_eq!(message.header.seq, 9);
        assert_eq!(message.payload, payload);
    }

    #[test]
    fn keyed_peers_reject_plaintext_application_packets() {
        assert!(plaintext_allowed(false, PacketKind::Snapshot, 0));
        assert!(plaintext_allowed(true, PacketKind::Hello, 0));
        assert!(plaintext_allowed(true, PacketKind::Welcome, 0));
        assert!(plaintext_allowed(true, PacketKind::Challenge, 0));
        assert!(!plaintext_allowed(true, PacketKind::Input, 0));
        assert!(!plaintext_allowed(true, PacketKind::Snapshot, 0));
        assert!(plaintext_allowed(
            true,
            PacketKind::Snapshot,
            FLAG_ENCRYPTED
        ));
    }
}
