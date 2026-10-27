//! Transport poll loop and per-packet message dispatch.

use std::collections::VecDeque;
use std::io;
use std::time::Instant;

use moba_sim::{PlayerCommand, Simulation};

use crate::crypto::verify_hello_tag;
use crate::protocol::{
    FLAG_NEED_FULL, Header, PacketKind, WireCommand, decode_hello, decode_input,
    dequantize_position, sequence_after,
};
use crate::transport::ReceivedMessage;

use super::clients::{ClientConn, NetIdTable};
use super::match_flow::slot_team;
use super::{NetServer, Phase};

impl NetServer {
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

    #[allow(clippy::too_many_lines)] // One match arm per protocol message.
    #[allow(clippy::needless_pass_by_value)] // The message owns its payload.
    pub(super) fn handle_message(&mut self, message: ReceivedMessage) -> io::Result<()> {
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
                let keys_configured =
                    self.config.pre_shared_key.is_some() || !self.client_keys.is_empty();
                if self.require_client_identity
                    && (!hello.identity || !self.allowed_client_keys.contains(&hello.client_public))
                {
                    self.reject_client(message.addr, 0, 5)?;
                    return Ok(());
                }
                // Keyed and identity handshakes answer an address-bound challenge first, and
                // the Hello authenticator covers it, so a captured Hello is useless against
                // any other server or match that shares the same keys.
                if (keys_configured || self.require_client_identity)
                    && !self.challenge_is_pending(message.addr, hello.server_challenge)
                {
                    self.issue_challenge(message.addr)?;
                    return Ok(());
                }
                if self.require_client_identity
                    && !crate::crypto::verify_client_hello(
                        &hello.client_public,
                        hello.protocol,
                        hello.nonce,
                        hello.server_challenge,
                        hello.reconnect,
                        hello.spectate,
                        &hello.client_signature,
                    )
                {
                    let _ = self.take_challenge(message.addr, hello.server_challenge);
                    self.reject_client(message.addr, 0, 5)?;
                    return Ok(());
                }
                let matched_key = self.config.pre_shared_key.filter(|key| {
                    verify_hello_tag(
                        key,
                        hello.protocol,
                        hello.nonce,
                        hello.server_challenge,
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
                                hello.server_challenge,
                                hello.reconnect,
                                hello.spectate,
                                &hello.auth,
                            )
                        })
                        .copied()
                });
                if keys_configured || self.require_client_identity {
                    // One use per challenge, whether or not the Hello authenticated.
                    let _ = self.take_challenge(message.addr, hello.server_challenge);
                }
                if keys_configured && matched_key.is_none() {
                    self.reject_client(message.addr, 0, 4)?;
                    return Ok(());
                }
                // The credential this Hello proved; reconnects must present the same one.
                let identity = self.require_client_identity.then_some(hello.client_public);
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
                    if !self.clients[slot]
                        .as_ref()
                        .is_some_and(|client| client.owned_by(matched_key, identity))
                    {
                        // Another credential cannot re-key or take over this slot, even
                        // from a spoofed copy of the owner's address.
                        self.reject_client(message.addr, 0, 3)?;
                        return Ok(());
                    }
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
                    if self.clients[slot].as_ref().is_some_and(|client| {
                        client.connected || !client.owned_by(matched_key, identity)
                    }) {
                        self.reject_client(message.addr, 0, 3)?;
                        return Ok(());
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
                let session = self.new_session()?;
                let team_size = usize::from(self.content.rules.team_size);
                let team = if hello.spectate {
                    moba_game::Team::Blue
                } else {
                    slot_team(slot, team_size)
                };
                self.clients[slot] = Some(ClientConn {
                    addr: message.addr,
                    session,
                    key: matched_key,
                    identity,
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
            PacketKind::ContentAck => {
                let Some(index) = self.client_index_by_addr(message.addr) else {
                    return Ok(());
                };
                let client = self.clients[index].as_ref().expect("index is live");
                if message.header.session == client.session
                    && client.connected
                    && message.header.seq == self.content_generation
                {
                    self.content_update_retry = None;
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
}

pub(super) fn wire_to_command(
    client: &ClientConn,
    command: WireCommand,
    simulation: &mut Simulation,
) -> Option<PlayerCommand> {
    Some(match command {
        WireCommand::Stop => PlayerCommand::Stop,
        WireCommand::MoveTo { x, y } => PlayerCommand::MoveTo(dequantize_position(x, y)),
        WireCommand::Attack { net_id } => {
            let target = client.entity_for(net_id)?;
            if !simulation.is_visible_to(client.team, target) {
                return None;
            }
            PlayerCommand::Attack(target)
        }
        WireCommand::CastAbility { slot, x, y } => PlayerCommand::CastAbility {
            slot,
            target: dequantize_position(x, y),
        },
        WireCommand::BuyItem { item } => PlayerCommand::BuyItem(item),
        WireCommand::UseItem { slot } => PlayerCommand::UseItem(slot),
        WireCommand::Ready { .. } => return None,
        WireCommand::Surrender => PlayerCommand::Surrender,
    })
}
