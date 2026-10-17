//! UDP client: handshake, intent submission and snapshot history.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moba_game::Team;

use crate::protocol::{
    FLAG_NEED_FULL, Header, InputMessage, NetEntity, PacketKind, SnapshotDelta, SnapshotState,
    WireCommand, decode_hello, decode_packet, decode_snapshot, decode_welcome, encode_hello,
    encode_input, encode_packet, sequence_after,
};
use crate::transport::UdpTransport;

const HANDSHAKE_RETRY: Duration = Duration::from_millis(250);
const INPUT_RESEND: Duration = Duration::from_millis(33);
const PING_INTERVAL: Duration = Duration::from_secs(1);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
const HISTORY_LIMIT: usize = 64;

/// Client connection state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientState {
    /// Handshake in progress.
    Connecting,
    /// Welcome received.
    Connected,
    /// Disconnected (timeout or server request).
    Disconnected,
}

/// Observable client events.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientEvent {
    /// Handshake completed and a slot was assigned.
    Connected {
        /// Assigned player slot.
        slot: u8,
    },
    /// A snapshot was applied.
    Snapshot {
        /// Snapshot sequence.
        seq: u32,
        /// Server tick.
        tick: u32,
    },
    /// The connection ended.
    Disconnected,
}

/// Client counters for bandwidth/RTT tests.
#[derive(Copy, Clone, Debug, Default)]
pub struct ClientStats {
    /// UDP bytes received.
    pub bytes_received: u64,
    /// UDP bytes sent.
    pub bytes_sent: u64,
    /// Applied snapshots.
    pub snapshots: u64,
    /// Latest round-trip estimate.
    pub rtt_ms: Option<u32>,
    /// Connection state.
    pub state: Option<ClientState>,
}

/// A connected game client.
#[derive(Debug)]
pub struct NetClient {
    transport: UdpTransport,
    server: SocketAddr,
    state: ClientState,
    session: u32,
    slot: u8,
    team: Team,
    tick_rate: u16,
    snapshot_interval: u8,
    seed: u64,
    hero_pick: u8,
    nonce: u64,
    reconnect_token: u32,
    sequence: u32,
    input_seq: u32,
    pending: Option<InputMessage>,
    last_input_sent: Instant,
    last_acked_input: u32,
    latest: Option<SnapshotState>,
    history: VecDeque<SnapshotState>,
    last_acked_snapshot: u32,
    events: VecDeque<ClientEvent>,
    last_hello: Instant,
    last_received: Instant,
    last_ping: Instant,
    ping_sent: Option<Instant>,
    rtt_ms: Option<u32>,
    snapshots_received: u64,
}

impl NetClient {
    /// Binds a local socket and starts the handshake without blocking.
    ///
    /// # Errors
    /// Returns an OS error when the socket cannot be bound.
    pub fn start(server: SocketAddr) -> io::Result<Self> {
        Self::start_with_session(server, 0)
    }

    /// Binds a new socket and starts the handshake, requesting resumption of `session`.
    ///
    /// # Errors
    /// Returns an OS error when the socket cannot be bound.
    pub fn start_with_session(server: SocketAddr, session: u32) -> io::Result<Self> {
        let transport = UdpTransport::bind("0.0.0.0:0".parse().expect("valid bind address"))?;
        let now = Instant::now();
        let nonce = mint_nonce();
        let mut client = Self {
            transport,
            server,
            state: ClientState::Connecting,
            session: 0,
            slot: 0xFF,
            team: Team::Blue,
            tick_rate: 30,
            snapshot_interval: 1,
            seed: 0,
            hero_pick: 0,
            nonce,
            reconnect_token: session,
            sequence: 0,
            input_seq: 0,
            pending: None,
            last_input_sent: now,
            last_acked_input: 0,
            latest: None,
            history: VecDeque::with_capacity(HISTORY_LIMIT),
            last_acked_snapshot: 0,
            events: VecDeque::with_capacity(16),
            last_hello: now,
            last_received: now,
            last_ping: now,
            ping_sent: None,
            rtt_ms: None,
            snapshots_received: 0,
        };
        client.send_hello()?;
        Ok(client)
    }

