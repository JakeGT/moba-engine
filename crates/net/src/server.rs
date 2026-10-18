//! Authoritative UDP server: handshake, player slots and snapshot broadcasting.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use moba_game::GameContent;
use moba_sim::{ObservedUnit, PlayerCommand, Replay, SimConfig, Simulation, TickInputs};

use crate::crypto::{
    SessionCrypto, derive_session_key, identity_public_key, random_nonce, sign_server_welcome,
    verify_hello_tag, welcome_tag,
};
use crate::protocol::{
    EVENT_CAST, EVENT_MATCH_ENDED, EVENT_PROJECTILE_HIT, EVENT_PROJECTILE_SPAWN,
    EVENT_STRUCTURE_DESTROYED, EVENT_UNIT_DIED, EVENT_ZONE_SPAWNED, FLAG_NEED_FULL, Header,
    InputMessage, MAX_DATAGRAM, MAX_SNAPSHOT_EVENTS, MatchPhase, NetEntity, NetEvent, NetId,
    PacketKind, SnapshotDelta, SnapshotState, Welcome, WireCatalog, WireCommand, build_delta_into,
    decode_hello, decode_input, dequantize_position, encode_snapshot, encode_welcome,
    quantize_position, sequence_after,
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
    /// Optional path for deterministic replay files (one per match).
    pub record: Option<PathBuf>,
    /// Additional read-only spectator connections beyond the ten player slots.
    pub max_spectators: usize,
    /// Per-address UDP packet budget per second (0 disables rate limiting).
    pub max_packets_per_second: u32,
    /// Global UDP packet budget per second across all addresses (0 disables).
    pub max_total_packets_per_second: u32,
    /// Require every connected player to ready up before starting the countdown.
    pub require_ready: bool,
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
            .field("record", &self.record)
            .field("max_spectators", &self.max_spectators)
            .field("max_packets_per_second", &self.max_packets_per_second)
            .field(
                "max_total_packets_per_second",
                &self.max_total_packets_per_second,
            )
            .field("require_ready", &self.require_ready)
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
            record: None,
            max_spectators: 8,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
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
    /// Datagrams dropped by the per-address rate limiter.
    pub rate_limited: u64,
    /// Hello packets rejected as cross-address replays.
    pub replayed_hellos: u64,
    /// Total nanoseconds spent inside simulation ticks.
    pub tick_nanos: u64,
    /// Longest simulation tick.
    pub tick_max_nanos: u64,
    /// Simulation tick number of the longest tick.
    pub tick_max_at: u32,
    /// Wave number during the longest tick.
    pub tick_max_wave: u32,
    /// Live units during the longest tick.
    pub tick_max_units: u32,
    /// Simulation ticks at or above 500 µs.
    pub tick_slow_500us: u64,
    /// Simulation ticks at or above 1 ms.
    pub tick_slow_1ms: u64,
    /// Longest total server frame (inputs + simulation + snapshots).
    pub frame_max_nanos: u64,
    /// Server frame number of the longest frame.
    pub frame_max_at: u32,
    /// Server frames at or above 1 ms.
    pub frame_slow_1ms: u64,
    /// Mean simulation tick, filled by [`NetServer::stats`].
    pub tick_avg_nanos: u64,
}

#[derive(Debug)]
struct RateBucket {
    addr: SocketAddr,
    tokens: f32,
    last_refill: Instant,
    last_seen: Instant,
}

#[derive(Debug)]
struct RateLimiter {
    rate: f32,
    burst: f32,
    total_rate: f32,
    total_burst: f32,
    total_tokens: f32,
    total_last: Instant,
    max_buckets: usize,
    buckets: Vec<RateBucket>,
}

impl RateLimiter {
    fn new(packets_per_second: u32, total_packets_per_second: u32, max_buckets: usize) -> Self {
        let rate = packets_per_second as f32;
        let total_rate = total_packets_per_second as f32;
        Self {
            rate,
            burst: rate.max(1.0),
            total_rate,
            total_burst: total_rate.max(1.0),
            total_tokens: total_rate.max(1.0),
            total_last: Instant::now(),
            max_buckets,
            buckets: Vec::new(),
        }
    }

    fn allow_at(&mut self, addr: SocketAddr, now: Instant) -> bool {
        if self.rate <= 0.0 {
            return true;
        }
        if self.total_rate > 0.0 {
            let elapsed = now.saturating_duration_since(self.total_last).as_secs_f32();
            self.total_tokens =
                (self.total_tokens + elapsed * self.total_rate).min(self.total_burst);
            self.total_last = now;
            if self.total_tokens < 1.0 {
                return false;
            }
            self.total_tokens -= 1.0;
        }
        let position = self.buckets.iter().position(|bucket| bucket.addr == addr);
        let index = if let Some(index) = position {
            index
        } else if self.buckets.len() >= self.max_buckets {
            let oldest = self
                .buckets
                .iter()
                .enumerate()
                .min_by_key(|(_, bucket)| bucket.last_seen)
                .map_or(0, |(index, _)| index);
            self.buckets[oldest] = RateBucket {
                addr,
                tokens: self.burst,
                last_refill: now,
                last_seen: now,
            };
            oldest
        } else {
            self.buckets.push(RateBucket {
                addr,
                tokens: self.burst,
                last_refill: now,
                last_seen: now,
            });
            self.buckets.len() - 1
        };
        let bucket = &mut self.buckets[index];
        let elapsed = now
            .saturating_duration_since(bucket.last_refill)
            .as_secs_f32();
        bucket.tokens = (bucket.tokens + elapsed * self.rate).min(self.burst);
        bucket.last_refill = now;
        bucket.last_seen = now;
        if bucket.tokens < 1.0 {
            return false;
        }
        bucket.tokens -= 1.0;
        true
    }
}

#[derive(Debug)]
struct HandshakeNonce {
    key: [u8; 32],
    nonce: u64,
    addr: SocketAddr,
    seen: Instant,
}

#[derive(Debug)]
struct ClientChallenge {
    addr: SocketAddr,
    nonce: u64,
    seen: Instant,
}

#[derive(Debug)]
struct NonceTracker {
    entries: Vec<HandshakeNonce>,
    window: Duration,
    max_entries: usize,
}

impl NonceTracker {
    fn new(window: Duration, max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            window,
            max_entries,
        }
    }

    fn allow(&mut self, key: [u8; 32], nonce: u64, addr: SocketAddr, now: Instant) -> bool {
        self.entries
            .retain(|entry| now.saturating_duration_since(entry.seen) <= self.window);
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.key == key && entry.nonce == nonce)
        {
            if entry.addr != addr {
                return false;
            }
            entry.seen = now;
            return true;
        }
        if self.entries.len() >= self.max_entries {
            self.entries.remove(0);
        }
        self.entries.push(HandshakeNonce {
            key,
            nonce,
            addr,
            seen: now,
        });
        true
    }
}

#[derive(Debug)]
struct ContentReload {
    path: PathBuf,
    modified: Option<SystemTime>,
}

struct ServerIdentity {
    seed: [u8; 32],
    public: [u8; 32],
}

impl core::fmt::Debug for ServerIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ServerIdentity")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct SnapshotRecord {
    seq: u32,
    entities: Vec<NetEntity>,
}

#[derive(Debug)]
struct NetIdTable {
    entries: Vec<Option<(moba_ecs::Entity, NetId)>>,
    next: NetId,
}

impl Default for NetIdTable {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            next: 1,
        }
    }
}

