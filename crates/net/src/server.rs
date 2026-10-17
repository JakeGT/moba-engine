//! Authoritative UDP server: handshake, player slots and snapshot broadcasting.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moba_sim::{ObservedUnit, PlayerCommand, SimConfig, Simulation, TickInputs};

use crate::crypto::{
    SessionCrypto, derive_session_key, random_nonce, verify_hello_tag, welcome_tag,
};
use crate::protocol::{
    FLAG_NEED_FULL, Header, InputMessage, MAX_DATAGRAM, MatchPhase, NetEntity, NetId, PacketKind,
    SnapshotDelta, SnapshotState, Welcome, WireCommand, build_delta, decode_hello, decode_input,
    dequantize_position, encode_snapshot, encode_welcome, sequence_after,
};
use crate::transport::{ReceivedMessage, UdpTransport};

/// How long a silent client is kept before it is dropped and its slot reverts to a bot.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
/// Slots remain reserved for this long after a disconnect so players can reconnect.
pub const RECONNECT_GRACE: Duration = Duration::from_secs(60);
/// Snapshot history per client, used as delta baselines.
const SNAPSHOT_HISTORY: usize = 64;
/// Server tick rate (must match the simulation).
pub const SERVER_TICK_RATE: u16 = 30;

/// Server configuration.
#[derive(Clone)]
pub struct ServerConfig {
    /// UDP bind address.
    pub bind: SocketAddr,
    /// Simulation configuration (bots fill empty slots).
    pub sim: SimConfig,
    /// Send a snapshot every N ticks.
    pub snapshot_interval: u8,
    /// Maximum connected clients.
    pub max_clients: usize,
    /// Ticks between first client connect and gameplay (0 starts immediately).
    pub countdown_ticks: u32,
    /// Ticks the results screen stays up before a rematch.
    pub results_ticks: u32,
    /// Silence before a client is treated as disconnected.
    pub client_timeout: Duration,
    /// How long a disconnected player's slot is reserved for reconnection.
    pub reconnect_grace: Duration,
    /// Optional pre-shared key; when set, handshakes are authenticated and payloads encrypted.
    pub pre_shared_key: Option<[u8; 32]>,
}

impl core::fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ServerConfig")
            .field("bind", &self.bind)
            .field("sim", &self.sim)
            .field("snapshot_interval", &self.snapshot_interval)
            .field("max_clients", &self.max_clients)
            .field("countdown_ticks", &self.countdown_ticks)
            .field("results_ticks", &self.results_ticks)
            .field("client_timeout", &self.client_timeout)
            .field("reconnect_grace", &self.reconnect_grace)
            .field("pre_shared_key", &self.pre_shared_key.is_some())
            .finish()
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:7777".parse().expect("valid default address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 90,
            results_ticks: 450,
            client_timeout: CLIENT_TIMEOUT,
            reconnect_grace: RECONNECT_GRACE,
            pre_shared_key: None,
        }
    }
}

/// Server counters for bandwidth and soak tests.
#[derive(Copy, Clone, Debug, Default)]
pub struct ServerStats {
    /// Connected clients.
    pub clients: usize,
    /// Simulation ticks executed.
    pub ticks: u64,
    /// Snapshots encoded and sent.
    pub snapshots_sent: u64,
    /// UDP bytes sent (including headers/fragments).
    pub bytes_sent: u64,
    /// UDP bytes received.
    pub bytes_received: u64,
}

#[derive(Debug)]
struct SnapshotRecord {
    seq: u32,
    entities: Vec<NetEntity>,
}

#[derive(Debug)]
struct ClientConn {
    addr: SocketAddr,
    session: u32,
    team: moba_game::Team,
    connected: bool,
    last_seen: Instant,
    disconnected_at: Option<Instant>,
    last_input_seq: u32,
    command: Option<WireCommand>,
    net_ids: Vec<(moba_ecs::Entity, NetId)>,
    next_net_id: NetId,
    snapshot_seq: u32,
    acked_snapshot: u32,
    need_full: bool,
    history: VecDeque<SnapshotRecord>,
    welcome_nonce: u64,
    welcome_payload: Vec<u8>,
    bytes_sent: u64,
}

