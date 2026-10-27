//! Wire format: packet header, commands and delta-compressed snapshots.
//!
//! All integers are little-endian and every decoder is bounds-checked. Packets are capped
//! at [`MAX_DATAGRAM`] bytes (fits a typical Ethernet MTU without IP fragmentation).
//! Large messages are split by the transport into fragments.
//!
//! Source layout (private submodules, all items re-exported here): this file holds the
//! packet header, framing and shared constants; `bytes` the little-endian reader/writer
//! helpers; `handshake` `Hello`/`Welcome`; `catalog` the `Wire*` catalogue types, with
//! `catalog_from` (`GameContent` -> wire), `catalog_to` (wire -> `GameContent`) and
//! `catalog_codec` (catalogue bytes); `input` commands; `snapshot` entity state and
//! delta building; `snapshot_codec` snapshot bytes.

use core::fmt;

use moba_core::{Fix, FixVec2};

use bytes::{Reader, put_u16, put_u32};

mod bytes;
mod catalog;
mod catalog_codec;
mod catalog_from;
mod catalog_to;
mod handshake;
mod input;
mod snapshot;
mod snapshot_codec;
#[cfg(test)]
mod tests;

pub use catalog::{
    MAX_WIRE_ABILITIES, MAX_WIRE_AREA_EFFECTS, MAX_WIRE_EFFECTS, MAX_WIRE_HEROES, MAX_WIRE_ITEMS,
    MAX_WIRE_LANE_POINTS, MAX_WIRE_LANES, MAX_WIRE_MODIFIERS, MAX_WIRE_STRUCTURES, MAX_WIRE_WALLS,
    WIRE_ABILITY_NAME, WIRE_HERO_NAME, WIRE_ITEM_NAME, WIRE_LANE_NAME, WIRE_MANIFEST_FIELD,
    WIRE_MODIFIER_NAME, WireAbility, WireAreaEffect, WireCatalog, WireEffect, WireHero, WireItem,
    WireLane, WireMap, WireMinionStats, WireModifier, WireRewards, WireRules, WireStructure,
    WireStructureStats, WireUnitRewards, WireWall,
};
pub use catalog_codec::{decode_content_update, encode_content_update};
pub use handshake::{Hello, Welcome, decode_hello, decode_welcome, encode_hello, encode_welcome};
pub use input::{InputMessage, WireCommand, decode_input, encode_input};
pub use snapshot::{
    ENTITY_FLAG_BURN, ENTITY_FLAG_DEAD, ENTITY_FLAG_HASTE, ENTITY_FLAG_ROOT, ENTITY_FLAG_SHIELD,
    ENTITY_FLAG_SILENCE, ENTITY_FLAG_SLOW, EVENT_CAST, EVENT_MATCH_ENDED, EVENT_PROJECTILE_HIT,
    EVENT_PROJECTILE_SPAWN, EVENT_STRUCTURE_DESTROYED, EVENT_UNIT_DIED, EVENT_ZONE_SPAWNED,
    MATCH_ENDED_DRAW, MAX_SNAPSHOT_EVENTS, MatchPhase, NetEntity, NetEvent, NetId, NetProgress,
    NetUpdate, OUTCOME_BLUE, OUTCOME_DRAW, OUTCOME_NONE, OUTCOME_RED, STATUS_BURN, STATUS_DEAD,
    STATUS_HASTE, STATUS_ROOT, STATUS_SHIELD, STATUS_SILENCE, STATUS_SLOW, SnapshotDelta,
    SnapshotState, build_delta, build_delta_into,
};
pub use snapshot_codec::{decode_snapshot, encode_snapshot};

/// Datagram magic (`"MOBA"` little-endian).
pub const MAGIC: u32 = 0x4D4F_4241;
/// Wire protocol version.
pub const PROTOCOL_VERSION: u8 = 39;
/// Fixed packet header size.
pub const HEADER_LEN: usize = 26;
/// Maximum datagram size, including the header.
pub const MAX_DATAGRAM: usize = 1_200;
/// Maximum application payload per datagram.
pub const MAX_PAYLOAD: usize = MAX_DATAGRAM - HEADER_LEN;
/// Position quantisation: 1/16 world unit.
pub const POSITION_SCALE: i32 = 16;
/// Highest quantised coordinate for a 256-unit map at 1/16 precision.
pub const MAX_QUANTIZED_POSITION: u16 = 4095;

/// Packet type in the fixed header.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketKind {
    /// Client -> server connection request.
    Hello = 1,
    /// Server -> client connection acceptance.
    Welcome = 2,
    /// Client -> server intent.
    Input = 3,
    /// Server -> client state snapshot.
    Snapshot = 4,
    /// Client -> server snapshot acknowledgement.
    SnapshotAck = 5,
    /// Round-trip probe.
    Ping = 6,
    /// Round-trip reply.
    Pong = 7,
    /// Either side requests connection teardown.
    Disconnect = 8,
    /// Transport-level fragment of another message.
    Fragment = 9,
    /// Server -> client identity challenge.
    Challenge = 10,
    /// Server -> client full catalogue update between matches.
    ContentUpdate = 11,
    /// Client -> server acknowledgement of a catalogue update.
    ContentAck = 12,
}