impl NetIdTable {
    fn get_or_insert(&mut self, entity: moba_ecs::Entity) -> NetId {
        let index = entity.index() as usize;
        if index >= self.entries.len() {
            self.entries.resize_with(index + 1, || None);
        }
        if let Some((known, id)) = self.entries[index]
            && known == entity
        {
            return id;
        }
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        self.entries[index] = Some((entity, id));
        id
    }

    fn entity_for(&self, net_id: NetId) -> Option<moba_ecs::Entity> {
        self.entries
            .iter()
            .flatten()
            .find(|(_, id)| *id == net_id)
            .map(|(entity, _)| *entity)
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.next = 1;
    }
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // Connection flags are independent state.
struct ClientConn {
    addr: SocketAddr,
    session: u32,
    slot: u8,
    key: Option<[u8; 32]>,
    spectator: bool,
    team: moba_game::Team,
    connected: bool,
    ready: bool,
    last_seen: Instant,
    disconnected_at: Option<Instant>,
    last_input_seq: u32,
    command: Option<WireCommand>,
    pending_purchase: Option<u8>,
    net_ids: NetIdTable,
    snapshot_seq: u32,
    acked_snapshot: u32,
    need_full: bool,
    history: VecDeque<SnapshotRecord>,
    history_pool: Vec<Vec<NetEntity>>,
    welcome_nonce: u64,
    welcome_payload: Vec<u8>,
    bytes_sent: u64,
}

impl ClientConn {
    fn net_id_for(&mut self, entity: moba_ecs::Entity) -> NetId {
        self.net_ids.get_or_insert(entity)
    }

    fn entity_for(&self, net_id: NetId) -> Option<moba_ecs::Entity> {
        self.net_ids.entity_for(net_id)
    }

    fn baseline(&self, seq: u32) -> Option<&[NetEntity]> {
        self.history
            .iter()
            .find(|record| record.seq == seq)
            .map(|record| record.entities.as_slice())
    }

    fn record(&mut self, seq: u32, entities: &[NetEntity]) {
        let mut buffer = self.history_pool.pop().unwrap_or_default();
        buffer.clear();
        buffer.extend_from_slice(entities);
        self.history.push_back(SnapshotRecord {
            seq,
            entities: buffer,
        });
        while self.history.len() > SNAPSHOT_HISTORY {
            if let Some(record) = self.history.pop_front() {
                let mut entities = record.entities;
                entities.clear();
                self.history_pool.push(entities);
            }
        }
    }

    fn clear_history(&mut self) {
        while let Some(record) = self.history.pop_front() {
            let mut entities = record.entities;
            entities.clear();
            self.history_pool.push(entities);
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

/// Event retained between ticks and filtered per viewer when snapshots are sent.
#[derive(Copy, Clone, Debug)]
struct RawEvent {
    kind: u8,
    team: moba_game::Team,
    source: Option<moba_ecs::Entity>,
    target: Option<moba_ecs::Entity>,
    source_slot: u8,
    target_slot: u8,
    position: moba_core::FixVec2,
    end: moba_core::FixVec2,
    direction: moba_core::FixVec2,
    value: u16,
    aux: u16,
    arc: moba_core::Fix,
}

impl RawEvent {
    #[allow(clippy::too_many_lines)] // One exhaustive SimEvent conversion.
    fn from_sim(event: &moba_sim::SimEvent, simulation: &moba_sim::Simulation) -> Option<Self> {
        let slot = |entity: moba_ecs::Entity| {
            simulation
                .world()
                .get::<moba_sim::HeroSlot>(entity)
                .map_or(0xFF, |slot| slot.0)
        };
        let raw = match *event {
            moba_sim::SimEvent::AbilityCast {
                caster,
                team,
                ability,
                target,
            } => Self {
                kind: EVENT_CAST,
                team,
                source: Some(caster),
                target: None,
                source_slot: slot(caster),
                target_slot: 0xFF,
                position: target,
                end: target,
                direction: moba_core::FixVec2::ZERO,
                value: u16::from(ability),
                aux: 0,
                arc: moba_core::Fix::ZERO,
            },
            moba_sim::SimEvent::ProjectileSpawned {
                owner,
                team,
                position,
                direction,
                speed_per_sec,
                radius,
                range,
                arc,
            } => Self {
                kind: EVENT_PROJECTILE_SPAWN,
                team,
                source: Some(owner),
                target: None,
                source_slot: slot(owner),
                target_slot: 0xFF,
                position,
                end: position + direction * range,
                direction,
                value: u16::try_from(speed_per_sec.to_i32().max(0)).unwrap_or(u16::MAX),
                aux: u16::try_from(radius.to_i32().max(0)).unwrap_or(u16::MAX),
                arc,
            },
            moba_sim::SimEvent::ProjectileHit {
                owner,
                team,
                target,
                position,
            } => Self {
                kind: EVENT_PROJECTILE_HIT,
                team,
                source: Some(owner),
                target: Some(target),
                source_slot: slot(owner),
                target_slot: slot(target),
                position,
                end: position,
                direction: moba_core::FixVec2::ZERO,
                value: 0,
                aux: 0,
                arc: moba_core::Fix::ZERO,
            },
            moba_sim::SimEvent::UnitDied {
                entity,
                kind,
                team,
                position,
                killer,
                hero_slot,
                killer_slot,
            } => Self {
                kind: EVENT_UNIT_DIED,
                team,
                source: killer,
                target: Some(entity),
                source_slot: killer_slot.unwrap_or(0xFF),
                target_slot: hero_slot.unwrap_or(0xFF),
                position,
                end: position,
                direction: moba_core::FixVec2::ZERO,
                value: kind as u16,
                aux: 0,
                arc: moba_core::Fix::ZERO,
            },
            moba_sim::SimEvent::StructureDestroyed {
                team,
                kind,
                position,
            } => Self {
                kind: EVENT_STRUCTURE_DESTROYED,
                team,
                source: None,
                target: None,
                source_slot: 0xFF,
                target_slot: 0xFF,
                position,
                end: position,
                direction: moba_core::FixVec2::ZERO,
                value: kind as u16,
                aux: 0,
                arc: moba_core::Fix::ZERO,
            },
            moba_sim::SimEvent::MatchEnded { winner } => Self {
                kind: EVENT_MATCH_ENDED,
                team: winner,
                source: None,
                target: None,
                source_slot: 0xFF,
                target_slot: 0xFF,
                position: moba_core::FixVec2::ZERO,
                end: moba_core::FixVec2::ZERO,
                direction: moba_core::FixVec2::ZERO,
                value: winner.index() as u16,
                aux: 0,
                arc: moba_core::Fix::ZERO,
            },
            moba_sim::SimEvent::ZoneSpawned {
                owner,
                team,
                position,
                radius,
                duration_ticks,
            } => Self {
                kind: EVENT_ZONE_SPAWNED,
                team,
                source: Some(owner),
                target: None,
                source_slot: slot(owner),
                target_slot: 0xFF,
                position,
                end: position,
                direction: moba_core::FixVec2::ZERO,
                value: u16::try_from(radius.to_i32().max(0)).unwrap_or(u16::MAX),
                aux: u16::try_from(duration_ticks).unwrap_or(u16::MAX),
                arc: moba_core::Fix::ZERO,
            },
            moba_sim::SimEvent::WaveSpawned { .. } | moba_sim::SimEvent::HeroRespawned { .. } => {
                return None;
            }
        };
        Some(raw)
    }

    fn visible_to(
        &self,
        viewer: moba_game::Team,
        spectator: bool,
        units: &[moba_sim::ObservedUnit],
    ) -> bool {
        if spectator || self.kind == EVENT_MATCH_ENDED || self.team == viewer {
            return true;
        }
        let visible = |entity: moba_ecs::Entity| units.iter().any(|unit| unit.entity == entity);
        match (self.source, self.target) {
            (Some(source), _) => visible(source),
            (None, Some(target)) => visible(target),
            (None, None) => false,
        }
    }

    fn to_net(self, client: &mut ClientConn) -> NetEvent {
        let (x, y) = quantize_position(self.position);
        let (x2, y2) = quantize_position(self.end);
        NetEvent {
            kind: self.kind,
            team: self.team.index() as u8,
            source: self
                .source
                .map_or(0xFFFF, |entity| client.net_id_for(entity)),
            target: self
                .target
                .map_or(0xFFFF, |entity| client.net_id_for(entity)),
            source_slot: self.source_slot,
            target_slot: self.target_slot,
            x,
            y,
            x2,
            y2,
            value: self.value,
            aux: self.aux,
            dx: (self.direction.x.to_f32() * 32_767.0).clamp(-32_767.0, 32_767.0) as i16,
            dy: (self.direction.y.to_f32() * 32_767.0).clamp(-32_767.0, 32_767.0) as i16,
            arc: (self.arc.to_f32() * 16.0).clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
        }
    }
}

/// Observable server phase passed to extension systems.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ServerPhase {
    /// Waiting for players.
    Lobby,
    /// Countdown before play.
    Countdown,
    /// Simulation is ticking.
    Playing,
    /// Post-match results screen.
    Results,
}

type ExtensionSystem = Box<dyn FnMut(&mut Simulation, ServerPhase) + Send>;

#[derive(Default)]
struct Systems(Vec<ExtensionSystem>);

impl core::fmt::Debug for Systems {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("Systems").field(&self.0.len()).finish()
    }
}

/// A running authoritative network server.
#[derive(Debug)]
pub struct NetServer {
    transport: UdpTransport,
    simulation: Simulation,
    content: Arc<GameContent>,
    identity: Option<ServerIdentity>,
    client_keys: Vec<[u8; 32]>,
    require_client_identity: bool,
    allowed_client_keys: Vec<[u8; 32]>,
    client_challenges: Vec<ClientChallenge>,
    nonces: NonceTracker,
    reload: Option<ContentReload>,
    rotation: Vec<PathBuf>,
    rotation_index: usize,
    limiter: RateLimiter,
    config: ServerConfig,
    clients: Vec<Option<ClientConn>>,
    observed: [Vec<ObservedUnit>; 2],
    observed_spectator: Vec<ObservedUnit>,
    current: Vec<NetEntity>,
    payload: Vec<u8>,
    delta: SnapshotDelta,
    session_counter: u32,
    stats: ServerStats,
    phase: Phase,
    systems: Systems,
    frame: u64,
    match_number: u32,
    record_frames: Vec<TickInputs>,
    record_checkpoints: Vec<(u32, u64)>,
    pending_events: Vec<RawEvent>,
    results_log: Option<PathBuf>,
    results_logged: bool,
    recorded: bool,
    surrender_votes: [u16; 2],
    pending_surrender: Option<moba_game::Team>,
}

impl NetServer {
    /// Binds the server socket and starts a match.
    ///
    /// # Errors
    /// Returns an OS error when the socket cannot be bound.
    pub fn bind(config: ServerConfig) -> io::Result<Self> {
        Self::bind_with_content(config, Arc::new(GameContent::default()))
    }