    /// Connects and completes the handshake, retrying until `timeout`.
    ///
    /// # Errors
    /// Returns `TimedOut` if no welcome arrives, or a socket error.
    pub fn connect(server: SocketAddr, timeout: Duration) -> io::Result<Self> {
        Self::connect_with_session(server, timeout, 0)
    }

    /// Connects or resumes a previous session token.
    ///
    /// # Errors
    /// Returns `TimedOut` if no welcome arrives, or a socket error.
    pub fn connect_with_session(
        server: SocketAddr,
        timeout: Duration,
        session: u32,
    ) -> io::Result<Self> {
        let mut client = Self::start_with_session(server, session)?;
        let start = Instant::now();
        let mut next_hello = Instant::now();
        while start.elapsed() < timeout {
            client.poll()?;
            if client.state == ClientState::Connected {
                return Ok(client);
            }
            let now = Instant::now();
            if now >= next_hello {
                client.send_hello()?;
                next_hello = now + HANDSHAKE_RETRY;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "server did not answer the handshake",
        ))
    }

    /// Processes pending datagrams. Call every frame.
    ///
    /// # Errors
    /// Returns OS receive errors.
    pub fn poll(&mut self) -> io::Result<()> {
        for _ in 0..256 {
            let Some(message) = self.transport.recv()? else {
                break;
            };
            if message.addr != self.server {
                continue;
            }
            match message.header.kind {
                PacketKind::Welcome => {
                    if self.state != ClientState::Connecting {
                        continue;
                    }
                    let Ok(welcome) = decode_welcome(&message.payload) else {
                        continue;
                    };
                    if welcome.nonce != self.nonce {
                        continue;
                    }
                    self.session = message.header.session;
                    self.slot = welcome.slot;
                    self.team = if welcome.slot < 5 {
                        Team::Blue
                    } else {
                        Team::Red
                    };
                    self.tick_rate = welcome.tick_rate;
                    self.snapshot_interval = welcome.snapshot_interval;
                    self.seed = welcome.seed;
                    self.hero_pick = welcome.hero_pick;
                    self.state = ClientState::Connected;
                    self.last_received = Instant::now();
                    self.push_event(ClientEvent::Connected { slot: welcome.slot });
                }
                PacketKind::Snapshot => {
                    if self.state != ClientState::Connected
                        || message.header.session != self.session
                    {
                        continue;
                    }
                    self.last_received = Instant::now();
                    self.handle_snapshot(message.header, &message.payload);
                }
                PacketKind::Disconnect => {
                    if self.state == ClientState::Connected {
                        self.state = ClientState::Disconnected;
                        self.push_event(ClientEvent::Disconnected);
                    }
                }
                PacketKind::Pong => {
                    if let Some(sent) = self.ping_sent.take() {
                        self.rtt_ms =
                            Some(u32::try_from(sent.elapsed().as_millis()).unwrap_or(u32::MAX));
                    }
                }
                PacketKind::Ping if message.header.session == self.session => {
                    let header = Header {
                        kind: PacketKind::Pong,
                        flags: 0,
                        session: self.session,
                        seq: message.header.seq,
                        ack: 0,
                        ack_bits: 0,
                    };
                    let _ = self.transport.send(self.server, &header, &[]);
                }
                _ => {}
            }
        }
        if self.state == ClientState::Connected && self.last_received.elapsed() > CONNECTION_TIMEOUT
        {
            self.state = ClientState::Disconnected;
            self.push_event(ClientEvent::Disconnected);
        }
        Ok(())
    }

    /// Sends periodic keepalives and retransmits the pending command. Call every tick.
    ///
    /// # Errors
    /// Returns OS send errors.
    pub fn update(&mut self) -> io::Result<()> {
        if self.state != ClientState::Connected {
            return Ok(());
        }
        if let Some(pending) = &self.pending {
            if !sequence_after(pending.input_seq, self.last_acked_input) {
                self.pending = None;
            } else if self.last_input_sent.elapsed() >= INPUT_RESEND {
                self.send_input()?;
            }
        }
        if self.pending.is_none() && self.last_input_sent.elapsed() >= INPUT_RESEND {
            // Nothing to retransmit; keep the deadline fresh to avoid a burst later.
            self.last_input_sent = Instant::now();
        }
        if self.last_ping.elapsed() >= PING_INTERVAL {
            self.send_ping()?;
        }
        Ok(())
    }

