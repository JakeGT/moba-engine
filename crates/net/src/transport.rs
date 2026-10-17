//! Non-blocking UDP transport with application-level fragmentation.

use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::protocol::{
    HEADER_LEN, Header, MAX_DATAGRAM, MAX_PAYLOAD, PacketKind, decode_packet, encode_packet,
};

/// Fragment payload overhead: message id, index, count, inner kind, inner seq.
pub const FRAGMENT_HEADER_LEN: usize = 2 + 2 + 2 + 1 + 4;
/// Maximum payload carried by one fragment datagram.
pub const MAX_FRAGMENT_CHUNK: usize = MAX_PAYLOAD - FRAGMENT_HEADER_LEN;
/// Maximum fragments in one message.
pub const MAX_FRAGMENTS: usize = 255;
/// Largest message the transport will reassemble.
pub const MAX_MESSAGE: usize = MAX_FRAGMENT_CHUNK * MAX_FRAGMENTS;

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

/// Non-blocking UDP endpoint.
#[derive(Debug)]
pub struct UdpTransport {
    socket: UdpSocket,
    buffer: Vec<u8>,
    fragment_id: u16,
    assemblies: Vec<Assembly>,
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

    /// Sends one logical message, fragmenting it when necessary.
    ///
    /// # Errors
    /// Returns the first OS send error; a partially sent fragmented message is dropped by
    /// the receiver when it times out, and the next snapshot supersedes it.
    pub fn send(&mut self, addr: SocketAddr, header: &Header, payload: &[u8]) -> io::Result<()> {
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
            self.socket.send_to(&datagram, addr)?;
            self.bytes_sent += datagram.len() as u64;
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
            self.socket.send_to(&datagram, addr)?;
            self.bytes_sent += datagram.len() as u64;
        }
        Ok(())
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
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                self.expire_assemblies();
                return Ok(None);
            }
        }
    }

    fn handle_datagram(&mut self, addr: SocketAddr, length: usize) -> Option<ReceivedMessage> {
        let (header, payload) = decode_packet(&self.buffer[..length]).ok()?;
        if header.kind != PacketKind::Fragment {
            return Some(ReceivedMessage {
                addr,
                header,
                payload: payload.to_vec(),
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
        Some(ReceivedMessage {
            addr,
            header: Header {
                kind: completed.kind,
                flags: completed.flags,
                session: completed.session,
                seq: completed.seq,
                ack: header.ack,
                ack_bits: header.ack_bits,
            },
            payload,
        })
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
            Some(latest) if sequence > latest && sequence - latest < 32 => {
                let shift = sequence - latest;
                self.bits = (self.bits << shift) | (1 << (shift - 1));
                self.latest = Some(sequence);
            }
            Some(latest) if sequence > latest => {
                self.bits = 0;
                self.latest = Some(sequence);
            }
            Some(latest) if latest - sequence > 0 && latest - sequence <= 32 => {
                self.bits |= 1 << (latest - sequence - 1);
            }
            _ => {}
        }
    }

    /// Returns `true` when `sequence` was recorded (or is the latest).
    #[must_use]
    pub fn contains(&self, sequence: u32) -> bool {
        match self.latest {
            Some(latest) if sequence == latest => true,
            Some(latest) if latest > sequence && latest - sequence <= 32 => {
                self.bits & (1 << (latest - sequence - 1)) != 0
            }
            _ => false,
        }
    }
}

/// Small FIFO used for fixed-capacity event queues.
pub type EventQueue<T> = VecDeque<T>;

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
}
