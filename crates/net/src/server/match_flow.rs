//! Match phases: simulation ticks, ready/countdown, surrender, reset and slot upkeep.

use std::sync::Arc;
use std::time::Instant;

use moba_sim::{PlayerCommand, Simulation, TickInputs};

use crate::protocol::{Header, MAX_SNAPSHOT_EVENTS, PacketKind};

use super::events::RawEvent;
use super::ingress::wire_to_command;
use super::{NetServer, Phase};

impl NetServer {
    pub(super) fn tick_simulation(&mut self) {
        let mut inputs: TickInputs = self.simulation.bot_inputs();
        for slot in 0..self.config.max_clients.min(self.clients.len()) {
            let Some(client) = self.clients[slot].as_mut() else {
                continue;
            };
            if let Some(wire) = client.command.take()
                && let Some(command) = wire_to_command(client, wire, &mut self.simulation)
            {
                inputs.set(slot, command);
            }
            if let Some(item) = client.pending_purchase.take() {
                inputs.set_purchase(slot, item);
            }
        }
        if let Some(team) = self.pending_surrender.take() {
            for slot in 0..self.config.max_clients.min(self.clients.len()) {
                if let Some(client) = self.clients[slot].as_ref()
                    && client.connected
                    && !client.spectator
                    && client.team == team
                {
                    inputs.set(slot, PlayerCommand::Surrender);
                    break;
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
                let queue = if raw.is_attack() {
                    &mut self.pending_attacks
                } else {
                    &mut self.pending_events
                };
                if queue.len() >= MAX_SNAPSHOT_EVENTS * 4 {
                    queue.pop_front();
                }
                queue.push_back(raw);
            }
        }
        self.simulation.clear_events();
        if self.config.record.is_some()
            && self.record_frames.len().is_multiple_of(150)
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

    pub(super) fn maybe_start_countdown(&mut self) {
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

    pub(super) fn set_ready(&mut self, slot: usize, ready: bool) {
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

    pub(super) fn cast_surrender_vote(&mut self, slot: usize) {
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
            if let Some(player) = self.clients[index].as_ref()
                && player.connected
                && !player.spectator
                && player.team == team
            {
                connected |= 1 << index;
            }
        }
        if connected != 0 && self.surrender_votes[team.index()] & connected == connected {
            self.pending_surrender = Some(team);
        }
    }

    pub(super) fn reset_match(&mut self) {
        self.content_update_retry = None;
        self.match_number = self.match_number.wrapping_add(1);
        self.record_frames.clear();
        self.record_checkpoints.clear();
        self.recorded = false;
        self.results_logged = false;
        let mut config = self.config.sim;
        config.seed = config.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.config.sim = config;
        let rotated = self.rotate_content();
        self.reassign_teams();
        self.simulation = Simulation::new_with_content(config, Arc::clone(&self.content));
        if let Some(factory) = &self.bot_policy_factory {
            self.simulation.set_bot_policy(Some(factory()));
        }
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
        if rotated {
            self.broadcast_content_update();
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

    pub(super) fn expire_clients(&mut self) {
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

    pub(super) fn reassign_teams(&mut self) {
        let team_size = usize::from(self.content.rules.team_size);
        for slot in 0..self.config.max_clients.min(self.clients.len()) {
            let Some(client) = self.clients[slot].as_mut() else {
                continue;
            };
            if client.spectator {
                continue;
            }
            let team = slot_team(slot, team_size);
            if client.team != team {
                client.team = team;
                client.need_full = true;
                client.clear_history();
                client.net_ids.clear();
            }
        }
    }

    pub(super) fn refresh_bot_mask(&mut self) {
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
}

/// Deterministic team for an active player slot.
pub(super) fn slot_team(slot: usize, team_size: usize) -> moba_game::Team {
    if slot < team_size {
        moba_game::Team::Blue
    } else {
        moba_game::Team::Red
    }
}
