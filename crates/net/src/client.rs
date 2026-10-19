//! UDP client: handshake, intent submission and snapshot history.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moba_game::Team;

use crate::crypto::{
    SessionCrypto, derive_session_key, hello_tag, random_nonce, verify_server_welcome,
    verify_welcome_tag,
};
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
    /// A presentation event from a snapshot.
    GameEvent(crate::protocol::NetEvent),
    /// The catalogue was atomically replaced between matches.
    ContentUpdated {
        /// New content hash.
        hash: u64,
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

/// Client identity seed used to answer a server challenge.
struct ClientIdentity {
    seed: [u8; 32],
    public: [u8; 32],
}

impl core::fmt::Debug for ClientIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ClientIdentity")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

/// Options for a handshake-capable client connection.
#[derive(Clone, Default)]
pub struct ClientAuth {
    /// Optional pre-shared/token key.
    pub pre_shared_key: Option<[u8; 32]>,
    /// Optional pinned server public key.
    pub expected_server: Option<[u8; 32]>,
    /// Optional Ed25519 client identity seed.
    pub identity_seed: Option<[u8; 32]>,
    /// Connect as a read-only spectator.
    pub spectator: bool,
}

impl core::fmt::Debug for ClientAuth {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ClientAuth")
            .field("pre_shared_key", &self.pre_shared_key.is_some())
            .field("expected_server", &self.expected_server)
            .field("identity_seed", &self.identity_seed.is_some())
            .field("spectator", &self.spectator)
            .finish()
    }
}

/// A connected game client.
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
    items: Vec<crate::protocol::WireItem>,
    heroes: Vec<crate::protocol::WireHero>,
    abilities: Vec<crate::protocol::WireAbility>,
    map: crate::protocol::WireMap,
    rules: crate::protocol::WireRules,
    content_hash: u64,
    nonce: u64,
    pre_shared_key: Option<[u8; 32]>,
    expected_server: Option<[u8; 32]>,
    identity: Option<ClientIdentity>,
    challenge: u64,
    server_public_key: Option<[u8; 32]>,
    spectator: bool,
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