    /// Queues and immediately transmits a command (retransmitted until acknowledged).
    ///
    /// # Errors
    /// Returns `NotConnected` before the handshake completes.
    pub fn send_command(&mut self, command: WireCommand) -> io::Result<()> {
        if self.state != ClientState::Connected {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "client handshake is not complete",
            ));
        }
        self.input_seq = self.input_seq.wrapping_add(1);
        self.pending = Some(InputMessage {
            client_tick: self.latest.as_ref().map_or(0, |state| state.tick),
            input_seq: self.input_seq,
            command,
        });
        self.send_input()
    }

    /// Requests a clean disconnect.
    ///
    /// # Errors
    /// Returns OS send errors.
    pub fn disconnect(&mut self) -> io::Result<()> {
        if self.state == ClientState::Connected {
            let header = Header {
                kind: PacketKind::Disconnect,
                flags: 0,
                session: self.session,
                seq: self.next_sequence(),
                ack: self.last_acked_snapshot,
                ack_bits: 0,
            };
            let _ = self.transport.send(self.server, &header, &[]);
        }
        self.state = ClientState::Disconnected;
        Ok(())
    }

    /// Drops any pending input (for example after a rematch reset).
    pub fn clear_pending_input(&mut self) {
        self.pending = None;
    }

    /// Latest applied snapshot.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&SnapshotState> {
        self.latest.as_ref()
    }

    /// Connection state.
    #[must_use]
    pub const fn state(&self) -> ClientState {
        self.state
    }

    /// Assigned player slot (`0xFF` before connection).
    #[must_use]
    pub const fn player_slot(&self) -> u8 {
        self.slot
    }

    /// Session token usable with [`Self::connect_with_session`] after a disconnect.
    #[must_use]
    pub const fn session(&self) -> u32 {
        self.session
    }

    /// Assigned team.
    #[must_use]
    pub const fn team(&self) -> Team {
        self.team
    }

    /// Match seed announced by the server.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Hero pick announced by the server.
    #[must_use]
    pub const fn hero_pick(&self) -> u8 {
        self.hero_pick
    }

    /// Server tick rate.
    #[must_use]
    pub const fn tick_rate(&self) -> u16 {
        self.tick_rate
    }

    /// Snapshot interval in ticks.
    #[must_use]
    pub const fn snapshot_interval(&self) -> u8 {
        self.snapshot_interval
    }

    /// Pops one event.
    pub fn next_event(&mut self) -> Option<ClientEvent> {
        self.events.pop_front()
    }

    /// Counters.
    #[must_use]
    pub fn stats(&self) -> ClientStats {
        ClientStats {
            bytes_received: self.transport.bytes_received,
            bytes_sent: self.transport.bytes_sent,
            snapshots: self.snapshots_received,
            rtt_ms: self.rtt_ms,
            state: Some(self.state),
        }
    }

    /// Finds an entity in the latest snapshot.
    #[must_use]
    pub fn entity(&self, net_id: u16) -> Option<&NetEntity> {
        self.latest.as_ref().and_then(|state| state.get(net_id))
    }

    fn handle_snapshot(&mut self, header: Header, payload: &[u8]) {
        let Ok(delta) = decode_snapshot(payload) else {
            return;
        };
        let seq = header.seq;
        let applied = if delta.baseline_seq == 0 {
            Some(SnapshotState::from_full(seq, &delta))
        } else {
            self.history
                .iter()
                .find(|state| state.seq == delta.baseline_seq)
                .and_then(|baseline| baseline.apply(&delta, seq).ok())
        };
        let Some(state) = applied else {
            self.request_full();
            return;
        };
        if sequence_after(delta.acked_input, self.last_acked_input) {
            self.last_acked_input = delta.acked_input;
        }
        if let Some(pending) = &self.pending {
            if !sequence_after(pending.input_seq, self.last_acked_input) {
                self.pending = None;
            }
        }
        if self
            .latest
            .as_ref()
            .is_none_or(|latest| sequence_after(seq, latest.seq))
        {
            self.latest = Some(state.clone());
        }
        self.history.push_back(state);
        if self.history.len() > HISTORY_LIMIT {
            self.history.pop_front();
        }
        self.snapshots_received += 1;
        if sequence_after(seq, self.last_acked_snapshot) {
            self.last_acked_snapshot = seq;
            self.send_ack(seq, false);
        }
        self.push_event(ClientEvent::Snapshot {
            seq,
            tick: delta.tick,
        });
    }

    fn request_full(&mut self) {
        self.last_acked_snapshot = 0;
        self.send_ack(0, true);
    }

    fn send_ack(&mut self, seq: u32, need_full: bool) {
        let header = Header {
            kind: PacketKind::SnapshotAck,
            flags: if need_full { FLAG_NEED_FULL } else { 0 },
            session: self.session,
            seq: self.next_sequence(),
            ack: seq,
            ack_bits: 0,
        };
        let _ = self.transport.send(self.server, &header, &[]);
    }

    fn send_input(&mut self) -> io::Result<()> {
        let Some(message) = self.pending else {
            return Ok(());
        };
        let payload = encode_input(&message);
        let header = Header {
            kind: PacketKind::Input,
            flags: 0,
            session: self.session,
            seq: self.next_sequence(),
            ack: self.last_acked_snapshot,
            ack_bits: 0,
        };
        self.transport.send(self.server, &header, &payload)?;
        self.last_input_sent = Instant::now();
        Ok(())
    }

    fn send_hello(&mut self) -> io::Result<()> {
        let hello = crate::protocol::Hello {
            protocol: u16::from(crate::protocol::PROTOCOL_VERSION),
            nonce: self.nonce,
            reconnect: self.reconnect_token,
        };
        let payload = encode_hello(&hello);
        let header = Header {
            kind: PacketKind::Hello,
            flags: 0,
            session: 0,
            seq: self.next_sequence(),
            ack: 0,
            ack_bits: 0,
        };
        self.transport.send(self.server, &header, &payload)?;
        self.last_hello = Instant::now();
        Ok(())
    }

    fn send_ping(&mut self) -> io::Result<()> {
        let header = Header {
            kind: PacketKind::Ping,
            flags: 0,
            session: self.session,
            seq: self.next_sequence(),
            ack: self.last_acked_snapshot,
            ack_bits: 0,
        };
        self.transport.send(self.server, &header, &[])?;
        self.last_ping = Instant::now();
        self.ping_sent = Some(Instant::now());
        Ok(())
    }

    fn next_sequence(&mut self) -> u32 {
        self.sequence = self.sequence.wrapping_add(1);
        self.sequence
    }

    fn push_event(&mut self, event: ClientEvent) {
        if self.events.len() < 32 {
            self.events.push_back(event);
        }
    }
}

