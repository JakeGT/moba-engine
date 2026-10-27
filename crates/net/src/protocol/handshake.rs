//! Handshake payloads: [`Hello`] and [`Welcome`].

use super::WireError;
use super::bytes::{Reader, put_u16, put_u32};
use super::catalog::WireCatalog;
use super::catalog_codec::{decode_catalog, encode_catalog};

/// Handshake request payload.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    /// Client protocol version.
    pub protocol: u16,
    /// Random nonce echoed by the welcome for RTT/identification.
    pub nonce: u64,
    /// Session token to resume after a disconnect (`0` for a new connection).
    pub reconnect: u32,
    /// HMAC-SHA256 proof of the pre-shared key (all zero in plaintext mode).
    pub auth: [u8; 32],
    /// Request a read-only spectator connection.
    pub spectate: bool,
    /// Whether a client public key/signature is present.
    pub identity: bool,
    /// Client Ed25519 public key.
    pub client_public: [u8; 32],
    /// Signature over the server challenge.
    pub client_signature: [u8; 64],
    /// Server challenge being answered (`0` on the first attempt).
    pub server_challenge: u64,
}

/// Handshake response payload.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Welcome {
    /// Assigned player slot (`0..10`).
    pub slot: u8,
    /// Server tick rate.
    pub tick_rate: u16,
    /// Snapshots are sent every N ticks.
    pub snapshot_interval: u8,
    /// Current server tick.
    pub tick: u32,
    /// Match seed.
    pub seed: u64,
    /// Hero pick for this slot.
    pub hero_pick: u8,
    /// Client nonce echoed back.
    pub nonce: u64,
    /// Server handshake nonce (zero in plaintext mode).
    pub server_nonce: u64,
    /// HMAC-SHA256 proof of the pre-shared key (all zero in plaintext mode).
    pub auth: [u8; 32],
    /// This connection is a read-only spectator.
    pub spectator: bool,
    /// Complete runtime catalogue.
    pub catalog: WireCatalog,
    /// `GameContent::hash` of the catalogue (also authenticated by the Welcome HMAC).
    pub content_hash: u64,
    /// Whether a pinned server identity signature is present.
    pub identity: bool,
    /// Ed25519 server public key (zero when unsigned).
    pub server_public_key: [u8; 32],
    /// Ed25519 signature over the Welcome challenge.
    pub server_signature: [u8; 64],
}

/// Encodes a handshake request.
#[must_use]
pub fn encode_hello(hello: &Hello) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    put_u16(&mut out, hello.protocol);
    out.extend_from_slice(&hello.nonce.to_le_bytes());
    put_u32(&mut out, hello.reconnect);
    out.extend_from_slice(&hello.auth);
    out.push(u8::from(hello.spectate));
    out.push(u8::from(hello.identity));
    out.extend_from_slice(&hello.client_public);
    out.extend_from_slice(&hello.client_signature);
    out.extend_from_slice(&hello.server_challenge.to_le_bytes());
    out
}

/// Decodes a handshake request.
pub fn decode_hello(data: &[u8]) -> Result<Hello, WireError> {
    let mut reader = Reader::new(data);
    let hello = Hello {
        protocol: reader.u16()?,
        nonce: u64::from_le_bytes(
            reader
                .bytes(8)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        ),
        reconnect: reader.u32()?,
        auth: reader
            .bytes(32)?
            .try_into()
            .map_err(|_| WireError::Truncated)?,
        spectate: reader.u8()? != 0,
        identity: reader.u8()? != 0,
        client_public: reader
            .bytes(32)?
            .try_into()
            .map_err(|_| WireError::Truncated)?,
        client_signature: reader
            .bytes(64)?
            .try_into()
            .map_err(|_| WireError::Truncated)?,
        server_challenge: u64::from_le_bytes(
            reader
                .bytes(8)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        ),
    };
    if reader.remaining() != 0 {
        return Err(WireError::TrailingBytes);
    }
    Ok(hello)
}

/// Encodes a handshake response.
#[must_use]
pub fn encode_welcome(welcome: &Welcome) -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.push(welcome.slot);
    put_u16(&mut out, welcome.tick_rate);
    out.push(welcome.snapshot_interval);
    put_u32(&mut out, welcome.tick);
    out.extend_from_slice(&welcome.seed.to_le_bytes());
    out.push(welcome.hero_pick);
    out.extend_from_slice(&welcome.nonce.to_le_bytes());
    out.extend_from_slice(&welcome.server_nonce.to_le_bytes());
    out.extend_from_slice(&welcome.auth);
    out.push(u8::from(welcome.spectator));
    encode_catalog(&welcome.catalog, &mut out);
    out.extend_from_slice(&welcome.content_hash.to_le_bytes());
    out.push(u8::from(welcome.identity));
    out.extend_from_slice(&welcome.server_public_key);
    out.extend_from_slice(&welcome.server_signature);
    out
}

/// Decodes a handshake response.
#[allow(clippy::too_many_lines)] // Wire field order mirrors the encoder.
pub fn decode_welcome(data: &[u8]) -> Result<Welcome, WireError> {
    let mut reader = Reader::new(data);
    let welcome = Welcome {
        slot: reader.u8()?,
        tick_rate: reader.u16()?,
        snapshot_interval: reader.u8()?,
        tick: reader.u32()?,
        seed: u64::from_le_bytes(
            reader
                .bytes(8)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        ),
        hero_pick: reader.u8()?,
        nonce: u64::from_le_bytes(
            reader
                .bytes(8)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        ),
        server_nonce: u64::from_le_bytes(
            reader
                .bytes(8)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        ),
        auth: reader
            .bytes(32)?
            .try_into()
            .map_err(|_| WireError::Truncated)?,
        spectator: reader.u8()? != 0,
        catalog: decode_catalog(&mut reader)?,
        content_hash: u64::from_le_bytes(
            reader
                .bytes(8)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        ),
        identity: reader.u8()? != 0,
        server_public_key: reader
            .bytes(32)?
            .try_into()
            .map_err(|_| WireError::Truncated)?,
        server_signature: reader
            .bytes(64)?
            .try_into()
            .map_err(|_| WireError::Truncated)?,
    };
    if reader.remaining() != 0 {
        return Err(WireError::TrailingBytes);
    }
    Ok(welcome)
}