impl PacketKind {
    /// Decodes a kind byte.
    pub const fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::Hello,
            2 => Self::Welcome,
            3 => Self::Input,
            4 => Self::Snapshot,
            5 => Self::SnapshotAck,
            6 => Self::Ping,
            7 => Self::Pong,
            8 => Self::Disconnect,
            9 => Self::Fragment,
            10 => Self::Challenge,
            11 => Self::ContentUpdate,
            12 => Self::ContentAck,
            _ => return None,
        })
    }
}

/// Set when a client cannot apply a delta and needs a full snapshot.
pub const FLAG_NEED_FULL: u16 = 1;
/// Payload is AEAD-sealed (optional PSK mode).
pub const FLAG_ENCRYPTED: u16 = 1 << 1;

/// Fixed packet header.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// Message type.
    pub kind: PacketKind,
    /// Message flags.
    pub flags: u16,
    /// Connection token (zero during the handshake).
    pub session: u32,
    /// Sender sequence number.
    pub seq: u32,
    /// Highest sequence received from the peer.
    pub ack: u32,
    /// Bitmask of the 32 sequence numbers before `ack`.
    pub ack_bits: u32,
}

/// Wire errors; all decoders are total and return this on bad input.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    /// Not enough bytes.
    Truncated,
    /// Wrong magic.
    BadMagic,
    /// Unsupported protocol version.
    BadVersion,
    /// Unknown packet kind.
    BadKind,
    /// Payload length does not match the datagram.
    BadLength,
    /// Payload exceeds the datagram budget.
    TooLarge,
    /// Trailing bytes after the declared payload.
    TrailingBytes,
    /// Invalid command tag or value.
    BadValue,
    /// Too many entities/updates for one message.
    TooMany,
    /// Delta references an entity that the baseline does not contain.
    MissingBaseline,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated packet"),
            Self::BadMagic => f.write_str("bad packet magic"),
            Self::BadVersion => f.write_str("unsupported protocol version"),
            Self::BadKind => f.write_str("unknown packet kind"),
            Self::BadLength => f.write_str("bad payload length"),
            Self::TooLarge => f.write_str("message too large"),
            Self::TrailingBytes => f.write_str("trailing bytes"),
            Self::BadValue => f.write_str("invalid wire value"),
            Self::TooMany => f.write_str("too many entities"),
            Self::MissingBaseline => f.write_str("delta baseline missing an entity"),
        }
    }
}

impl std::error::Error for WireError {}

/// Encodes a packet into `out`, returning the number of bytes written.
///
/// # Errors
/// Returns [`WireError::TooLarge`] when the payload does not fit one datagram.
pub fn encode_packet(
    header: &Header,
    payload: &[u8],
    out: &mut Vec<u8>,
) -> Result<usize, WireError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(WireError::TooLarge);
    }
    out.clear();
    out.reserve(HEADER_LEN + payload.len());
    put_u32(out, MAGIC);
    out.push(PROTOCOL_VERSION);
    out.push(header.kind as u8);
    put_u16(out, header.flags);
    put_u32(out, header.session);
    put_u32(out, header.seq);
    put_u32(out, header.ack);
    put_u32(out, header.ack_bits);
    put_u16(
        out,
        u16::try_from(payload.len()).map_err(|_| WireError::TooLarge)?,
    );
    out.extend_from_slice(payload);
    Ok(out.len())
}

/// Decodes a packet header and payload.
pub fn decode_packet(data: &[u8]) -> Result<(Header, &[u8]), WireError> {
    if data.len() < HEADER_LEN {
        return Err(WireError::Truncated);
    }
    let mut reader = Reader::new(data);
    if reader.u32()? != MAGIC {
        return Err(WireError::BadMagic);
    }
    let version = reader.u8()?;
    if version != PROTOCOL_VERSION {
        return Err(WireError::BadVersion);
    }
    let kind = PacketKind::from_u8(reader.u8()?).ok_or(WireError::BadKind)?;
    let flags = reader.u16()?;
    let session = reader.u32()?;
    let seq = reader.u32()?;
    let ack = reader.u32()?;
    let ack_bits = reader.u32()?;
    let payload_len = usize::from(reader.u16()?);
    if payload_len > MAX_PAYLOAD {
        return Err(WireError::TooLarge);
    }
    if reader.remaining() != payload_len {
        return Err(WireError::BadLength);
    }
    let payload = reader.bytes(payload_len)?;
    Ok((
        Header {
            kind,
            flags,
            session,
            seq,
            ack,
            ack_bits,
        },
        payload,
    ))
}

/// Quantises a world position to 1/16-unit grid cells.
#[must_use]
pub fn quantize_position(position: FixVec2) -> (u16, u16) {
    (quantize_axis(position.x), quantize_axis(position.y))
}

/// Dequantises 1/16-unit grid cells.
#[must_use]
pub fn dequantize_position(x: u16, y: u16) -> FixVec2 {
    FixVec2::new(
        Fix::from_bits(i64::from(x) << 28),
        Fix::from_bits(i64::from(y) << 28),
    )
}

fn quantize_axis(value: Fix) -> u16 {
    let scaled = (value * Fix::from_i32(POSITION_SCALE)).to_i64();
    scaled.clamp(0, i64::from(MAX_QUANTIZED_POSITION)) as u16
}

/// Serial-number comparison (RFC 1982 style) for 32-bit packet sequences.
///
/// Returns `true` when `candidate` is newer than `reference`, tolerating wraparound.
#[must_use]
pub const fn sequence_after(candidate: u32, reference: u32) -> bool {
    let difference = candidate.wrapping_sub(reference);
    difference != 0 && difference < 0x8000_0000
}