impl ClientConn {
    fn net_id_for(&mut self, entity: moba_ecs::Entity) -> NetId {
        if let Some((_, id)) = self.net_ids.iter().find(|(known, _)| *known == entity) {
            return *id;
        }
        let id = self.next_net_id;
        self.next_net_id = self.next_net_id.wrapping_add(1);
        self.net_ids.push((entity, id));
        id
    }

    fn entity_for(&self, net_id: NetId) -> Option<moba_ecs::Entity> {
        self.net_ids
            .iter()
            .find(|(_, id)| *id == net_id)
            .map(|(entity, _)| *entity)
    }

    fn baseline(&self, seq: u32) -> Option<&[NetEntity]> {
        self.history
            .iter()
            .find(|record| record.seq == seq)
            .map(|record| record.entities.as_slice())
    }

    fn record(&mut self, seq: u32, entities: Vec<NetEntity>) {
        self.history.push_back(SnapshotRecord { seq, entities });
        if self.history.len() > SNAPSHOT_HISTORY {
            self.history.pop_front();
        }
    }
}

/// A running authoritative network server.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Lobby,
    Countdown(u32),
    Playing,
    Results(u32),
}

/// A running authoritative network server.
#[derive(Debug)]
pub struct NetServer {
    transport: UdpTransport,
    simulation: Simulation,
    config: ServerConfig,
    clients: [Option<ClientConn>; 10],
    observed: [Vec<ObservedUnit>; 2],
    current: Vec<NetEntity>,
    payload: Vec<u8>,
    session_counter: u32,
    stats: ServerStats,
    phase: Phase,
    frame: u64,
    match_number: u32,
}

impl NetServer {
    /// Binds the server socket and starts a match.
    ///
    /// # Errors
    /// Returns an OS error when the socket cannot be bound.
    pub fn bind(config: ServerConfig) -> io::Result<Self> {
        let transport = UdpTransport::bind(config.bind)?;
        let simulation = Simulation::new(config.sim);
        Ok(Self {
            transport,
            simulation,
            config,
            clients: std::array::from_fn(|_| None),
            observed: std::array::from_fn(|_| Vec::with_capacity(moba_sim::MAX_UNITS)),
            current: Vec::with_capacity(moba_sim::MAX_UNITS),
            payload: Vec::with_capacity(MAX_DATAGRAM),
            session_counter: 1,
            stats: ServerStats::default(),
            phase: Phase::Lobby,
            frame: 0,
            match_number: 0,
        })
    }

