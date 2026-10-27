//! Handshake state: challenges, nonce replay protection, Welcome and reconnects.

use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::crypto::{
    SessionCrypto, derive_session_key, random_nonce, sign_server_welcome, welcome_tag,
};
use crate::protocol::{Header, PacketKind, Welcome, WireCatalog, encode_welcome};

use super::{NetServer, SERVER_TICK_RATE};

#[derive(Debug)]
struct HandshakeNonce {
    key: [u8; 32],
    nonce: u64,
    addr: SocketAddr,
    seen: Instant,
}

#[derive(Debug)]
pub(super) struct ClientChallenge {
    pub(super) addr: SocketAddr,
    pub(super) nonce: u64,
    pub(super) seen: Instant,
}

#[derive(Debug)]
pub(super) struct NonceTracker {
    entries: Vec<HandshakeNonce>,
    window: Duration,
    max_entries: usize,
}

impl NonceTracker {
    pub(super) fn new(window: Duration, max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            window,
            max_entries,
        }
    }

    pub(super) fn allow(
        &mut self,
        key: [u8; 32],
        nonce: u64,
        addr: SocketAddr,
        now: Instant,
    ) -> bool {
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

impl NetServer {
    /// A random session id that no current slot uses.
    pub(super) fn new_session(&self) -> io::Result<u32> {
        loop {
            let session = crate::crypto::random_session()?;
            if !self
                .clients
                .iter()
                .flatten()
                .any(|client| client.session == session)
            {
                return Ok(session);
            }
        }
    }

    pub(super) fn issue_challenge(&mut self, addr: SocketAddr) -> io::Result<()> {
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

    pub(super) fn take_challenge(&mut self, addr: SocketAddr, nonce: u64) -> bool {
        if nonce == 0 {
            return false;
        }
        let before = self.client_challenges.len();
        self.client_challenges
            .retain(|challenge| !(challenge.addr == addr && challenge.nonce == nonce));
        before != self.client_challenges.len()
    }

    pub(super) fn challenge_is_pending(&self, addr: SocketAddr, nonce: u64) -> bool {
        self.client_challenges.iter().any(|challenge| {
            challenge.addr == addr
                && challenge.nonce == nonce
                && challenge.seen.elapsed() <= Duration::from_secs(10)
        })
    }

    pub(super) fn reconnect_client(
        &mut self,
        slot: usize,
        addr: SocketAddr,
        nonce: u64,
    ) -> io::Result<()> {
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

    /// Rejects a Hello.
    ///
    /// Always plaintext: a Hello's source address is unauthenticated, and sealing the reply
    /// with a key already installed for that address would turn one spoofed Hello into a
    /// genuine, authenticated Disconnect for whoever owns the address. Connected clients drop
    /// unsealed application packets, so only a client that is still handshaking acts on it.
    pub(super) fn reject_client(
        &mut self,
        addr: SocketAddr,
        session: u32,
        reason: u8,
    ) -> io::Result<()> {
        let header = Header {
            kind: PacketKind::Disconnect,
            flags: 0,
            session,
            seq: 0,
            ack: 0,
            ack_bits: 0,
        };
        self.transport.send_plaintext(addr, &header, &[reason])
    }

    #[allow(clippy::too_many_lines)] // Handshake catalogue assembly is one linear frame.
    pub(super) fn send_welcome(
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

    pub(super) fn free_slot(&self) -> Option<usize> {
        let active = usize::from(self.content.rules.team_size) * 2;
        (0..self.config.max_clients.min(self.clients.len()).min(active))
            .find(|slot| self.clients[*slot].is_none())
    }

    pub(super) fn free_spectator_slot(&self) -> Option<usize> {
        (self.config.max_clients.min(self.clients.len())..self.clients.len())
            .find(|slot| self.clients[*slot].is_none())
    }

    pub(super) fn client_index_by_addr(&self, addr: SocketAddr) -> Option<usize> {
        self.clients
            .iter()
            .position(|client| client.as_ref().is_some_and(|client| client.addr == addr))
    }
}