    /// Binds with a validated runtime content pack.
    ///
    /// # Errors
    /// Returns an OS error when the socket cannot be bound.
    pub fn bind_with_content(config: ServerConfig, content: Arc<GameContent>) -> io::Result<Self> {
        let transport = UdpTransport::bind(config.bind)?;
        let total_slots = config.max_clients + config.max_spectators;
        let config_sim = config.sim;
        let simulation = Simulation::new_with_content(config_sim, Arc::clone(&content));
        let limiter = RateLimiter::new(
            config.max_packets_per_second,
            config.max_total_packets_per_second,
            256,
        );
        Ok(Self {
            transport,
            simulation,
            content,
            identity: None,
            client_keys: Vec::new(),
            require_client_identity: false,
            allowed_client_keys: Vec::new(),
            client_challenges: Vec::new(),
            nonces: NonceTracker::new(Duration::from_secs(60), 2_048),
            reload: None,
            rotation: Vec::new(),
            rotation_index: 0,
            limiter,
            config,
            clients: (0..total_slots).map(|_| None).collect(),
            observed: std::array::from_fn(|_| Vec::with_capacity(moba_sim::MAX_UNITS)),
            observed_spectator: Vec::with_capacity(moba_sim::MAX_UNITS),
            current: Vec::with_capacity(moba_sim::MAX_UNITS),
            payload: Vec::with_capacity(MAX_DATAGRAM),
            delta: SnapshotDelta::default(),
            session_counter: 1,
            stats: ServerStats::default(),
            phase: Phase::Lobby,
            systems: Systems::default(),
            frame: 0,
            match_number: 0,
            record_frames: Vec::new(),
            record_checkpoints: Vec::new(),
            pending_events: Vec::with_capacity(MAX_SNAPSHOT_EVENTS * 4),
            results_log: None,
            results_logged: false,
            recorded: false,
            surrender_votes: [0; 2],
            pending_surrender: None,
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

    /// Enables pinned server identity signing and returns the public key.
    pub fn set_server_identity(&mut self, seed: [u8; 32]) -> [u8; 32] {
        let public = identity_public_key(&seed);
        self.identity = Some(ServerIdentity { seed, public });
        public
    }

    /// Current server public key, when identity signing is enabled.
    #[must_use]
    pub fn server_public_key(&self) -> Option<[u8; 32]> {
        self.identity.as_ref().map(|identity| identity.public)
    }

    /// Appends one JSONL record per completed match to `path`.
    pub fn set_results_log(&mut self, path: PathBuf) {
        self.results_log = Some(path);
    }

    /// Adds a per-client key to the accepted key ring (Hello HMAC + session encryption).
    pub fn add_client_key(&mut self, key: [u8; 32]) {
        if !self.client_keys.contains(&key) {
            self.client_keys.push(key);
        }
    }

    /// Requires every client to prove possession of one of the allowed Ed25519 keys.
    pub fn require_client_identity(&mut self, keys: impl IntoIterator<Item = [u8; 32]>) {
        self.allowed_client_keys.clear();
        self.allowed_client_keys.extend(keys);
        self.require_client_identity = true;
    }

    fn issue_challenge(&mut self, addr: SocketAddr) -> io::Result<()> {
        let now = Instant::now();
        self.client_challenges.retain(|challenge| {
            now.saturating_duration_since(challenge.seen) <= Duration::from_secs(10)
        });
        let nonce = random_nonce()?;
        self.client_challenges.push(ClientChallenge {
            addr,
            nonce,
            seen: now,
        });
        if self.client_challenges.len() > 1_024 {
            self.client_challenges.remove(0);
        }
        let header = Header {
            kind: PacketKind::Challenge,
            flags: 0,
            session: 0,
            seq: 0,
            ack: 0,
            ack_bits: 0,
        };
        self.transport
            .send_plaintext(addr, &header, &nonce.to_le_bytes())
    }

    fn take_challenge(&mut self, addr: SocketAddr, nonce: u64) -> bool {
        if nonce == 0 {
            return false;
        }
        let before = self.client_challenges.len();
        self.client_challenges
            .retain(|challenge| !(challenge.addr == addr && challenge.nonce == nonce));
        before != self.client_challenges.len()
    }

    fn challenge_is_pending(&self, addr: SocketAddr, nonce: u64) -> bool {
        self.client_challenges.iter().any(|challenge| {
            challenge.addr == addr
                && challenge.nonce == nonce
                && challenge.seen.elapsed() <= Duration::from_secs(10)
        })
    }

    /// Watches a content pack file and reloads it while the lobby is empty.
    ///
    /// Connected clients keep their handshake catalogue, so reloads are deferred until no
    /// clients are connected and the server is back in the lobby.
    pub fn watch_content(&mut self, path: PathBuf) {
        let modified = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok();
        self.reload = Some(ContentReload { path, modified });
    }

    /// Rotates through content packs between empty-lobby matches.
    pub fn watch_rotation(&mut self, paths: Vec<PathBuf>) {
        self.rotation = paths;
        self.rotation_index = 0;
    }

    fn rotate_content(&mut self) {
        if self.rotation.len() < 2 || self.clients.iter().flatten().any(|client| client.connected) {
            return;
        }
        let next = (self.rotation_index + 1) % self.rotation.len();
        let path = self.rotation[next].clone();
        match moba_game::load_content_file(&path) {
            Ok(content) => {
                let hash = content.hash();
                let name = content.map.name.clone();
                let content = Arc::new(content);
                self.content = content;
                self.rotation_index = next;
                eprintln!(
                    "map rotation: selected {} ({name}, hash {hash:#018x})",
                    path.display()
                );
            }
            Err(error) => eprintln!("map rotation failed for {}: {error}", path.display()),
        }
    }

    fn poll_content_reload(&mut self) {
        let Some(reload) = self.reload.as_mut() else {
            return;
        };
        if self.phase != Phase::Lobby
            || self.clients.iter().flatten().any(|client| client.connected)
        {
            return;
        }
        let Ok(modified) = std::fs::metadata(&reload.path).and_then(|metadata| metadata.modified())
        else {
            return;
        };
        if reload.modified == Some(modified) {
            return;
        }
        reload.modified = Some(modified);
        match moba_game::load_content_file(&reload.path) {
            Ok(content) => {
                let hash = content.hash();
                let content = Arc::new(content);
                self.simulation =
                    Simulation::new_with_content(self.config.sim, Arc::clone(&content));
                self.content = content;
                self.record_frames.clear();
                self.recorded = false;
                eprintln!(
                    "content reloaded {} items, hash {hash:#018x}",
                    self.content.items.len()
                );
            }
            Err(error) => eprintln!("content reload failed: {error}"),
        }
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
        stats.tick_avg_nanos = stats.tick_nanos.checked_div(stats.ticks).unwrap_or(0);
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
            if !self.limiter.allow_at(message.addr, Instant::now()) {
                self.stats.rate_limited += 1;
                continue;
            }
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
    /// Registers an extension system run after each simulation tick in insertion order.
    #[must_use]
    pub fn with_system<F>(mut self, system: F) -> Self
    where
        F: FnMut(&mut Simulation, ServerPhase) + Send + 'static,
    {
        self.systems.0.push(Box::new(system));
        self
    }

    /// Returns the extension systems count.
    #[must_use]
    pub fn system_count(&self) -> usize {
        self.systems.0.len()
    }

    /// Advances one server frame.
    ///
    /// # Errors
    /// Returns a socket error when snapshots cannot be sent.
    pub fn tick(&mut self) -> io::Result<()> {
        let frame_started = Instant::now();
        self.frame = self.frame.wrapping_add(1);
        if self.frame % 30 == 0 {
            self.poll_content_reload();
        }
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
                for system in &mut self.systems.0 {
                    system(&mut self.simulation, ServerPhase::Playing);
                }
                if self.simulation.outcome().is_some() {
                    self.phase = Phase::Results(self.config.results_ticks.max(1));
                    self.write_recording();
                    self.write_result_log();
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
        let frame_nanos = frame_started.elapsed().as_nanos() as u64;
        if frame_nanos >= 1_000_000 {
            self.stats.frame_slow_1ms += 1;
        }
        if frame_nanos > self.stats.frame_max_nanos {
            self.stats.frame_max_nanos = frame_nanos;
            self.stats.frame_max_at = u32::try_from(self.frame).unwrap_or(u32::MAX);
        }
        Ok(())
    }

    fn tick_simulation(&mut self) {
        let mut inputs: TickInputs = self.simulation.bot_inputs();
        for slot in 0..self.config.max_clients.min(self.clients.len()) {
            let Some(client) = self.clients[slot].as_mut() else {
                continue;
            };
            if let Some(wire) = client.command.take() {
                if let Some(command) = wire_to_command(client, wire) {
                    inputs.set(slot, command);
                }
            }
            if let Some(item) = client.pending_purchase.take() {
                inputs.set_purchase(slot, item);
            }
        }
        if let Some(team) = self.pending_surrender.take() {
            for slot in 0..self.config.max_clients.min(self.clients.len()) {
                if let Some(client) = self.clients[slot].as_ref() {
                    if client.connected && !client.spectator && client.team == team {
                        inputs.set(slot, PlayerCommand::Surrender);
                        break;
                    }
                }
            }
        }
        if self.config.record.is_some() {
            self.record_frames.push(inputs);
        }
        let started = Instant::now();
        self.simulation.tick(&inputs);
        for event in self.simulation.events() {
            if let Some(raw) = RawEvent::from_sim(event, &self.simulation) {
                if self.pending_events.len() >= MAX_SNAPSHOT_EVENTS * 4 {
                    self.pending_events.remove(0);
                }
                self.pending_events.push(raw);
            }
        }
        self.simulation.clear_events();
        if self.config.record.is_some()
            && self.record_frames.len() % 150 == 0
            && !self.record_frames.is_empty()
        {
            self.record_checkpoints.push((
                u32::try_from(self.record_frames.len()).unwrap_or(u32::MAX),
                self.simulation.state_hash(),
            ));
        }
        let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.stats.tick_nanos = self.stats.tick_nanos.saturating_add(elapsed);
        self.stats.tick_max_nanos = self.stats.tick_max_nanos.max(elapsed);
        self.stats.ticks += 1;
    }

    fn start_countdown(&mut self) {
        self.phase = if self.config.countdown_ticks == 0 {
            Phase::Playing
        } else {
            Phase::Countdown(self.config.countdown_ticks)
        };
        self.surrender_votes = [0; 2];
        self.pending_surrender = None;
    }

    fn maybe_start_countdown(&mut self) {
        if self.phase != Phase::Lobby {
            return;
        }
        if self.config.require_ready && !self.all_players_ready() {
            return;
        }
        self.start_countdown();
    }

    fn all_players_ready(&self) -> bool {
        let mut players = 0;
        for slot in 0..self.config.max_clients.min(self.clients.len()) {
            let Some(client) = self.clients[slot].as_ref() else {
                continue;
            };
            if !client.connected {
                continue;
            }
            players += 1;
            if !client.ready {
                return false;
            }
        }
        players > 0
    }

    fn set_ready(&mut self, slot: usize, ready: bool) {
        let Some(client) = self.clients[slot].as_mut() else {
            return;
        };
        if client.spectator {
            return;
        }
        client.ready = ready;
        if !ready && matches!(self.phase, Phase::Countdown(_)) {
            self.phase = Phase::Lobby;
        }
        self.maybe_start_countdown();
    }

    fn cast_surrender_vote(&mut self, slot: usize) {
        if !matches!(self.phase, Phase::Playing) {
            return;
        }
        let Some(client) = self.clients[slot].as_ref() else {
            return;
        };
        let team = client.team;
        self.surrender_votes[team.index()] |= 1 << slot;
        let mut connected = 0u16;
        for index in 0..self.config.max_clients.min(self.clients.len()) {
            if let Some(player) = self.clients[index].as_ref() {
                if player.connected && !player.spectator && player.team == team {
                    connected |= 1 << index;
                }
            }
        }
        if connected != 0 && self.surrender_votes[team.index()] & connected == connected {
            self.pending_surrender = Some(team);
        }
    }

    fn reset_match(&mut self) {
        self.match_number = self.match_number.wrapping_add(1);
        self.record_frames.clear();
        self.record_checkpoints.clear();
        self.recorded = false;
        self.results_logged = false;
        let mut config = self.config.sim;
        config.seed = config.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.config.sim = config;
        self.rotate_content();
        self.simulation = Simulation::new_with_content(config, Arc::clone(&self.content));
        for slot in 0..self.config.max_clients.min(self.clients.len()) {
            let Some(client) = self.clients[slot].as_mut() else {
                continue;
            };
            client.clear_history();
            client.net_ids.clear();
            client.acked_snapshot = 0;
            client.need_full = true;
            client.command = None;
            client.pending_purchase = None;
            client.last_input_seq = 0;
        }
        self.refresh_bot_mask();
        self.surrender_votes = [0; 2];
        self.pending_surrender = None;
        for slot in 0..self.config.max_clients.min(self.clients.len()) {
            if let Some(client) = self.clients[slot].as_mut() {
                client.ready = false;
            }
        }
        if self.config.require_ready {
            self.phase = Phase::Lobby;
        } else if self.clients.iter().any(Option::is_some) {
            self.start_countdown();
        } else {
            self.phase = Phase::Lobby;
        }
    }

    /// Writes any in-progress match replay. Safe to call multiple times.
    ///
    /// A completed match is written automatically when its nexus falls; this method is for
    /// graceful server shutdown during a live match.
    pub fn finish_recording(&mut self) {
        self.write_recording();
    }

    fn write_result_log(&mut self) {
        if self.results_logged {
            return;
        }
        let Some(path) = self.results_log.clone() else {
            return;
        };
        self.results_logged = true;
        let outcome = self.simulation.outcome().map_or("draw", |team| match team {
            moba_game::Team::Blue => "blue",
            moba_game::Team::Red => "red",
        });
        let (blue, red) = self.simulation.score();
        let replay = self
            .config
            .record
            .as_ref()
            .map(|path| replay_path(path, self.match_number));
        let line = result_json(
            self.match_number.wrapping_add(1),
            self.config.sim.seed,
            self.simulation.current_tick().get(),
            outcome,
            blue,
            red,
            &self.content.manifest.name,
            &self.content.manifest.version,
            self.content.hash(),
            self.simulation.state_hash(),
            replay.as_deref(),
        );
        let write = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write as _;
                file.write_all(line.as_bytes())
                    .and_then(|()| file.write_all(b"\n"))
            });
        if let Err(error) = write {
            eprintln!("results log {}: {error}", path.display());
        }
    }

    fn write_recording(&mut self) {
        if self.recorded || self.record_frames.is_empty() {
            return;
        }
        let Some(path) = self.config.record.clone() else {
            return;
        };
        let state_hash = self.simulation.state_hash();
        let replay = Replay {
            config: self.simulation.config(),
            frames: std::mem::take(&mut self.record_frames),
            sim_logic_version: moba_sim::SIM_LOGIC_VERSION,
            expected_hash: Some(state_hash),
            content_hash: self.content.hash(),
            content_name: self.content.manifest.name.clone(),
            content_version: self.content.manifest.version.clone(),
            checkpoints: std::mem::take(&mut self.record_checkpoints),
        };
        let target = replay_path(&path, self.match_number);
        match std::fs::write(&target, replay.encode()) {
            Ok(()) => eprintln!(
                "replay: wrote {} ticks to {} ({state_hash:#018x})",
                replay.frames.len(),
                target.display()
            ),
            Err(error) => eprintln!("replay: cannot write {}: {error}", target.display()),
        }
        self.recorded = true;
    }

    /// Runs a real-time 30 Hz loop until `duration` elapses or `shutdown` is set.
    ///
    /// # Errors
    /// Returns socket I/O errors.
    pub fn run_until(
        &mut self,
        duration: Duration,
        shutdown: &std::sync::atomic::AtomicBool,
    ) -> io::Result<()> {
        let tick_duration = Duration::from_secs_f64(1.0 / f64::from(SERVER_TICK_RATE));
        let start = Instant::now();
        let mut next_tick = Instant::now();
        while start.elapsed() < duration && !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
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

    /// Runs a real-time 30 Hz loop for `duration`.
    ///
    /// # Errors
    /// Returns socket I/O errors.
    pub fn run_for(&mut self, duration: Duration) -> io::Result<()> {
        let shutdown = std::sync::atomic::AtomicBool::new(false);
        self.run_until(duration, &shutdown)
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
                    client.ready = false;
                    client.disconnected_at = Some(now);
                    client.command = None;
                    self.surrender_votes[client.team.index()] &= !(1 << slot);
                    if self.config.require_ready && matches!(self.phase, Phase::Countdown(_)) {
                        self.phase = Phase::Lobby;
                    }
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
        let active = usize::from(self.content.rules.team_size) * 2;
        let mut mask = if self.content.rules.bot_fill && active <= 16 {
            ((1u32 << active) - 1) as u16
        } else {
            0
        };
        for slot in 0..self.config.max_clients.min(self.clients.len()).min(active) {
            if self.clients[slot]
                .as_ref()
                .is_some_and(|client| client.connected)
            {
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
                    // Explicitly reject stale clients instead of silently dropping them.
                    self.reject_client(message.addr, message.header.session, 1)?;
                    return Ok(());
                }
                if self.require_client_identity {
                    if !hello.identity || !self.allowed_client_keys.contains(&hello.client_public) {
                        self.reject_client(message.addr, 0, 5)?;
                        return Ok(());
                    }
                    if !self.challenge_is_pending(message.addr, hello.server_challenge) {
                        self.issue_challenge(message.addr)?;
                        return Ok(());
                    }
                    if !crate::crypto::verify_client_hello(
                        &hello.client_public,
                        hello.protocol,
                        hello.nonce,
                        hello.server_challenge,
                        hello.reconnect,
                        hello.spectate,
                        &hello.client_signature,
                    ) {
                        let _ = self.take_challenge(message.addr, hello.server_challenge);
                        self.reject_client(message.addr, 0, 5)?;
                        return Ok(());
                    }
                    let _ = self.take_challenge(message.addr, hello.server_challenge);
                }
                let keys_configured =
                    self.config.pre_shared_key.is_some() || !self.client_keys.is_empty();
                let matched_key = self.config.pre_shared_key.filter(|key| {
                    verify_hello_tag(
                        key,
                        hello.protocol,
                        hello.nonce,
                        hello.reconnect,
                        hello.spectate,
                        &hello.auth,
                    )
                });
                let matched_key = matched_key.or_else(|| {
                    self.client_keys
                        .iter()
                        .find(|key| {
                            verify_hello_tag(
                                key,
                                hello.protocol,
                                hello.nonce,
                                hello.reconnect,
                                hello.spectate,
                                &hello.auth,
                            )
                        })
                        .copied()
                });
                if keys_configured && matched_key.is_none() {
                    self.reject_client(message.addr, 0, 4)?;
                    return Ok(());
                }
                if !self.nonces.allow(
                    matched_key.unwrap_or([0; 32]),
                    hello.nonce,
                    message.addr,
                    Instant::now(),
                ) {
                    self.stats.replayed_hellos += 1;
                    self.reject_client(message.addr, 0, 4)?;
                    return Ok(());
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
                    if let Some(client) = self.clients[slot].as_mut() {
                        client.key = matched_key;
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
                    if let Some(client) = self.clients[slot].as_mut() {
                        client.key = matched_key;
                    }
                    self.reconnect_client(slot, message.addr, hello.nonce)?;
                    return Ok(());
                }
                let slot = if hello.spectate {
                    self.free_spectator_slot()
                } else {
                    self.free_slot()
                };
                let Some(slot) = slot else {
                    self.reject_client(message.addr, 0, 2)?;
                    return Ok(());
                };
                let session = 0xA5A5_0000 | (self.session_counter & 0xFFFF);
                self.session_counter = self.session_counter.wrapping_add(1);
                let team_size = usize::from(self.content.rules.team_size);
                let team = if hello.spectate || slot < team_size {
                    moba_game::Team::Blue
                } else {
                    moba_game::Team::Red
                };
                self.clients[slot] = Some(ClientConn {
                    addr: message.addr,
                    session,
                    key: matched_key,
                    slot: if hello.spectate {
                        0xFF
                    } else {
                        u8::try_from(slot).expect("player slot fits u8")
                    },
                    spectator: hello.spectate,
                    team,
                    connected: true,
                    ready: false,
                    last_seen: Instant::now(),
                    disconnected_at: None,
                    last_input_seq: 0,
                    command: None,
                    pending_purchase: None,
                    net_ids: NetIdTable::default(),
                    snapshot_seq: 0,
                    acked_snapshot: 0,
                    need_full: true,
                    history: VecDeque::new(),
                    history_pool: Vec::new(),
                    welcome_nonce: 0,
                    welcome_payload: Vec::new(),
                    bytes_sent: 0,
                });
                self.refresh_bot_mask();
                self.maybe_start_countdown();
                self.send_welcome(message.addr, slot, hello.nonce, session)?;
            }
            PacketKind::Input => {
                let Some(index) = self.client_index_by_addr(message.addr) else {
                    return Ok(());
                };
                let client = self.clients[index].as_mut().expect("index is live");
                if message.header.session != client.session || !client.connected || client.spectator
                {
                    return Ok(());
                }
                let Ok(input) = decode_input(&message.payload) else {
                    return Ok(());
                };
                client.last_seen = Instant::now();
                let mut ready_change = None;
                let mut surrender = false;
                if sequence_after(input.input_seq, client.last_input_seq) {
                    client.last_input_seq = input.input_seq;
                    match input.command {
                        WireCommand::BuyItem { item } => {
                            if self.phase == Phase::Playing {
                                client.pending_purchase = Some(item);
                            }
                        }
                        WireCommand::Ready { ready } => ready_change = Some(ready),
                        WireCommand::Surrender => surrender = true,
                        command => client.command = Some(command),
                    }
                }
                if let Some(ready) = ready_change {
                    self.set_ready(index, ready);
                }
                if surrender {
                    self.cast_surrender_vote(index);
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
        client.clear_history();
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

    #[allow(clippy::too_many_lines)] // Handshake catalogue assembly is one linear frame.
    fn send_welcome(
        &mut self,
        addr: SocketAddr,
        slot: usize,
        nonce: u64,
        session: u32,
    ) -> io::Result<()> {
        let config = self.config.sim;
        let (slot_u8, spectator) = self
            .clients
            .get(slot)
            .and_then(|client| client.as_ref())
            .map_or((0xFF, true), |client| (client.slot, client.spectator));
        let catalog = WireCatalog::from(&*self.content);
        let content_hash = self.content.hash();
        let mut server_nonce = 0;
        let mut auth = [0u8; 32];
        let mut crypto = None;
        let client_key = self
            .clients
            .get(slot)
            .and_then(|client| client.as_ref())
            .and_then(|client| client.key);
        if let Some(key) = client_key {
            server_nonce = random_nonce()?;
            auth = welcome_tag(&key, nonce, server_nonce, session, slot_u8, content_hash);
            let session_key = derive_session_key(&key, nonce, server_nonce);
            crypto = Some(SessionCrypto::new(session_key, session, 1));
        }
        let (identity, server_public_key, server_signature) =
            self.identity
                .as_ref()
                .map_or((false, [0; 32], [0; 64]), |identity| {
                    (
                        true,
                        identity.public,
                        sign_server_welcome(
                            &identity.seed,
                            u16::from(crate::protocol::PROTOCOL_VERSION),
                            nonce,
                            server_nonce,
                            session,
                            slot_u8,
                            content_hash,
                        ),
                    )
                });
        let welcome = Welcome {
            slot: slot_u8,
            tick_rate: SERVER_TICK_RATE,
            snapshot_interval: self.config.snapshot_interval,
            tick: self.simulation.current_tick().get(),
            seed: config.seed,
            hero_pick: if spectator {
                0
            } else {
                config.hero_picks[slot]
            },
            nonce,
            server_nonce,
            auth,
            spectator,
            catalog,
            content_hash,
            identity,
            server_public_key,
            server_signature,
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
        let active = usize::from(self.content.rules.team_size) * 2;
        (0..self.config.max_clients.min(self.clients.len()).min(active))
            .find(|slot| self.clients[*slot].is_none())
    }

    fn free_spectator_slot(&self) -> Option<usize> {
        (self.config.max_clients.min(self.clients.len())..self.clients.len())
            .find(|slot| self.clients[*slot].is_none())
    }

    fn client_index_by_addr(&self, addr: SocketAddr) -> Option<usize> {
        self.clients
            .iter()
            .position(|client| client.as_ref().is_some_and(|client| client.addr == addr))
    }

    #[allow(clippy::too_many_lines)] // Snapshot fan-out is one ordered pass.
    fn send_snapshots(&mut self) -> io::Result<()> {
        let mut observed = core::mem::take(&mut self.observed);
        for team in moba_game::Team::ALL {
            self.simulation
                .observed_units(team, &mut observed[team.index()]);
        }
        let mut observed_spectator = core::mem::take(&mut self.observed_spectator);
        self.simulation.observed_all(&mut observed_spectator);
        let tick = self.simulation.current_tick().get();
        let (phase_code, countdown) = self.phase();
        let mut current = core::mem::take(&mut self.current);
        let mut payload = core::mem::take(&mut self.payload);
        let pending_events = core::mem::take(&mut self.pending_events);
        let mut delta = core::mem::take(&mut self.delta);
        let mut ready_mask = 0u16;
        let mut player_mask = 0u16;
        for slot in 0..self.config.max_clients.min(self.clients.len()) {
            if let Some(player) = self.clients[slot].as_ref() {
                if player.connected && !player.spectator {
                    player_mask |= 1 << slot;
                    if player.ready {
                        ready_mask |= 1 << slot;
                    }
                }
            }
        }
        for index in 0..self.clients.len() {
            let Some(client) = self.clients[index].as_mut() else {
                continue;
            };
            if !client.connected {
                continue;
            }
            current.clear();
            let viewer_slot = if client.spectator {
                0xFF
            } else {
                u8::try_from(index).unwrap_or(0xFF)
            };
            let units = if client.spectator {
                &observed_spectator
            } else {
                &observed[client.team.index()]
            };
            for unit in units {
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
            build_delta_into(
                &current,
                baseline,
                tick,
                client.last_input_seq,
                baseline_seq,
                &mut delta,
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
            delta.ready_mask = ready_mask;
            delta.player_mask = player_mask;
            let viewer_team = client.team;
            let spectator = client.spectator;
            delta.events = pending_events
                .iter()
                .filter(|event| event.visible_to(viewer_team, spectator, units))
                .take(MAX_SNAPSHOT_EVENTS)
                .map(|event| (*event).to_net(client))
                .collect();
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
            client.record(seq, &current);
            self.stats.snapshots_sent += 1;
        }
        self.observed = observed;
        self.observed_spectator = observed_spectator;
        self.current = current;
        self.payload = payload;
        self.delta = delta;
        self.pending_events.clear();
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
        WireCommand::Ready { .. } => return None,
        WireCommand::Surrender => PlayerCommand::Surrender,
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

fn json_string(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if character < ' ' => {
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

#[allow(clippy::too_many_arguments)] // One flat record keeps call sites explicit.
fn result_json(
    match_number: u32,
    seed: u64,
    ticks: u32,
    outcome: &str,
    blue_score: u32,
    red_score: u32,
    content_name: &str,
    content_version: &str,
    content_hash: u64,
    state_hash: u64,
    replay: Option<&Path>,
) -> String {
    let replay = replay.map_or_else(
        || String::from("null"),
        |path| json_string(&path.to_string_lossy()),
    );
    format!(
        "{{\"match\":{match_number},\"seed\":{seed},\"ticks\":{ticks},\"outcome\":{},\"blue_score\":{blue_score},\"red_score\":{red_score},\"content\":{},\"content_version\":{},\"content_hash\":\"{content_hash:#018x}\",\"state_hash\":\"{state_hash:#018x}\",\"replay\":{replay}}}",
        json_string(outcome),
        json_string(content_name),
        json_string(content_version)
    )
}

fn replay_path(base: &Path, match_number: u32) -> PathBuf {
    if match_number == 0 {
        return base.to_path_buf();
    }
    let stem = base
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("match");
    base.with_file_name(format!("{stem}.match{match_number}.mbr"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_sim::{Health, UnitKind};
    use std::sync::{Arc, Mutex};

    #[test]
    fn dense_net_ids_handle_generation_reuse() {
        let mut table = NetIdTable::default();
        let first = moba_ecs::Entity::from_parts(5, 1);
        let id = table.get_or_insert(first);
        assert_eq!(table.get_or_insert(first), id);
        assert_eq!(table.entity_for(id), Some(first));
        let reused = moba_ecs::Entity::from_parts(5, 2);
        let new_id = table.get_or_insert(reused);
        assert_ne!(id, new_id);
        assert_eq!(table.entity_for(new_id), Some(reused));
        table.clear();
        assert_eq!(table.entity_for(new_id), None);
        assert_eq!(table.get_or_insert(reused), 1);
    }

    #[test]
    fn extension_systems_run_in_order_after_tick() {
        let config = ServerConfig {
            bind: "127.0.0.1:0".parse().expect("address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 0,
            results_ticks: 1,
            client_timeout: Duration::from_millis(100),
            reconnect_grace: Duration::from_secs(1),
            pre_shared_key: None,
            record: None,
            max_spectators: 0,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
        };
        let order = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::clone(&order);
        let second = Arc::clone(&order);
        let mut server = NetServer::bind(config)
            .expect("bind")
            .with_system(move |simulation, phase| {
                assert_eq!(phase, ServerPhase::Playing);
                let _ = simulation.world().entity_count();
                first.lock().expect("lock").push(1u8);
            })
            .with_system(move |_, phase| {
                second
                    .lock()
                    .expect("lock")
                    .push(if phase == ServerPhase::Playing { 2 } else { 9 });
            });
        assert_eq!(server.system_count(), 2);
        server.phase = Phase::Playing;
        server.tick().expect("tick");
        assert_eq!(*order.lock().expect("lock"), vec![1, 2]);
    }

    #[test]
    fn rotation_selects_the_next_pack_when_the_lobby_is_empty() {
        let config = ServerConfig {
            bind: "127.0.0.1:0".parse().expect("address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 0,
            results_ticks: 1,
            client_timeout: Duration::from_millis(100),
            reconnect_grace: Duration::from_secs(1),
            pre_shared_key: None,
            record: None,
            max_spectators: 0,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
        };
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let mut server = NetServer::bind(config).expect("bind");
        server.watch_rotation(vec![root.join("aram.toml"), root.join("1v1-mid.toml")]);
        server.phase = Phase::Lobby;
        server.reset_match();
        assert_eq!(server.content.map.name, "1v1 Mid");
        server.reset_match();
        assert_eq!(server.content.map.name, "ARAM");
    }

    #[test]
    fn event_visibility_requires_an_observed_source_or_target() {
        let source = moba_ecs::Entity::from_parts(9, 1);
        let target = moba_ecs::Entity::from_parts(10, 1);
        let raw = RawEvent {
            kind: EVENT_CAST,
            team: moba_game::Team::Red,
            source: Some(source),
            target: None,
            source_slot: 6,
            target_slot: 0xFF,
            position: moba_core::FixVec2::from_ints(100, 100),
            end: moba_core::FixVec2::ZERO,
            direction: moba_core::FixVec2::ZERO,
            value: 1,
            aux: 0,
            arc: moba_core::Fix::ZERO,
        };
        let hidden = RawEvent::visible_to(&raw, moba_game::Team::Blue, false, &[]);
        assert!(!hidden, "hidden enemy cast must not leak");
        let unit = moba_sim::ObservedUnit {
            entity: source,
            kind: UnitKind::Hero,
            team: moba_game::Team::Red,
            position: moba_core::FixVec2::from_ints(100, 100),
            health: 500,
            max_health: 600,
            mana: 200,
            max_mana: 300,
            dead: false,
            hero_slot: Some(6),
            hero_pick: 0,
            cooldowns: [0; 3],
            level: 1,
            xp: 0,
            gold: 0,
            items: [0xFF; 6],
            shielded: false,
            slowed: false,
            rooted: false,
            silenced: false,
            burning: false,
            hasted: false,
        };
        assert!(RawEvent::visible_to(
            &raw,
            moba_game::Team::Blue,
            false,
            &[unit]
        ));
        assert!(RawEvent::visible_to(&raw, moba_game::Team::Blue, true, &[]));
        let death = RawEvent {
            kind: EVENT_UNIT_DIED,
            team: moba_game::Team::Blue,
            source: None,
            target: Some(target),
            source_slot: 0xFF,
            target_slot: 0,
            ..raw
        };
        assert!(RawEvent::visible_to(
            &death,
            moba_game::Team::Blue,
            false,
            &[]
        ));
    }

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
            record: None,
            max_spectators: 8,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
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

    #[test]
    fn content_reloads_only_with_an_empty_lobby() {
        let path = std::env::temp_dir().join(format!(
            "moba-reload-test-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::write(
            &path,
            r#"[[items]]
name = "First"
cost = 1
"#,
        )
        .expect("seed content");
        let config = ServerConfig {
            bind: "127.0.0.1:0".parse().expect("address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 0,
            results_ticks: 10,
            client_timeout: Duration::from_millis(100),
            reconnect_grace: Duration::from_secs(2),
            pre_shared_key: None,
            record: None,
            max_spectators: 4,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
        };
        let mut server = NetServer::bind(config).expect("bind");
        server.watch_content(path.clone());
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(
            &path,
            r#"[[items]]
name = "Reloaded"
cost = 1
"#,
        )
        .expect("rewrite content");
        for _ in 0..30 {
            server.tick().expect("tick");
        }
        assert_eq!(server.simulation().content().items[0].name, "Reloaded");
        assert_eq!(
            server.simulation().content().hash(),
            moba_game::load_content_file(&path).expect("load").hash()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rate_limiter_enforces_burst_and_refill() {
        let addr: SocketAddr = "127.0.0.1:4000".parse().expect("addr");
        let other: SocketAddr = "127.0.0.1:4001".parse().expect("addr");
        let start = Instant::now();
        let mut limiter = RateLimiter::new(10, 0, 8);
        for _ in 0..10 {
            assert!(limiter.allow_at(addr, start));
        }
        assert!(!limiter.allow_at(addr, start));
        assert!(limiter.allow_at(other, start));
        assert!(limiter.allow_at(addr, start + Duration::from_millis(100)));
        assert!(!limiter.allow_at(addr, start + Duration::from_millis(150)));
    }

    #[test]
    fn client_challenges_are_address_bound_and_expire() {
        let config = ServerConfig {
            bind: "127.0.0.1:0".parse().expect("address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 0,
            results_ticks: 10,
            client_timeout: Duration::from_millis(100),
            reconnect_grace: Duration::from_secs(2),
            pre_shared_key: None,
            record: None,
            max_spectators: 4,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
        };
        let mut server = NetServer::bind(config).expect("bind");
        let first: SocketAddr = "127.0.0.1:6100".parse().expect("addr");
        let second: SocketAddr = "127.0.0.1:6101".parse().expect("addr");
        let nonce = 0x1234_5678;
        server.client_challenges.push(ClientChallenge {
            addr: first,
            nonce,
            seen: Instant::now(),
        });
        assert!(server.challenge_is_pending(first, nonce));
        assert!(!server.challenge_is_pending(second, nonce));
        assert!(!server.take_challenge(second, nonce));
        assert!(server.take_challenge(first, nonce));
        assert!(!server.challenge_is_pending(first, nonce));

        server.client_challenges.push(ClientChallenge {
            addr: first,
            nonce,
            seen: Instant::now()
                .checked_sub(Duration::from_secs(11))
                .expect("test clock"),
        });
        assert!(!server.challenge_is_pending(first, nonce));
    }

    #[test]
    fn run_until_observes_shutdown_flag() {
        let config = ServerConfig {
            bind: "127.0.0.1:0".parse().expect("address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 0,
            results_ticks: 10,
            client_timeout: Duration::from_millis(100),
            reconnect_grace: Duration::from_secs(2),
            pre_shared_key: None,
            record: None,
            max_spectators: 4,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
        };
        let mut server = NetServer::bind(config).expect("bind");
        let shutdown = std::sync::atomic::AtomicBool::new(true);
        server
            .run_until(Duration::from_secs(10), &shutdown)
            .expect("shutdown loop");
        assert_eq!(server.stats().ticks, 0);
    }

    #[test]
    fn results_log_appends_once_per_match() {
        let path = std::env::temp_dir().join(format!(
            "moba-results-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let config = ServerConfig {
            bind: "127.0.0.1:0".parse().expect("address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 0,
            results_ticks: 10,
            client_timeout: Duration::from_millis(100),
            reconnect_grace: Duration::from_secs(2),
            pre_shared_key: None,
            record: None,
            max_spectators: 4,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
        };
        let mut server = NetServer::bind(config).expect("bind");
        server.set_results_log(path.clone());
        server.write_result_log();
        server.write_result_log();
        let text = std::fs::read_to_string(&path).expect("results posted");
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("\"match\":1"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn result_json_is_valid_and_escaped() {
        let line = result_json(
            2,
            99,
            4_500,
            "blue",
            7,
            3,
            "A\"B",
            "1.0",
            0x1234,
            0x5678,
            Some(Path::new("replay\\one.mbr")),
        );
        assert!(line.starts_with("{\"match\":2,\"seed\":99,\"ticks\":4500"));
        assert!(line.contains("\"outcome\":\"blue\""));
        assert!(line.contains("\"blue_score\":7,\"red_score\":3"));
        assert!(line.contains("\"content\":\"A\\\"B\""));
        assert!(line.contains("\"content_hash\":\"0x0000000000001234\""));
        assert!(line.contains("\"state_hash\":\"0x0000000000005678\""));
        assert!(line.contains("\"replay\":\"replay\\\\one.mbr\""));
    }

    #[test]
    fn handshake_nonce_replay_is_address_bound() {
        let key = [9u8; 32];
        let first: SocketAddr = "127.0.0.1:5000".parse().expect("addr");
        let second: SocketAddr = "127.0.0.1:5001".parse().expect("addr");
        let start = Instant::now();
        let mut tracker = NonceTracker::new(Duration::from_secs(10), 8);
        assert!(tracker.allow(key, 7, first, start));
        assert!(tracker.allow(key, 7, first, start));
        assert!(!tracker.allow(key, 7, second, start));
        assert!(tracker.allow(key, 8, second, start));
        assert!(tracker.allow(key, 7, second, start + Duration::from_secs(11)));
    }

    #[test]
    fn global_budget_caps_distributed_floods() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(100, 5, 64);
        let mut allowed = 0;
        for index in 0..20u16 {
            let addr: SocketAddr = format!("127.0.0.1:{}", 5000 + index).parse().expect("addr");
            if limiter.allow_at(addr, start) {
                allowed += 1;
            }
        }
        assert!(allowed <= 5, "global burst caps distinct sources");
    }

    #[test]
    fn recorded_bot_ticks_replay_deterministically() {
        let replay_path = std::env::temp_dir().join(format!(
            "moba-replay-test-{}-{}.mbr",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let config = ServerConfig {
            bind: "127.0.0.1:0".parse().expect("address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 0,
            results_ticks: 10,
            client_timeout: Duration::from_millis(100),
            reconnect_grace: Duration::from_secs(2),
            pre_shared_key: None,
            record: Some(replay_path.clone()),
            max_spectators: 8,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
        };
        let mut server = NetServer::bind(config).expect("bind");
        server.phase = Phase::Playing;
        for _ in 0..60 {
            server.tick().expect("tick");
        }
        server.finish_recording();
        let bytes = std::fs::read(&replay_path).expect("replay written");
        let replay = moba_sim::Replay::decode(&bytes).expect("replay decodes");
        let result = replay.play().expect("replay verifies");
        assert_eq!(result.ticks, 60);
        assert_eq!(result.state_hash, server.simulation().state_hash());
        let _ = std::fs::remove_file(&replay_path);
    }
}