    /// Bound address.
    ///
    /// # Errors
    /// Returns the OS error if the local address cannot be queried.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.transport.local_addr()
    }

    /// Current simulation.
    #[must_use]
    pub const fn simulation(&self) -> &Simulation {
        &self.simulation
    }

    /// Current phase and remaining countdown/results ticks.
    #[must_use]
    pub fn phase(&self) -> (MatchPhase, u16) {
        match self.phase {
            Phase::Lobby => (MatchPhase::Lobby, 0),
            Phase::Countdown(remaining) => (
                MatchPhase::Countdown,
                u16::try_from(remaining).unwrap_or(u16::MAX),
            ),
            Phase::Playing => (MatchPhase::Playing, 0),
            Phase::Results(remaining) => (
                MatchPhase::Results,
                u16::try_from(remaining).unwrap_or(u16::MAX),
            ),
        }
    }

    /// Zero-based match number served by this process.
    #[must_use]
    pub const fn match_number(&self) -> u32 {
        self.match_number
    }

    /// Current counters.
    #[must_use]
    pub fn stats(&self) -> ServerStats {
        let mut stats = self.stats;
        stats.clients = self
            .clients
            .iter()
            .flatten()
            .filter(|client| client.connected)
            .count();
        stats.bytes_sent = self.transport.bytes_sent;
        stats.bytes_received = self.transport.bytes_received;
        stats
    }

    /// Processes all pending datagrams.
    ///
    /// # Errors
    /// Returns OS receive errors (apart from `WouldBlock`).
    pub fn poll(&mut self) -> io::Result<()> {
        for _ in 0..512 {
            let Some(message) = self.transport.recv()? else {
                break;
            };
            if let Err(error) = self.handle_message(message) {
                eprintln!("net: dropped message: {error}");
            }
        }
        self.expire_clients();
        Ok(())
    }

    /// Advances the match state machine and broadcasts snapshots.
    ///
    /// Phases: lobby (paused) → countdown (paused) → playing → results (paused) → rematch.
    ///
    /// # Errors
    /// Returns OS send errors from snapshot transmission.
    pub fn tick(&mut self) -> io::Result<()> {
        self.frame = self.frame.wrapping_add(1);
        match self.phase {
            Phase::Lobby => {}
            Phase::Countdown(remaining) => {
                self.phase = if remaining <= 1 {
                    Phase::Playing
                } else {
                    Phase::Countdown(remaining - 1)
                };
            }
            Phase::Playing => {
                self.tick_simulation();
                if self.simulation.outcome().is_some() {
                    self.phase = Phase::Results(self.config.results_ticks.max(1));
                }
            }
            Phase::Results(remaining) => {
                if remaining <= 1 {
                    self.reset_match();
                } else {
                    self.phase = Phase::Results(remaining - 1);
                }
            }
        }
        let interval = u64::from(self.config.snapshot_interval.max(1));
        if self.frame % interval == 0 {
            self.send_snapshots()?;
        }
        Ok(())
    }

    fn tick_simulation(&mut self) {
        let mut inputs: TickInputs = self.simulation.bot_inputs();
        for slot in 0..self.clients.len() {
            let Some(client) = self.clients[slot].as_mut() else {
                continue;
            };
            let Some(wire) = client.command.take() else {
                continue;
            };
            if let Some(command) = wire_to_command(client, wire) {
                inputs.set(slot, command);
            }
        }
        self.simulation.tick(&inputs);
        self.simulation.clear_events();
        self.stats.ticks += 1;
    }

    fn start_countdown(&mut self) {
        self.phase = if self.config.countdown_ticks == 0 {
            Phase::Playing
        } else {
            Phase::Countdown(self.config.countdown_ticks)
        };
    }

    fn reset_match(&mut self) {
        self.match_number = self.match_number.wrapping_add(1);
        let mut config = self.config.sim;
        config.seed = config.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.config.sim = config;
        self.simulation = Simulation::new(config);
        for slot in 0..self.clients.len() {
            let Some(client) = self.clients[slot].as_mut() else {
                continue;
            };
            client.history.clear();
            client.net_ids.clear();
            client.next_net_id = 1;
            client.acked_snapshot = 0;
            client.need_full = true;
            client.command = None;
            client.last_input_seq = 0;
        }
        self.refresh_bot_mask();
        if self.clients.iter().any(Option::is_some) {
            self.start_countdown();
        } else {
            self.phase = Phase::Lobby;
        }
    }

    /// Runs a real-time 30 Hz loop for `duration`.
    ///
    /// # Errors
    /// Returns socket I/O errors.
    pub fn run_for(&mut self, duration: Duration) -> io::Result<()> {
        let tick_duration = Duration::from_secs_f64(1.0 / f64::from(SERVER_TICK_RATE));
        let start = Instant::now();
        let mut next_tick = Instant::now();
        while start.elapsed() < duration {
            self.poll()?;
            let now = Instant::now();
            if now >= next_tick {
                self.tick()?;
                next_tick += tick_duration;
                if next_tick < now {
                    next_tick = now + tick_duration;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    fn expire_clients(&mut self) {
        let now = Instant::now();
        for slot in 0..self.clients.len() {
            let timeout = self.config.client_timeout;
            let grace = self.config.reconnect_grace;
            let Some(client) = self.clients[slot].as_mut() else {
                continue;
            };
            if client.connected {
                if now.duration_since(client.last_seen) > timeout {
                    client.connected = false;
                    client.disconnected_at = Some(now);
                    client.command = None;
                    self.transport.remove_peer_key(client.addr);
                    let header = Header {
                        kind: PacketKind::Disconnect,
                        flags: 0,
                        session: client.session,
                        seq: 0,
                        ack: 0,
                        ack_bits: 0,
                    };
                    let _ = self.transport.send(client.addr, &header, &[1]);
                }
            } else if client
                .disconnected_at
                .is_some_and(|at| now.duration_since(at) > grace)
            {
                self.transport.remove_peer_key(client.addr);
                self.clients[slot] = None;
            }
        }
        self.refresh_bot_mask();
    }

    fn refresh_bot_mask(&mut self) {
        let mut mask = 0x03FFu16;
        for (slot, client) in self.clients.iter().enumerate() {
            if client.as_ref().is_some_and(|client| client.connected) {
                mask &= !(1 << slot);
            }
        }
        self.simulation.set_bot_mask(mask);
    }

    #[allow(clippy::too_many_lines)] // One match arm per protocol message.
    #[allow(clippy::needless_pass_by_value)] // The message owns its payload.
    fn handle_message(&mut self, message: ReceivedMessage) -> io::Result<()> {
        match message.header.kind {
            PacketKind::Hello => {
                let Ok(hello) = decode_hello(&message.payload) else {
                    return Ok(());
                };
                if hello.protocol != u16::from(crate::protocol::PROTOCOL_VERSION) {
                    return Ok(());
                }
                if let Some(key) = self.config.pre_shared_key {
                    if !verify_hello_tag(
                        &key,
                        hello.protocol,
                        hello.nonce,
                        hello.reconnect,
                        &hello.auth,
                    ) {
                        self.reject_client(message.addr, 0, 4)?;
                        return Ok(());
                    }
                }
                if let Some(slot) = self.client_index_by_addr(message.addr) {
                    let (connected, session) = {
                        let client = self.clients[slot].as_mut().expect("index is live");
                        if client.connected {
                            client.last_seen = Instant::now();
                        }
                        (client.connected, client.session)
                    };
                    if connected {
                        let cached = {
                            let client = self.clients[slot].as_ref().expect("index is live");
                            (client.welcome_nonce == hello.nonce
                                && !client.welcome_payload.is_empty())
                            .then(|| client.welcome_payload.clone())
                        };
                        if let Some(payload) = cached {
                            let header = Header {
                                kind: PacketKind::Welcome,
                                flags: 0,
                                session,
                                seq: 0,
                                ack: 0,
                                ack_bits: 0,
                            };
                            self.transport
                                .send_plaintext(message.addr, &header, &payload)?;
                        } else {
                            self.send_welcome(message.addr, slot, hello.nonce, session)?;
                        }
                        return Ok(());
                    }
                    if hello.reconnect != session {
                        self.reject_client(message.addr, session, 3)?;
                        return Ok(());
                    }
                    self.reconnect_client(slot, message.addr, hello.nonce)?;
                    return Ok(());
                }
                if hello.reconnect != 0 {
                    let Some(slot) = self.clients.iter().position(|client| {
                        client
                            .as_ref()
                            .is_some_and(|client| client.session == hello.reconnect)
                    }) else {
                        self.reject_client(message.addr, 0, 3)?;
                        return Ok(());
                    };
                    if self.clients[slot]
                        .as_ref()
                        .is_some_and(|client| client.connected)
                    {
                        self.reject_client(message.addr, 0, 3)?;
                        return Ok(());
                    }
                    self.reconnect_client(slot, message.addr, hello.nonce)?;
                    return Ok(());
                }
                let Some(slot) = self.free_slot() else {
                    self.reject_client(message.addr, 0, 2)?;
                    return Ok(());
                };
                let session = 0xA5A5_0000 | (self.session_counter & 0xFFFF);
                self.session_counter = self.session_counter.wrapping_add(1);
                let team = if slot < 5 {
                    moba_game::Team::Blue
                } else {
                    moba_game::Team::Red
                };
                self.clients[slot] = Some(ClientConn {
                    addr: message.addr,
                    session,
                    team,
                    connected: true,
                    last_seen: Instant::now(),
                    disconnected_at: None,
                    last_input_seq: 0,
                    command: None,
                    net_ids: Vec::new(),
                    next_net_id: 1,
                    snapshot_seq: 0,
                    acked_snapshot: 0,
                    need_full: true,
                    history: VecDeque::new(),
                    welcome_nonce: 0,
                    welcome_payload: Vec::new(),
                    bytes_sent: 0,
                });
                self.refresh_bot_mask();
                if self.phase == Phase::Lobby {
                    self.start_countdown();
                }
                self.send_welcome(message.addr, slot, hello.nonce, session)?;
            }
            PacketKind::Input => {
                let Some(index) = self.client_index_by_addr(message.addr) else {
                    return Ok(());
                };
                let client = self.clients[index].as_mut().expect("index is live");
                if message.header.session != client.session || !client.connected {
                    return Ok(());
                }
                let Ok(input) = decode_input(&message.payload) else {
                    return Ok(());
                };
                client.last_seen = Instant::now();
                let mut purchase = None;
                if sequence_after(input.input_seq, client.last_input_seq) {
                    client.last_input_seq = input.input_seq;
                    match input.command {
                        WireCommand::BuyItem { item } => purchase = Some(item),
                        command => client.command = Some(command),
                    }
                }
                if let Some(item) = purchase {
                    self.simulation.purchase(index, item);
                }
            }
            PacketKind::SnapshotAck => {
                let Some(index) = self.client_index_by_addr(message.addr) else {
                    return Ok(());
                };
                let client = self.clients[index].as_mut().expect("index is live");
                if message.header.session != client.session || !client.connected {
                    return Ok(());
                }
                client.last_seen = Instant::now();
                if message.header.flags & FLAG_NEED_FULL != 0 {
                    client.need_full = true;
                    client.acked_snapshot = 0;
                } else if sequence_after(message.header.ack, client.acked_snapshot) {
                    client.acked_snapshot = message.header.ack;
                }
            }
            PacketKind::Ping => {
                if let Some(index) = self.client_index_by_addr(message.addr) {
                    let client = self.clients[index].as_mut().expect("index is live");
                    if message.header.session == client.session && client.connected {
                        client.last_seen = Instant::now();
                        let header = Header {
                            kind: PacketKind::Pong,
                            flags: 0,
                            session: client.session,
                            seq: message.header.seq,
                            ack: 0,
                            ack_bits: 0,
                        };
                        self.transport.send(client.addr, &header, &[])?;
                    }
                }
            }
            PacketKind::Disconnect => {
                if let Some(index) = self.client_index_by_addr(message.addr) {
                    if let Some(client) = self.clients[index].as_mut() {
                        client.connected = false;
                        client.disconnected_at = Some(Instant::now());
                        client.command = None;
                        self.transport.remove_peer_key(client.addr);
                    }
                    self.refresh_bot_mask();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn reconnect_client(&mut self, slot: usize, addr: SocketAddr, nonce: u64) -> io::Result<()> {
        let Some(client) = self.clients[slot].as_mut() else {
            return Ok(());
        };
        let session = client.session;
        let old_addr = client.addr;
        self.transport.remove_peer_key(old_addr);
        client.addr = addr;
        client.connected = true;
        client.disconnected_at = None;
        client.last_seen = Instant::now();
        client.last_input_seq = 0;
        client.command = None;
        client.acked_snapshot = 0;
        client.need_full = true;
        client.history.clear();
        self.refresh_bot_mask();
        self.send_welcome(addr, slot, nonce, session)
    }

    fn reject_client(&mut self, addr: SocketAddr, session: u32, reason: u8) -> io::Result<()> {
        let header = Header {
            kind: PacketKind::Disconnect,
            flags: 0,
            session,
            seq: 0,
            ack: 0,
            ack_bits: 0,
        };
        self.transport.send(addr, &header, &[reason])
    }

    fn send_welcome(
        &mut self,
        addr: SocketAddr,
        slot: usize,
        nonce: u64,
        session: u32,
    ) -> io::Result<()> {
        let config = self.config.sim;
        let slot_u8 = u8::try_from(slot).expect("slot fits u8");
        let mut server_nonce = 0;
        let mut auth = [0u8; 32];
        let mut crypto = None;
        if let Some(key) = self.config.pre_shared_key {
            server_nonce = random_nonce()?;
            auth = welcome_tag(&key, nonce, server_nonce, session, slot_u8);
            let session_key = derive_session_key(&key, nonce, server_nonce);
            crypto = Some(SessionCrypto::new(session_key, session, 1));
        }
        let welcome = Welcome {
            slot: slot_u8,
            tick_rate: SERVER_TICK_RATE,
            snapshot_interval: self.config.snapshot_interval,
            tick: self.simulation.current_tick().get(),
            seed: config.seed,
            hero_pick: config.hero_picks[slot],
            nonce,
            server_nonce,
            auth,
        };
        let payload = encode_welcome(&welcome);
        if let Some(client) = self.clients[slot].as_mut() {
            client.welcome_nonce = nonce;
            client.welcome_payload.clone_from(&payload);
        }
        let header = Header {
            kind: PacketKind::Welcome,
            flags: 0,
            session,
            seq: 0,
            ack: 0,
            ack_bits: 0,
        };
        self.transport.send_plaintext(addr, &header, &payload)?;
        if let Some(crypto) = crypto {
            self.transport.set_peer_key(addr, crypto);
        }
        Ok(())
    }

    fn free_slot(&self) -> Option<usize> {
        (0..self.config.max_clients.min(self.clients.len()))
            .find(|slot| self.clients[*slot].is_none())
    }

    fn client_index_by_addr(&self, addr: SocketAddr) -> Option<usize> {
        self.clients
            .iter()
            .position(|client| client.as_ref().is_some_and(|client| client.addr == addr))
    }

    fn send_snapshots(&mut self) -> io::Result<()> {
        let mut observed = core::mem::take(&mut self.observed);
        for team in moba_game::Team::ALL {
            self.simulation
                .observed_units(team, &mut observed[team.index()]);
        }
        let tick = self.simulation.current_tick().get();
        let (phase_code, countdown) = self.phase();
        let mut current = core::mem::take(&mut self.current);
        let mut payload = core::mem::take(&mut self.payload);
        for index in 0..self.clients.len() {
            let Some(client) = self.clients[index].as_mut() else {
                continue;
            };
            if !client.connected {
                continue;
            }
            current.clear();
            let viewer_slot = u8::try_from(index).unwrap_or(0xFF);
            for unit in &observed[client.team.index()] {
                let mut entity = NetEntity::from_observed(client.net_id_for(unit.entity), unit);
                if unit.hero_slot != Some(viewer_slot) {
                    // Gold and inventory are private to the owning player.
                    entity.progress.gold = 0;
                    entity.progress.items = [moba_game::EMPTY_ITEM; 6];
                }
                current.push(entity);
            }
            current.sort_unstable_by_key(|entity| entity.net_id);
            let baseline = if client.need_full {
                None
            } else {
                client.baseline(client.acked_snapshot)
            };
            let baseline_seq = baseline.as_ref().map_or(0, |_| client.acked_snapshot);
            let mut delta: SnapshotDelta = build_delta(
                &current,
                baseline,
                tick,
                client.last_input_seq,
                baseline_seq,
            );
            let (blue_score, red_score) = self.simulation.score();
            delta.blue_score = blue_score;
            delta.red_score = red_score;
            delta.outcome = match self.simulation.outcome() {
                None => 0,
                Some(moba_game::Team::Blue) => 1,
                Some(moba_game::Team::Red) => 2,
            };
            delta.phase = phase_code as u8;
            delta.countdown = countdown;
            encode_snapshot(&delta, &mut payload)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            client.snapshot_seq = client.snapshot_seq.wrapping_add(1);
            let seq = client.snapshot_seq;
            let header = Header {
                kind: PacketKind::Snapshot,
                flags: 0,
                session: client.session,
                seq,
                ack: client.last_input_seq,
                ack_bits: 0,
            };
            self.transport.send(client.addr, &header, &payload)?;
            client.bytes_sent += payload.len() as u64;
            client.need_full = false;
            client.record(seq, current.clone());
            self.stats.snapshots_sent += 1;
        }
        self.observed = observed;
        self.current = current;
        self.payload = payload;
        Ok(())
    }
}

fn wire_to_command(client: &ClientConn, command: WireCommand) -> Option<PlayerCommand> {
    Some(match command {
        WireCommand::Stop => PlayerCommand::Stop,
        WireCommand::MoveTo { x, y } => PlayerCommand::MoveTo(dequantize_position(x, y)),
        WireCommand::Attack { net_id } => PlayerCommand::Attack(client.entity_for(net_id)?),
        WireCommand::CastAbility { slot, x, y } => PlayerCommand::CastAbility {
            slot,
            target: dequantize_position(x, y),
        },
        WireCommand::BuyItem { item } => PlayerCommand::BuyItem(item),
    })
}

/// Unused helper kept for tests/tools: decodes an input payload.
///
/// # Errors
/// Returns a wire error for malformed input.
pub fn decode_input_message(payload: &[u8]) -> Result<InputMessage, crate::protocol::WireError> {
    decode_input(payload)
}

/// Returns the state snapshot that would be built from a delta and a baseline (tooling).
pub fn apply_delta(
    baseline: &SnapshotState,
    delta: &SnapshotDelta,
    seq: u32,
) -> Result<SnapshotState, crate::protocol::WireError> {
    baseline.apply(delta, seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_sim::{Health, UnitKind};

    #[test]
    fn match_flow_transitions_and_resets() {
        let config = ServerConfig {
            bind: "127.0.0.1:0".parse().expect("address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 2,
            results_ticks: 3,
            client_timeout: Duration::from_millis(100),
            reconnect_grace: Duration::from_secs(2),
            pre_shared_key: None,
        };
        let mut server = NetServer::bind(config).expect("bind");
        assert_eq!(server.phase(), (MatchPhase::Lobby, 0));

        // A connecting client would start the countdown; drive the same transition directly.
        server.phase = Phase::Countdown(2);
        server.tick().expect("countdown");
        assert_eq!(server.phase(), (MatchPhase::Countdown, 1));
        server.tick().expect("playing");
        assert_eq!(server.phase(), (MatchPhase::Playing, 0));

        let nexus = server
            .simulation
            .world()
            .entities()
            .find(|entity| {
                server.simulation.world().get::<UnitKind>(*entity) == Some(&UnitKind::Nexus)
                    && server.simulation.world().get::<moba_game::Team>(*entity)
                        == Some(&moba_game::Team::Red)
            })
            .expect("red nexus");
        server
            .simulation
            .world_mut()
            .get_mut::<Health>(nexus)
            .expect("nexus health")
            .current = 0;
        server.tick().expect("result");
        assert_eq!(server.phase(), (MatchPhase::Results, 3));
        assert_eq!(server.simulation().outcome(), Some(moba_game::Team::Blue));

        for _ in 0..3 {
            server.tick().expect("results timer");
        }
        assert_eq!(server.phase(), (MatchPhase::Lobby, 0));
        assert_eq!(server.match_number(), 1);
        assert_eq!(server.simulation().outcome(), None);
    }
}
