//! Authoritative UDP server: handshake, player slots and snapshot broadcasting.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moba_sim::{ObservedUnit, PlayerCommand, SimConfig, Simulation, TickInputs};

use crate::protocol::{
    FLAG_NEED_FULL, Header, InputMessage, MAX_DATAGRAM, NetEntity, NetId, PacketKind,
    SnapshotDelta, SnapshotState, Welcome, WireCommand, build_delta, decode_hello, decode_input,
    dequantize_position, encode_snapshot, encode_welcome, sequence_after,
};
use crate::transport::{ReceivedMessage, UdpTransport};

/// How long a silent client is kept before it is dropped and its slot reverts to a bot.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
/// Snapshot history per client, used as delta baselines.
const SNAPSHOT_HISTORY: usize = 64;
/// Server tick rate (must match the simulation).
pub const SERVER_TICK_RATE: u16 = 30;

/// Server configuration.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// UDP bind address.
    pub bind: SocketAddr,
    /// Simulation configuration (bots fill empty slots).
    pub sim: SimConfig,
    /// Send a snapshot every N ticks.
    pub snapshot_interval: u8,
    /// Maximum connected clients.
    pub max_clients: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:7777".parse().expect("valid default address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
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
    last_seen: Instant,
    last_input_seq: u32,
    command: Option<WireCommand>,
    net_ids: Vec<(moba_ecs::Entity, NetId)>,
    next_net_id: NetId,
    snapshot_seq: u32,
    acked_snapshot: u32,
    need_full: bool,
    history: VecDeque<SnapshotRecord>,
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

    /// Current counters.
    #[must_use]
    pub fn stats(&self) -> ServerStats {
        let mut stats = self.stats;
        stats.clients = self.clients.iter().flatten().count();
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

    /// Advances the simulation and broadcasts snapshots.
    ///
    /// # Errors
    /// Returns OS send errors from snapshot transmission.
    pub fn tick(&mut self) -> io::Result<()> {
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
        self.stats.ticks += 1;
        let interval = u32::from(self.config.snapshot_interval.max(1));
        if self.simulation.current_tick().get() % interval == 0 {
            self.send_snapshots()?;
        }
        Ok(())
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
            let expired = self.clients[slot]
                .as_ref()
                .is_some_and(|client| now.duration_since(client.last_seen) > CLIENT_TIMEOUT);
            if expired {
                if let Some(client) = self.clients[slot].take() {
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
            }
        }
        self.refresh_bot_mask();
    }

    fn refresh_bot_mask(&mut self) {
        let mut mask = 0x03FFu16;
        for (slot, client) in self.clients.iter().enumerate() {
            if client.is_some() {
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
                if let Some(slot) = self.client_index_by_addr(message.addr) {
                    let client = self.clients[slot].as_mut().expect("index is live");
                    client.last_seen = Instant::now();
                    let session = client.session;
                    self.send_welcome(message.addr, slot, hello.nonce, session)?;
                    return Ok(());
                }
                let Some(slot) = self.free_slot() else {
                    let header = Header {
                        kind: PacketKind::Disconnect,
                        flags: 0,
                        session: 0,
                        seq: 0,
                        ack: 0,
                        ack_bits: 0,
                    };
                    let _ = self.transport.send(message.addr, &header, &[2]);
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
                    last_seen: Instant::now(),
                    last_input_seq: 0,
                    command: None,
                    net_ids: Vec::new(),
                    next_net_id: 1,
                    snapshot_seq: 0,
                    acked_snapshot: 0,
                    need_full: true,
                    history: VecDeque::new(),
                    bytes_sent: 0,
                });
                self.refresh_bot_mask();
                self.send_welcome(message.addr, slot, hello.nonce, session)?;
            }
            PacketKind::Input => {
                let Some(index) = self.client_index_by_addr(message.addr) else {
                    return Ok(());
                };
                let client = self.clients[index].as_mut().expect("index is live");
                if message.header.session != client.session {
                    return Ok(());
                }
                let Ok(input) = decode_input(&message.payload) else {
                    return Ok(());
                };
                client.last_seen = Instant::now();
                if sequence_after(input.input_seq, client.last_input_seq) {
                    client.last_input_seq = input.input_seq;
                    client.command = Some(input.command);
                }
            }
            PacketKind::SnapshotAck => {
                let Some(index) = self.client_index_by_addr(message.addr) else {
                    return Ok(());
                };
                let client = self.clients[index].as_mut().expect("index is live");
                if message.header.session != client.session {
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
                    if message.header.session == client.session {
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
                    self.clients[index] = None;
                    self.refresh_bot_mask();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn send_welcome(
        &mut self,
        addr: SocketAddr,
        slot: usize,
        nonce: u64,
        session: u32,
    ) -> io::Result<()> {
        let config = self.config.sim;
        let welcome = Welcome {
            slot: u8::try_from(slot).expect("slot fits u8"),
            tick_rate: SERVER_TICK_RATE,
            snapshot_interval: self.config.snapshot_interval,
            tick: self.simulation.current_tick().get(),
            seed: config.seed,
            hero_pick: config.hero_picks[slot],
            nonce,
        };
        let payload = encode_welcome(&welcome);
        let header = Header {
            kind: PacketKind::Welcome,
            flags: 0,
            session,
            seq: 0,
            ack: 0,
            ack_bits: 0,
        };
        self.transport.send(addr, &header, &payload)
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
        let mut current = core::mem::take(&mut self.current);
        let mut payload = core::mem::take(&mut self.payload);
        for index in 0..self.clients.len() {
            let Some(client) = self.clients[index].as_mut() else {
                continue;
            };
            current.clear();
            current.extend(
                observed[client.team.index()]
                    .iter()
                    .map(|unit| NetEntity::from_observed(client.net_id_for(unit.entity), unit)),
            );
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
