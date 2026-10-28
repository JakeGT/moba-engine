//! Per-client snapshot building and sending.

use std::net::SocketAddr;

use crate::protocol::{
    Header, MAX_SNAPSHOT_EVENTS, NetEntity, OUTCOME_BLUE, OUTCOME_DRAW, OUTCOME_NONE, OUTCOME_RED,
    PacketKind, build_delta_into, encode_snapshot,
};

use moba_sim::MatchOutcome;

use super::NetServer;

/// Snapshot `outcome` code (`OUTCOME_*`) for a simulation outcome.
pub(super) const fn outcome_code(outcome: Option<MatchOutcome>) -> u8 {
    match outcome {
        Some(MatchOutcome::Winner(moba_game::Team::Blue)) => OUTCOME_BLUE,
        Some(MatchOutcome::Winner(moba_game::Team::Red)) => OUTCOME_RED,
        Some(MatchOutcome::Draw) => OUTCOME_DRAW,
        // No outcome yet. `MatchOutcome` is `#[non_exhaustive]`; an outcome without a wire
        // code would also report none (none exists today).
        _ => OUTCOME_NONE,
    }
}

impl NetServer {
    #[allow(clippy::too_many_lines)] // Snapshot fan-out is one ordered pass.
    pub(super) fn send_snapshots(&mut self) {
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
        let pending_attacks = core::mem::take(&mut self.pending_attacks);
        let mut delta = core::mem::take(&mut self.delta);
        let mut ready_mask = 0u16;
        let mut player_mask = 0u16;
        for slot in 0..self.config.max_clients.min(self.clients.len()) {
            if let Some(player) = self.clients[slot].as_ref()
                && player.connected
                && !player.spectator
            {
                player_mask |= 1 << slot;
                if player.ready {
                    ready_mask |= 1 << slot;
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
                if let Some(cooldowns) = self
                    .simulation
                    .world()
                    .get::<moba_sim::ItemCooldowns>(unit.entity)
                {
                    entity.progress.item_cooldowns = cooldowns.0;
                }
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
            delta.outcome = outcome_code(self.simulation.outcome());
            delta.phase = phase_code as u8;
            delta.countdown = countdown;
            delta.ready_mask = ready_mask;
            delta.player_mask = player_mask;
            let viewer_team = client.team;
            let spectator = client.spectator;
            delta.events.clear();
            // Highest priority first (deaths, then other events, then hero attacks, then
            // other attacks) until the snapshot is full; arrival order within a priority.
            for priority in 0..=3 {
                let queue = if priority < 2 {
                    &pending_events
                } else {
                    &pending_attacks
                };
                for event in queue
                    .iter()
                    .filter(|event| event.priority() == priority)
                    .filter(|event| event.visible_to(viewer_team, spectator, units))
                    .take(MAX_SNAPSHOT_EVENTS.saturating_sub(delta.events.len()))
                {
                    delta.events.push(event.to_net(client, tick));
                }
            }
            // A failure for one client must not starve the clients after it (or drop the
            // scratch buffers taken above), so count it and move on.
            if encode_snapshot(&delta, &mut payload).is_err() {
                self.stats.snapshot_send_errors += 1;
                continue;
            }
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
            if self.transport.send(client.addr, &header, &payload).is_err() {
                self.stats.snapshot_send_errors += 1;
                // Keep the previous baseline: the next snapshot is a full one.
                client.need_full = true;
                continue;
            }
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
        self.pending_events = pending_events;
        self.pending_events.clear();
        self.pending_attacks = pending_attacks;
        self.pending_attacks.clear();
        if let Some((payload, wait, remaining, generation)) = self.content_update_retry.take() {
            let (wait, remaining) = if wait == 0 {
                let targets: Vec<(SocketAddr, u32)> = self
                    .clients
                    .iter()
                    .flatten()
                    .filter(|client| client.connected)
                    .map(|client| (client.addr, client.session))
                    .collect();
                for (addr, session) in targets {
                    let header = Header {
                        kind: PacketKind::ContentUpdate,
                        flags: 0,
                        session,
                        seq: generation,
                        ack: 0,
                        ack_bits: 0,
                    };
                    let _ = self.transport.send(addr, &header, &payload);
                }
                (45, remaining.saturating_sub(1))
            } else {
                (wait - 1, remaining)
            };
            if remaining > 0 {
                self.content_update_retry = Some((payload, wait, remaining, generation));
            }
        }
    }
}
