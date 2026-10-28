//! Per-connection state: net-id table and snapshot baseline history.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Instant;

use crate::protocol::{NetEntity, NetId, WireCommand};

use super::SNAPSHOT_HISTORY;

#[derive(Debug)]
pub(super) struct SnapshotRecord {
    seq: u32,
    entities: Vec<NetEntity>,
}

#[derive(Debug)]
pub(super) struct NetIdTable {
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
    pub(super) fn get_or_insert(&mut self, entity: moba_ecs::Entity) -> NetId {
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

    /// The net id `entity` already has, without assigning one.
    pub(super) fn get(&self, entity: moba_ecs::Entity) -> Option<NetId> {
        self.entries
            .get(entity.index() as usize)
            .copied()
            .flatten()
            .filter(|(known, _)| *known == entity)
            .map(|(_, id)| id)
    }

    pub(super) fn entity_for(&self, net_id: NetId) -> Option<moba_ecs::Entity> {
        self.entries
            .iter()
            .flatten()
            .find(|(_, id)| *id == net_id)
            .map(|(entity, _)| *entity)
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.next = 1;
    }
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // Connection flags are independent state.
pub(super) struct ClientConn {
    pub(super) addr: SocketAddr,
    pub(super) session: u32,
    pub(super) slot: u8,
    /// Token/PSK the owner authenticated with (`None` in open LAN mode).
    pub(super) key: Option<[u8; 32]>,
    /// Client identity public key the owner proved, when identities are required.
    pub(super) identity: Option<[u8; 32]>,
    pub(super) spectator: bool,
    pub(super) team: moba_game::Team,
    pub(super) connected: bool,
    pub(super) ready: bool,
    pub(super) last_seen: Instant,
    pub(super) disconnected_at: Option<Instant>,
    pub(super) last_input_seq: u32,
    pub(super) command: Option<WireCommand>,
    pub(super) pending_purchase: Option<u8>,
    pub(super) net_ids: NetIdTable,
    pub(super) snapshot_seq: u32,
    pub(super) acked_snapshot: u32,
    pub(super) need_full: bool,
    pub(super) history: VecDeque<SnapshotRecord>,
    pub(super) history_pool: Vec<Vec<NetEntity>>,
    pub(super) welcome_nonce: u64,
    pub(super) welcome_payload: Vec<u8>,
    pub(super) bytes_sent: u64,
}

impl ClientConn {
    /// Whether a Hello authenticated with `key`/`identity` may act for this slot.
    pub(super) fn owned_by(&self, key: Option<[u8; 32]>, identity: Option<[u8; 32]>) -> bool {
        self.key == key && self.identity == identity
    }

    pub(super) fn net_id_for(&mut self, entity: moba_ecs::Entity) -> NetId {
        self.net_ids.get_or_insert(entity)
    }

    pub(super) fn entity_for(&self, net_id: NetId) -> Option<moba_ecs::Entity> {
        self.net_ids.entity_for(net_id)
    }

    /// Whether `entity` was in the last snapshot sent to this client. A unit killed this
    /// tick is despawned (or, an enemy hero, no longer visible) before the snapshot that
    /// carries its death, but the client knew it a snapshot ago. Never allocates.
    pub(super) fn knew(&self, entity: moba_ecs::Entity) -> bool {
        let Some(net_id) = self.net_ids.get(entity) else {
            return false;
        };
        self.history.back().is_some_and(|record| {
            record
                .entities
                .binary_search_by_key(&net_id, |entity| entity.net_id)
                .is_ok()
        })
    }

    pub(super) fn baseline(&self, seq: u32) -> Option<&[NetEntity]> {
        self.history
            .iter()
            .find(|record| record.seq == seq)
            .map(|record| record.entities.as_slice())
    }

    pub(super) fn record(&mut self, seq: u32, entities: &[NetEntity]) {
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

    pub(super) fn clear_history(&mut self) {
        while let Some(record) = self.history.pop_front() {
            let mut entities = record.entities;
            entities.clear();
            self.history_pool.push(entities);
        }
    }
}