fn mint_nonce() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    nanos ^ 0x9E37_79B9_7F4A_7C15
}

/// Helper for tools/tests: encode a packet with an arbitrary payload.
///
/// # Errors
/// Returns a wire error when the payload is too large.
pub fn encode_raw(header: &Header, payload: &[u8]) -> Result<Vec<u8>, crate::protocol::WireError> {
    let mut out = Vec::new();
    encode_packet(header, payload, &mut out)?;
    Ok(out)
}

/// Helper for tools/tests: decode a datagram.
pub fn decode_raw(data: &[u8]) -> Result<(Header, Vec<u8>), crate::protocol::WireError> {
    let (header, payload) = decode_packet(data)?;
    Ok((header, payload.to_vec()))
}

/// Helper for tools/tests: decode a handshake response.
pub fn decode_welcome_payload(
    payload: &[u8],
) -> Result<crate::protocol::Welcome, crate::protocol::WireError> {
    decode_welcome(payload)
}

/// Helper for tools/tests: decode a handshake request.
pub fn decode_hello_payload(
    payload: &[u8],
) -> Result<crate::protocol::Hello, crate::protocol::WireError> {
    decode_hello(payload)
}

/// Helper for tools/tests: decode a snapshot payload.
pub fn decode_snapshot_payload(
    payload: &[u8],
) -> Result<SnapshotDelta, crate::protocol::WireError> {
    decode_snapshot(payload)
}
