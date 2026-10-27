//! Authoritative UDP server: handshake, player slots and snapshot broadcasting.
//!
//! Source layout (private submodules; the public API is defined or re-exported here):
//! this file holds configuration, [`NetServer`] and its public lifecycle API;
//! `rate_limit` the per-address/global datagram budget; `ingress` the transport poll
//! loop and per-packet dispatch; `handshake` challenges, nonces and Welcome/reconnect;
//! `clients` per-connection state; `events` presentation-event filtering; `snapshot`
//! snapshot fan-out; `match_flow` lobby/countdown/results phases; `content` pack
//! reload/rotation; `results` replay recording and the results log.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moba_game::GameContent;
use moba_sim::{BotPolicy, ObservedUnit, SimConfig, Simulation, TickInputs};

use crate::crypto::identity_public_key;
use crate::protocol::{
    InputMessage, MAX_DATAGRAM, MAX_SNAPSHOT_EVENTS, MatchPhase, NetEntity, SnapshotDelta,
    SnapshotState, decode_input,
};
use crate::transport::UdpTransport;

use clients::ClientConn;
use content::ContentReload;
use events::RawEvent;
use handshake::{ClientChallenge, NonceTracker};
use rate_limit::RateLimiter;

mod clients;
mod content;
mod events;
mod handshake;
mod ingress;
mod match_flow;
mod rate_limit;
mod results;
mod snapshot;
#[cfg(test)]
mod tests;

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
    /// Snapshots that failed to encode or send; the other clients are still served.
    pub snapshot_send_errors: u64,
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

/// A running authoritative network server.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Lobby,
    Countdown(u32),
    Playing,
    Results(u32),
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
type BotPolicyFactory = Arc<dyn Fn() -> Box<dyn BotPolicy> + Send + Sync>;

#[derive(Default)]
struct Systems(Vec<ExtensionSystem>);

impl core::fmt::Debug for Systems {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("Systems").field(&self.0.len()).finish()
    }
}

/// A running authoritative network server.
#[allow(clippy::struct_excessive_bools)] // Independent protocol/capability flags.
pub struct NetServer {
    transport: UdpTransport,
    simulation: Simulation,
    content: Arc<GameContent>,
    identity: Option<ServerIdentity>,
    client_keys: Vec<[u8; 32]>,
    require_client_identity: bool,
    require_content_attestation: bool,
    content_update_retry: Option<(Vec<u8>, u32, u8, u32)>,
    bot_policy_factory: Option<BotPolicyFactory>,
    bot_policy_registry: Vec<(&'static str, BotPolicyFactory)>,
    content_generation: u32,
    trusted_publisher: Option<[u8; 32]>,
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
    stats: ServerStats,
    phase: Phase,
    systems: Systems,
    frame: u64,
    match_number: u32,
    record_frames: Vec<TickInputs>,
    record_checkpoints: Vec<(u32, u64)>,
    pending_events: VecDeque<RawEvent>,
    results_log: Option<PathBuf>,
    results_logged: bool,
    recorded: bool,
    surrender_votes: [u16; 2],
    pending_surrender: Option<moba_game::Team>,
}

impl core::fmt::Debug for NetServer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NetServer")
            .field("phase", &self.phase)
            .field("clients", &self.clients.len())
            .finish_non_exhaustive()
    }
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
            require_content_attestation: false,
            content_update_retry: None,
            bot_policy_factory: None,
            bot_policy_registry: Vec::new(),
            content_generation: 0,
            trusted_publisher: None,
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
            stats: ServerStats::default(),
            phase: Phase::Lobby,
            systems: Systems::default(),
            frame: 0,
            match_number: 0,
            record_frames: Vec::new(),
            record_checkpoints: Vec::new(),
            pending_events: VecDeque::with_capacity(MAX_SNAPSHOT_EVENTS * 4),
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

    /// Removes a client identity key from the allowlist.
    ///
    /// Returns whether the key was present. Revoking the last key keeps the allowlist
    /// enforcing (all clients are then rejected) instead of silently disabling identity checks.
    pub fn revoke_client_key(&mut self, key: [u8; 32]) -> bool {
        let before = self.allowed_client_keys.len();
        self.allowed_client_keys.retain(|allowed| *allowed != key);
        before != self.allowed_client_keys.len()
    }

    /// Replaces a client identity key in the allowlist.
    ///
    /// Returns whether `old` was present. If `new` is already listed the old entry is simply
    /// removed.
    pub fn rotate_client_key(&mut self, old: [u8; 32], new: [u8; 32]) -> bool {
        let Some(index) = self.allowed_client_keys.iter().position(|key| *key == old) else {
            return false;
        };
        if self.allowed_client_keys.contains(&new) {
            self.allowed_client_keys.remove(index);
        } else {
            self.allowed_client_keys[index] = new;
        }
        // A rotated player can still reconnect to the slot they own.
        for client in self.clients.iter_mut().flatten() {
            if client.identity == Some(old) {
                client.identity = Some(new);
            }
        }
        true
    }

    /// Whether a key is currently allowed; intended for operator checks and tests.
    #[must_use]
    pub fn client_key_allowed(&self, key: [u8; 32]) -> bool {
        !self.require_client_identity || self.allowed_client_keys.contains(&key)
    }

    /// Number of allowed client identity keys.
    #[must_use]
    pub fn allowed_client_key_count(&self) -> usize {
        self.allowed_client_keys.len()
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

    /// Installs a bot-policy factory that is reapplied after every rematch.
    #[must_use]
    pub fn with_bot_policy<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Box<dyn BotPolicy> + Send + Sync + 'static,
    {
        let factory: BotPolicyFactory = Arc::new(factory);
        self.simulation.set_bot_policy(Some(factory()));
        self.bot_policy_factory = Some(Arc::clone(&factory));
        self.bot_policy_registry.push(("default", factory));
        self
    }

    /// Registers a named bot-policy factory; returns `false` for a duplicate id.
    pub fn register_bot_policy<F>(&mut self, id: &'static str, factory: F) -> bool
    where
        F: Fn() -> Box<dyn BotPolicy> + Send + Sync + 'static,
    {
        if self.bot_policy_registry.iter().any(|(name, _)| *name == id) {
            return false;
        }
        self.bot_policy_registry.push((id, Arc::new(factory)));
        true
    }

    /// Selects and immediately installs a registered bot policy.
    pub fn select_bot_policy(&mut self, id: &'static str) -> bool {
        let Some((_, factory)) = self
            .bot_policy_registry
            .iter()
            .find(|(name, _)| *name == id)
            .cloned()
        else {
            return false;
        };
        self.simulation.set_bot_policy(Some(factory()));
        self.bot_policy_factory = Some(factory);
        true
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
        if self.frame.is_multiple_of(30) {
            self.poll_content_reload();
        }
        match self.phase {
            Phase::Lobby => {
                for system in &mut self.systems.0 {
                    system(&mut self.simulation, ServerPhase::Lobby);
                }
            }
            Phase::Countdown(remaining) => {
                self.phase = if remaining <= 1 {
                    Phase::Playing
                } else {
                    Phase::Countdown(remaining - 1)
                };
                for system in &mut self.systems.0 {
                    system(&mut self.simulation, ServerPhase::Countdown);
                }
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
                for system in &mut self.systems.0 {
                    system(&mut self.simulation, ServerPhase::Results);
                }
                if remaining <= 1 {
                    self.reset_match();
                } else {
                    self.phase = Phase::Results(remaining - 1);
                }
            }
        }
        let interval = u64::from(self.config.snapshot_interval.max(1));
        if self.frame.is_multiple_of(interval) {
            self.send_snapshots();
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