impl core::fmt::Debug for NetClient {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NetClient")
            .field("server", &self.server)
            .field("state", &self.state)
            .field("slot", &self.slot)
            .field("session", &self.session)
            .field("encrypted", &self.pre_shared_key.is_some())
            .field("spectator", &self.spectator)
            .finish_non_exhaustive()
    }
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
        Self::start_with_options(server, session, None, None, None, false)
    }

    /// Starts a read-only spectator handshake.
    ///
    /// # Errors
    /// Returns an OS error when the socket cannot be bound or randomness fails.
    pub fn start_spectator(server: SocketAddr) -> io::Result<Self> {
        Self::start_with_options(server, 0, None, None, None, true)
    }

    /// Binds a new socket and starts an authenticated handshake with a pre-shared key.
    ///
    /// # Errors
    /// Returns an OS error when the socket cannot be bound or randomness fails.
    pub fn start_with_key(server: SocketAddr, key: [u8; 32]) -> io::Result<Self> {
        Self::start_with_options(server, 0, Some(key), None, None, false)
    }

    /// Starts an authenticated reconnect handshake.
    ///
    /// # Errors
    /// Returns an OS error when the socket cannot be bound or randomness fails.
    pub fn start_with_session_key(
        server: SocketAddr,
        session: u32,
        key: [u8; 32],
    ) -> io::Result<Self> {
        Self::start_with_options(server, session, Some(key), None, None, false)
    }

    fn start_with_options(
        server: SocketAddr,
        session: u32,
        pre_shared_key: Option<[u8; 32]>,
        expected_server: Option<[u8; 32]>,
        identity_seed: Option<[u8; 32]>,
        spectator: bool,
    ) -> io::Result<Self> {
        let transport = UdpTransport::bind("0.0.0.0:0".parse().expect("valid bind address"))?;
        let now = Instant::now();
        let nonce = random_nonce()?;
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
            items: Vec::new(),
            heroes: Vec::new(),
            abilities: Vec::new(),
            map: crate::protocol::WireMap::default(),
            rules: crate::protocol::WireRules::default(),
            content_hash: 0,
            nonce,
            pre_shared_key,
            expected_server,
            identity: identity_seed.map(|seed| ClientIdentity {
                seed,
                public: crate::crypto::identity_public_key(&seed),
            }),
            challenge: 0,
            server_public_key: None,
            spectator,
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
        Self::connect_with_session_key(server, timeout, session, None)
    }

    /// Connects with an authenticated, encrypted pre-shared-key handshake.
    ///
    /// # Errors
    /// Returns `TimedOut` or `ConnectionAborted` when the server rejects the handshake.
    pub fn connect_with_key(
        server: SocketAddr,
        timeout: Duration,
        key: [u8; 32],
    ) -> io::Result<Self> {
        Self::connect_with_session_key(server, timeout, 0, Some(key))
    }

    /// Connects with a pinned Ed25519 server public key.
    ///
    /// # Errors
    /// Returns `TimedOut`/`ConnectionAborted` when the server cannot be authenticated.
    pub fn connect_pinned(
        server: SocketAddr,
        timeout: Duration,
        public_key: [u8; 32],
    ) -> io::Result<Self> {
        Self::connect_with_options(server, timeout, 0, None, Some(public_key), None, false)
    }

    /// Connects with a pre-shared key and a pinned server public key.
    ///
    /// # Errors
    /// Returns `TimedOut`/`ConnectionAborted` when authentication fails.
    pub fn connect_with_key_pinned(
        server: SocketAddr,
        timeout: Duration,
        key: [u8; 32],
        public_key: [u8; 32],
    ) -> io::Result<Self> {
        Self::connect_with_options(server, timeout, 0, Some(key), Some(public_key), None, false)
    }

    /// Connects as a pinned spectator with a pre-shared key.
    ///
    /// # Errors
    /// Returns `TimedOut`/`ConnectionAborted` when authentication fails.
    pub fn connect_spectator_with_key_pinned(
        server: SocketAddr,
        timeout: Duration,
        key: [u8; 32],
        public_key: [u8; 32],
    ) -> io::Result<Self> {
        Self::connect_with_options(server, timeout, 0, Some(key), Some(public_key), None, true)
    }

    /// Connects as a pinned spectator.
    ///
    /// # Errors
    /// Returns `TimedOut`/`ConnectionAborted` when authentication fails.
    pub fn connect_spectator_pinned(
        server: SocketAddr,
        timeout: Duration,
        public_key: [u8; 32],
    ) -> io::Result<Self> {
        Self::connect_with_options(server, timeout, 0, None, Some(public_key), None, true)
    }

    /// Connects as a read-only spectator (no slot or input).
    ///
    /// # Errors
    /// Returns `TimedOut` or `ConnectionAborted` when the server rejects the handshake.
    pub fn connect_spectator(server: SocketAddr, timeout: Duration) -> io::Result<Self> {
        Self::connect_with_options(server, timeout, 0, None, None, None, true)
    }

    /// Resumes a spectator session token.
    ///
    /// # Errors
    /// Returns `TimedOut` or `ConnectionAborted` when the server rejects the handshake.
    pub fn connect_spectator_with_session(
        server: SocketAddr,
        timeout: Duration,
        session: u32,
    ) -> io::Result<Self> {
        Self::connect_with_options(server, timeout, session, None, None, None, true)
    }

    /// Resumes a spectator session token with an authenticated handshake.
    ///
    /// # Errors
    /// Returns `TimedOut` or `ConnectionAborted` when the server rejects the handshake.
    pub fn connect_spectator_with_session_key(
        server: SocketAddr,
        timeout: Duration,
        session: u32,
        key: [u8; 32],
    ) -> io::Result<Self> {
        Self::connect_with_options(server, timeout, session, Some(key), None, None, true)
    }

    /// Connects as a read-only spectator with an encrypted handshake.
    ///
    /// # Errors
    /// Returns `TimedOut` or `ConnectionAborted` when the server rejects the handshake.
    pub fn connect_spectator_with_key(
        server: SocketAddr,
        timeout: Duration,
        key: [u8; 32],
    ) -> io::Result<Self> {
        Self::connect_with_options(server, timeout, 0, Some(key), None, None, true)
    }

    /// Resumes a session with an authenticated, encrypted handshake.
    ///
    /// # Errors
    /// Returns `TimedOut` or `ConnectionAborted` when the server rejects the handshake.
    pub fn connect_with_session_key(
        server: SocketAddr,
        timeout: Duration,
        session: u32,
        key: Option<[u8; 32]>,
    ) -> io::Result<Self> {
        Self::connect_with_options(server, timeout, session, key, None, None, false)
    }

    /// Connects using [`ClientAuth`] options (token, pinned server, client identity).
    ///
    /// # Errors
    /// Returns `TimedOut`/`ConnectionAborted` when the handshake fails.
    #[allow(clippy::needless_pass_by_value)] // A by-value options struct is the ergonomic API.
    pub fn connect_with_auth(
        server: SocketAddr,
        timeout: Duration,
        auth: ClientAuth,
    ) -> io::Result<Self> {
        Self::connect_with_options(
            server,
            timeout,
            0,
            auth.pre_shared_key,
            auth.expected_server,
            auth.identity_seed,
            auth.spectator,
        )
    }

    fn connect_with_options(
        server: SocketAddr,
        timeout: Duration,
        session: u32,
        key: Option<[u8; 32]>,
        expected_server: Option<[u8; 32]>,
        identity_seed: Option<[u8; 32]>,
        spectator: bool,
    ) -> io::Result<Self> {
        let mut client = Self::start_with_options(
            server,
            session,
            key,
            expected_server,
            identity_seed,
            spectator,
        )?;
        let start = Instant::now();
        let mut next_hello = Instant::now();
        while start.elapsed() < timeout {
            client.poll()?;
            if client.state == ClientState::Connected {
                return Ok(client);
            }
            if client.state == ClientState::Disconnected {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "server rejected the handshake",
                ));
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
    #[allow(clippy::too_many_lines)] // One flat packet dispatch is easier to audit.
    pub fn poll(&mut self) -> io::Result<()> {
        for _ in 0..256 {
            let Some(message) = self.transport.recv()? else {
                break;
            };
            if message.addr != self.server {
                continue;
            }
            match message.header.kind {
                PacketKind::Challenge => {
                    if self.state == ClientState::Connecting && message.payload.len() == 8 {
                        let challenge = u64::from_le_bytes(
                            message.payload[..8].try_into().expect("length checked"),
                        );
                        self.challenge = challenge;
                        self.send_hello()?;
                    }
                }
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
                    let Ok(content_hash) = welcome.catalog.content_hash() else {
                        continue;
                    };
                    if content_hash != welcome.content_hash {
                        continue;
                    }
                    let catalog = &welcome.catalog;
                    let item_count =
                        usize::from(catalog.item_count).min(crate::protocol::MAX_WIRE_ITEMS);
                    let hero_count =
                        usize::from(catalog.hero_count).min(crate::protocol::MAX_WIRE_HEROES);
                    let ability_count =
                        usize::from(catalog.ability_count).min(crate::protocol::MAX_WIRE_ABILITIES);
                    if let Some(pinned) = self.expected_server {
                        if !welcome.identity
                            || welcome.server_public_key != pinned
                            || !verify_server_welcome(
                                &pinned,
                                u16::from(crate::protocol::PROTOCOL_VERSION),
                                self.nonce,
                                welcome.server_nonce,
                                message.header.session,
                                welcome.slot,
                                content_hash,
                                &welcome.server_signature,
                            )
                        {
                            self.state = ClientState::Disconnected;
                            self.push_event(ClientEvent::Disconnected);
                            continue;
                        }
                    }
                    self.server_public_key = welcome.identity.then_some(welcome.server_public_key);
                    if let Some(key) = self.pre_shared_key {
                        if !verify_welcome_tag(
                            &key,
                            self.nonce,
                            welcome.server_nonce,
                            message.header.session,
                            welcome.slot,
                            content_hash,
                            &welcome.auth,
                        ) {
                            self.state = ClientState::Disconnected;
                            self.push_event(ClientEvent::Disconnected);
                            continue;
                        }
                        let session_key =
                            derive_session_key(&key, self.nonce, welcome.server_nonce);
                        self.transport.set_peer_key(
                            self.server,
                            SessionCrypto::new(session_key, message.header.session, 0),
                        );
                    }
                    self.session = message.header.session;
                    self.slot = welcome.slot;
                    self.spectator = welcome.spectator;
                    let team_size = usize::from(catalog.rules.team_size);
                    self.team = if usize::from(welcome.slot) < team_size {
                        Team::Blue
                    } else {
                        Team::Red
                    };
                    self.tick_rate = welcome.tick_rate;
                    self.snapshot_interval = welcome.snapshot_interval;
                    self.seed = welcome.seed;
                    self.hero_pick = welcome.hero_pick;
                    self.items.clear();
                    self.items.extend_from_slice(&catalog.items[..item_count]);
                    self.heroes.clear();
                    self.heroes.extend_from_slice(&catalog.heroes[..hero_count]);
                    self.abilities.clear();
                    self.abilities
                        .extend_from_slice(&catalog.abilities[..ability_count]);
                    self.map = catalog.map;
                    self.rules = catalog.rules;
                    self.content_hash = content_hash;
                    self.state = ClientState::Connected;
                    self.last_received = Instant::now();
                    self.push_event(ClientEvent::Connected { slot: welcome.slot });
                }
                PacketKind::ContentUpdate => {
                    if self.state != ClientState::Connected
                        || message.header.session != self.session
                    {
                        continue;
                    }
                    let Ok((catalog, hash)) =
                        crate::protocol::decode_content_update(&message.payload)
                    else {
                        continue;
                    };
                    if hash == self.content_hash {
                        continue; // duplicate/replayed catalogue; keep prediction state
                    }
                    if self.latest.as_ref().is_some_and(|snapshot| {
                        crate::protocol::MatchPhase::from_u8(snapshot.phase)
                            == Some(crate::protocol::MatchPhase::Playing)
                    }) {
                        continue; // catalogue swaps are only valid between matches
                    }
                    let Ok(content) = catalog.to_content() else {
                        continue;
                    };
                    if content.validate().is_err() {
                        continue;
                    }
                    let item_count =
                        usize::from(catalog.item_count).min(crate::protocol::MAX_WIRE_ITEMS);
                    let hero_count =
                        usize::from(catalog.hero_count).min(crate::protocol::MAX_WIRE_HEROES);
                    let ability_count =
                        usize::from(catalog.ability_count).min(crate::protocol::MAX_WIRE_ABILITIES);
                    self.items.clear();
                    self.items.extend_from_slice(&catalog.items[..item_count]);
                    self.heroes.clear();
                    self.heroes.extend_from_slice(&catalog.heroes[..hero_count]);
                    self.abilities.clear();
                    self.abilities
                        .extend_from_slice(&catalog.abilities[..ability_count]);
                    self.map = catalog.map;
                    self.rules = catalog.rules;
                    self.content_hash = hash;
                    self.latest = None;
                    self.history.clear();
                    self.last_acked_snapshot = 0;
                    self.push_event(ClientEvent::ContentUpdated { hash });
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
                    if self.state != ClientState::Disconnected {
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

    /// Whether this connection is a read-only spectator.
    #[must_use]
    pub const fn is_spectator(&self) -> bool {
        self.spectator
    }

    /// Client public key presented during the handshake, if any.
    #[must_use]
    pub fn client_public_key(&self) -> Option<[u8; 32]> {
        self.identity.as_ref().map(|identity| identity.public)
    }

    /// Authenticated server public key, when the server presented one.
    #[must_use]
    pub const fn server_public_key(&self) -> Option<[u8; 32]> {
        self.server_public_key
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

    /// Runtime item catalogue received in the handshake.
    #[must_use]
    pub fn items(&self) -> &[crate::protocol::WireItem] {
        &self.items
    }

    /// Runtime hero catalogue received in the handshake.
    #[must_use]
    pub fn heroes(&self) -> &[crate::protocol::WireHero] {
        &self.heroes
    }

    /// Runtime ability catalogue received in the handshake.
    #[must_use]
    pub fn abilities(&self) -> &[crate::protocol::WireAbility] {
        &self.abilities
    }

    /// Minimap geometry received in the handshake.
    #[must_use]
    pub const fn map(&self) -> &crate::protocol::WireMap {
        &self.map
    }

    /// Match rules received in the handshake or latest content update.
    #[must_use]
    pub const fn rules(&self) -> &crate::protocol::WireRules {
        &self.rules
    }

    /// Current content hash.
    #[must_use]
    pub const fn content_hash(&self) -> u64 {
        self.content_hash
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
        for event in &delta.events {
            self.push_event(ClientEvent::GameEvent(*event));
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
        let protocol = u16::from(crate::protocol::PROTOCOL_VERSION);
        let auth = self.pre_shared_key.map_or([0; 32], |key| {
            hello_tag(
                &key,
                protocol,
                self.nonce,
                self.reconnect_token,
                self.spectator,
            )
        });
        let (identity, client_public, client_signature) =
            self.identity
                .as_ref()
                .map_or((false, [0; 32], [0; 64]), |identity| {
                    (
                        true,
                        identity.public,
                        crate::crypto::sign_client_hello(
                            &identity.seed,
                            protocol,
                            self.nonce,
                            self.challenge,
                            self.reconnect_token,
                            self.spectator,
                        ),
                    )
                });
        let hello = crate::protocol::Hello {
            protocol,
            nonce: self.nonce,
            reconnect: self.reconnect_token,
            auth,
            spectate: self.spectator,
            identity,
            client_public,
            client_signature,
            server_challenge: self.challenge,
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
