//! Content pack watching, rotation, trust checks and `ContentUpdate` broadcast.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use moba_game::GameContent;
use moba_sim::Simulation;

use crate::protocol::{Header, PacketKind, WireCatalog, encode_content_update};

use super::{NetServer, Phase};

#[derive(Debug)]
pub(super) struct ContentReload {
    path: PathBuf,
    modified: Option<SystemTime>,
}

impl NetServer {
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

    /// Configures the trust policy used for startup/rotation/reload packs.
    pub fn set_content_trust(&mut self, require_attestation: bool, publisher: Option<[u8; 32]>) {
        self.require_content_attestation = require_attestation;
        self.trusted_publisher = publisher;
    }

    fn content_trusted(&self, content: &GameContent) -> bool {
        if !self.require_content_attestation {
            return true;
        }
        let Some(attestation) = content.attestation.as_ref() else {
            return false;
        };
        if self
            .trusted_publisher
            .is_some_and(|publisher| publisher != attestation.publisher)
        {
            return false;
        }
        crate::crypto::verify_pack(content)
    }

    pub(super) fn rotate_content(&mut self) -> bool {
        self.content_update_retry = None;
        if self.rotation.len() < 2 {
            return false;
        }
        let next = (self.rotation_index + 1) % self.rotation.len();
        let path = self.rotation[next].clone();
        match moba_game::load_content_file(&path) {
            Ok(content) if !self.content_trusted(&content) => {
                eprintln!("map rotation rejected: pack trust policy failed");
                false
            }
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
                true
            }
            Err(error) => {
                eprintln!("map rotation failed for {}: {error}", path.display());
                false
            }
        }
    }

    pub(super) fn broadcast_content_update(&mut self) {
        self.content_generation = self.content_generation.wrapping_add(1);
        let generation = self.content_generation;
        if let Phase::Results(remaining) = self.phase
            && remaining < 180
        {
            // Leave enough time for the update/retries to reach every client.
            self.phase = Phase::Results(180);
        }
        let hash = self.content.hash();
        let catalog = WireCatalog::from(&*self.content);
        let mut payload = Vec::new();
        encode_content_update(&catalog, hash, &mut payload);
        let targets: Vec<(SocketAddr, u32)> = self
            .clients
            .iter()
            .flatten()
            .filter(|client| client.connected)
            .map(|client| (client.addr, client.session))
            .collect();
        for client in self.clients.iter_mut().flatten() {
            client.clear_history();
            client.net_ids.clear();
            client.acked_snapshot = 0;
            client.need_full = true;
            client.command = None;
            client.pending_purchase = None;
            client.last_input_seq = 0;
        }
        for (addr, session) in targets {
            let header = Header {
                kind: PacketKind::ContentUpdate,
                flags: 0,
                session,
                seq: generation,
                ack: 0,
                ack_bits: 0,
            };
            if let Err(error) = self.transport.send(addr, &header, &payload) {
                eprintln!("content update send failed for {addr}: {error}");
            }
        }
        // One send plus three retries at 1.5 s intervals; clients deduplicate by hash.
        self.content_update_retry = Some((payload, 30, 3, generation));
    }

    pub(super) fn poll_content_reload(&mut self) {
        let Some(reload) = self.reload.as_mut() else {
            return;
        };
        if !matches!(self.phase, Phase::Lobby | Phase::Results(_)) {
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
            Ok(content) if !self.content_trusted(&content) => {
                eprintln!("content reload rejected: pack trust policy failed");
            }
            Ok(content) => {
                let active = usize::from(content.rules.team_size) * 2;
                if self.clients.iter().enumerate().any(|(slot, client)| {
                    client
                        .as_ref()
                        .is_some_and(|client| !client.spectator && slot >= active)
                }) {
                    eprintln!(
                        "content reload rejected: team_size {} cannot serve occupied slots",
                        content.rules.team_size
                    );
                    return;
                }
                let content = Arc::new(content);
                self.simulation =
                    Simulation::new_with_content(self.config.sim, Arc::clone(&content));
                self.content = content;
                self.reassign_teams();
                self.record_frames.clear();
                self.recorded = false;
                self.broadcast_content_update();
                eprintln!(
                    "content reloaded and broadcast: {} items",
                    self.content.items.len()
                );
            }
            Err(error) => eprintln!("content reload failed: {error}"),
        }
    }
}
