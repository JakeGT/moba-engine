//! Wire format: packet header, commands and delta-compressed snapshots.
//!
//! All integers are little-endian and every decoder is bounds-checked. Packets are capped
//! at [`MAX_DATAGRAM`] bytes (fits a typical Ethernet MTU without IP fragmentation).
//! Large messages are split by the transport into fragments.

use core::fmt;

use moba_core::{Fix, FixVec2};
use moba_sim::ObservedUnit;

/// Datagram magic (`"MOBA"` little-endian).
pub const MAGIC: u32 = 0x4D4F_4241;
/// Wire protocol version.
pub const PROTOCOL_VERSION: u8 = 11;
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
}

/// Maximum items in a handshake catalogue.
pub const MAX_WIRE_ITEMS: usize = 16;
/// Fixed item-name bytes on the wire.
pub const WIRE_ITEM_NAME: usize = 16;
/// Maximum heroes in a handshake catalogue.
pub const MAX_WIRE_HEROES: usize = 8;
/// Fixed hero-name bytes on the wire.
pub const WIRE_HERO_NAME: usize = 16;
/// Maximum abilities in a handshake catalogue.
pub const MAX_WIRE_ABILITIES: usize = 24;
/// Fixed ability-name bytes on the wire.
pub const WIRE_ABILITY_NAME: usize = 16;
/// Maximum lanes in a handshake map.
pub const MAX_WIRE_LANES: usize = 4;
/// Maximum points per lane on the wire.
pub const MAX_WIRE_LANE_POINTS: usize = 16;
/// Fixed lane-name bytes on the wire.
pub const WIRE_LANE_NAME: usize = 16;
/// Maximum vision-wall segments in a handshake map.
pub const MAX_WIRE_WALLS: usize = 32;

/// One runtime item definition carried in the handshake.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireItem {
    /// Name bytes, padded with zeroes.
    pub name: [u8; WIRE_ITEM_NAME],
    /// Used bytes in `name`.
    pub name_len: u8,
    /// Gold cost.
    pub cost: u32,
    /// Health bonus.
    pub health: i32,
    /// Mana bonus.
    pub mana: i32,
    /// Attack damage bonus.
    pub attack_damage: i32,
    /// Armour bonus.
    pub armor: i32,
    /// Magic resistance bonus.
    pub magic_resist: i32,
    /// Movement speed bonus in raw fixed-point bits.
    pub move_speed_bits: i64,
}

impl Default for WireItem {
    fn default() -> Self {
        Self {
            name: [0; WIRE_ITEM_NAME],
            name_len: 0,
            cost: 0,
            health: 0,
            mana: 0,
            attack_damage: 0,
            armor: 0,
            magic_resist: 0,
            move_speed_bits: 0,
        }
    }
}

impl WireItem {
    /// Decodes the name as UTF-8, falling back to an empty string.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(WIRE_ITEM_NAME);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// One runtime hero definition carried in the handshake.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireHero {
    /// Name bytes, padded with zeroes.
    pub name: [u8; WIRE_HERO_NAME],
    /// Used bytes in `name`.
    pub name_len: u8,
    /// Movement speed in raw fixed-point bits.
    pub move_speed_bits: i64,
    /// Vision radius in raw fixed-point bits.
    pub vision_bits: i64,
    /// Ability ids for Q/W/E.
    pub abilities: [u8; 3],
}

impl Default for WireHero {
    fn default() -> Self {
        Self {
            name: [0; WIRE_HERO_NAME],
            name_len: 0,
            move_speed_bits: 0,
            vision_bits: 0,
            abilities: [0; 3],
        }
    }
}

impl WireHero {
    /// Decodes the name as UTF-8, falling back to an empty string.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(WIRE_HERO_NAME);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// One runtime ability definition carried in the handshake.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireAbility {
    /// Name bytes, padded with zeroes.
    pub name: [u8; WIRE_ABILITY_NAME],
    /// Used bytes in `name`.
    pub name_len: u8,
    /// Cooldown in ticks.
    pub cooldown_ticks: u32,
    /// Mana cost.
    pub mana_cost: u16,
}

impl Default for WireAbility {
    fn default() -> Self {
        Self {
            name: [0; WIRE_ABILITY_NAME],
            name_len: 0,
            cooldown_ticks: 0,
            mana_cost: 0,
        }
    }
}

impl WireAbility {
    /// Decodes the name as UTF-8, falling back to an empty string.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(WIRE_ABILITY_NAME);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// One lane path carried in the handshake map.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireLane {
    /// Name bytes, padded with zeroes.
    pub name: [u8; WIRE_LANE_NAME],
    /// Used bytes in `name`.
    pub name_len: u8,
    /// Number of valid points.
    pub point_count: u8,
    /// World-space points.
    pub points: [[i16; 2]; MAX_WIRE_LANE_POINTS],
}

impl Default for WireLane {
    fn default() -> Self {
        Self {
            name: [0; WIRE_LANE_NAME],
            name_len: 0,
            point_count: 0,
            points: [[0; 2]; MAX_WIRE_LANE_POINTS],
        }
    }
}

impl WireLane {
    /// Decodes the name as UTF-8, falling back to an empty string.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(WIRE_LANE_NAME);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// One vision-blocking wall segment.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireWall {
    /// First endpoint.
    pub a: [i16; 2],
    /// Second endpoint.
    pub b: [i16; 2],
}

/// Minimap geometry carried in the handshake.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireMap {
    /// Number of valid lanes.
    pub lane_count: u8,
    /// Lane paths.
    pub lanes: [WireLane; MAX_WIRE_LANES],
    /// Blue spawn.
    pub blue_spawn: [i16; 2],
    /// Red spawn.
    pub red_spawn: [i16; 2],
    /// Number of valid walls.
    pub wall_count: u8,
    /// Vision-blocking walls.
    pub walls: [WireWall; MAX_WIRE_WALLS],
}

impl Default for WireMap {
    fn default() -> Self {
        Self {
            lane_count: 0,
            lanes: [WireLane::default(); MAX_WIRE_LANES],
            blue_spawn: [0; 2],
            red_spawn: [0; 2],
            wall_count: 0,
            walls: [WireWall::default(); MAX_WIRE_WALLS],
        }
    }
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
    /// Number of valid entries in `items`.
    pub item_count: u8,
    /// Runtime item catalogue for shop/HUD rendering.
    pub items: [WireItem; MAX_WIRE_ITEMS],
    /// Number of valid entries in `heroes`.
    pub hero_count: u8,
    /// Runtime hero catalogue for HUD/vision rendering.
    pub heroes: [WireHero; MAX_WIRE_HEROES],
    /// Number of valid entries in `abilities`.
    pub ability_count: u8,
    /// Runtime ability catalogue for HUD rendering.
    pub abilities: [WireAbility; MAX_WIRE_ABILITIES],
    /// Minimap lane geometry.
    pub map: WireMap,
    /// Hash of the wire item/hero/ability/map catalogue (also authenticated by the Welcome HMAC).
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
    };
    if reader.remaining() != 0 {
        return Err(WireError::TrailingBytes);
    }
    Ok(hello)
}

/// Hashes the catalogue bytes exactly as sent in the handshake.
#[must_use]
pub fn catalog_hash(
    items: &[WireItem],
    heroes: &[WireHero],
    abilities: &[WireAbility],
    map: &WireMap,
) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
        }
    };
    for item in items {
        feed(&item.name);
        feed(&[item.name_len]);
        for value in [
            item.cost,
            item.health as u32,
            item.mana as u32,
            item.attack_damage as u32,
            item.armor as u32,
            item.magic_resist as u32,
        ] {
            feed(&value.to_le_bytes());
        }
        feed(&item.move_speed_bits.to_le_bytes());
    }
    for hero in heroes {
        feed(&hero.name);
        feed(&[hero.name_len]);
        feed(&hero.move_speed_bits.to_le_bytes());
        feed(&hero.vision_bits.to_le_bytes());
        feed(&hero.abilities);
    }
    for ability in abilities {
        feed(&ability.name);
        feed(&[ability.name_len]);
        feed(&ability.cooldown_ticks.to_le_bytes());
        feed(&ability.mana_cost.to_le_bytes());
    }
    for lane in map.lanes.iter().take(usize::from(map.lane_count)) {
        feed(&lane.name);
        feed(&[lane.name_len, lane.point_count]);
        for point in lane.points {
            feed(&point[0].to_le_bytes());
            feed(&point[1].to_le_bytes());
        }
    }
    for point in [map.blue_spawn, map.red_spawn] {
        feed(&point[0].to_le_bytes());
        feed(&point[1].to_le_bytes());
    }
    for wall in map.walls.iter().take(usize::from(map.wall_count)) {
        for point in [wall.a, wall.b] {
            feed(&point[0].to_le_bytes());
            feed(&point[1].to_le_bytes());
        }
    }
    hash
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
    out.push(welcome.item_count);
    for item in welcome.items.iter().take(MAX_WIRE_ITEMS) {
        out.extend_from_slice(&item.name);
        out.push(item.name_len);
        put_u32(&mut out, item.cost);
        put_u32(&mut out, item.health as u32);
        put_u32(&mut out, item.mana as u32);
        put_u32(&mut out, item.attack_damage as u32);
        put_u32(&mut out, item.armor as u32);
        put_u32(&mut out, item.magic_resist as u32);
        out.extend_from_slice(&item.move_speed_bits.to_le_bytes());
    }
    out.push(welcome.hero_count);
    for hero in welcome.heroes.iter().take(MAX_WIRE_HEROES) {
        out.extend_from_slice(&hero.name);
        out.push(hero.name_len);
        out.extend_from_slice(&hero.move_speed_bits.to_le_bytes());
        out.extend_from_slice(&hero.vision_bits.to_le_bytes());
        out.extend_from_slice(&hero.abilities);
    }
    out.push(welcome.ability_count);
    for ability in welcome.abilities.iter().take(MAX_WIRE_ABILITIES) {
        out.extend_from_slice(&ability.name);
        out.push(ability.name_len);
        put_u32(&mut out, ability.cooldown_ticks);
        put_u16(&mut out, ability.mana_cost);
    }
    out.push(welcome.map.lane_count);
    for lane in welcome.map.lanes.iter().take(MAX_WIRE_LANES) {
        out.extend_from_slice(&lane.name);
        out.push(lane.name_len);
        out.push(lane.point_count);
        for point in lane.points {
            out.extend_from_slice(&point[0].to_le_bytes());
            out.extend_from_slice(&point[1].to_le_bytes());
        }
    }
    out.extend_from_slice(&welcome.map.blue_spawn[0].to_le_bytes());
    out.extend_from_slice(&welcome.map.blue_spawn[1].to_le_bytes());
    out.extend_from_slice(&welcome.map.red_spawn[0].to_le_bytes());
    out.extend_from_slice(&welcome.map.red_spawn[1].to_le_bytes());
    out.push(welcome.map.wall_count);
    for wall in welcome.map.walls.iter().take(MAX_WIRE_WALLS) {
        out.extend_from_slice(&wall.a[0].to_le_bytes());
        out.extend_from_slice(&wall.a[1].to_le_bytes());
        out.extend_from_slice(&wall.b[0].to_le_bytes());
        out.extend_from_slice(&wall.b[1].to_le_bytes());
    }
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
        item_count: reader.u8()?,
        items: {
            let mut items = [WireItem::default(); MAX_WIRE_ITEMS];
            for item in &mut items {
                let mut name = [0; WIRE_ITEM_NAME];
                name.copy_from_slice(reader.bytes(WIRE_ITEM_NAME)?);
                item.name = name;
                item.name_len = reader.u8()?;
                item.cost = reader.u32()?;
                item.health = i32::from_le_bytes(
                    reader
                        .bytes(4)?
                        .try_into()
                        .map_err(|_| WireError::Truncated)?,
                );
                item.mana = i32::from_le_bytes(
                    reader
                        .bytes(4)?
                        .try_into()
                        .map_err(|_| WireError::Truncated)?,
                );
                item.attack_damage = i32::from_le_bytes(
                    reader
                        .bytes(4)?
                        .try_into()
                        .map_err(|_| WireError::Truncated)?,
                );
                item.armor = i32::from_le_bytes(
                    reader
                        .bytes(4)?
                        .try_into()
                        .map_err(|_| WireError::Truncated)?,
                );
                item.magic_resist = i32::from_le_bytes(
                    reader
                        .bytes(4)?
                        .try_into()
                        .map_err(|_| WireError::Truncated)?,
                );
                item.move_speed_bits = i64::from_le_bytes(
                    reader
                        .bytes(8)?
                        .try_into()
                        .map_err(|_| WireError::Truncated)?,
                );
            }
            items
        },
        hero_count: reader.u8()?,
        heroes: {
            let mut heroes = [WireHero::default(); MAX_WIRE_HEROES];
            for hero in &mut heroes {
                let mut name = [0; WIRE_HERO_NAME];
                name.copy_from_slice(reader.bytes(WIRE_HERO_NAME)?);
                hero.name = name;
                hero.name_len = reader.u8()?;
                hero.move_speed_bits = i64::from_le_bytes(
                    reader
                        .bytes(8)?
                        .try_into()
                        .map_err(|_| WireError::Truncated)?,
                );
                hero.vision_bits = i64::from_le_bytes(
                    reader
                        .bytes(8)?
                        .try_into()
                        .map_err(|_| WireError::Truncated)?,
                );
                hero.abilities.copy_from_slice(reader.bytes(3)?);
            }
            heroes
        },
        ability_count: reader.u8()?,
        abilities: {
            let mut abilities = [WireAbility::default(); MAX_WIRE_ABILITIES];
            for ability in &mut abilities {
                let mut name = [0; WIRE_ABILITY_NAME];
                name.copy_from_slice(reader.bytes(WIRE_ABILITY_NAME)?);
                ability.name = name;
                ability.name_len = reader.u8()?;
                ability.cooldown_ticks = reader.u32()?;
                ability.mana_cost = reader.u16()?;
            }
            abilities
        },
        map: {
            let lane_count = reader.u8()?;
            let mut lanes = [WireLane::default(); MAX_WIRE_LANES];
            for lane in &mut lanes {
                let mut name = [0; WIRE_LANE_NAME];
                name.copy_from_slice(reader.bytes(WIRE_LANE_NAME)?);
                lane.name = name;
                lane.name_len = reader.u8()?;
                lane.point_count = reader.u8()?;
                for point in &mut lane.points {
                    point[0] = i16::from_le_bytes(
                        reader
                            .bytes(2)?
                            .try_into()
                            .map_err(|_| WireError::Truncated)?,
                    );
                    point[1] = i16::from_le_bytes(
                        reader
                            .bytes(2)?
                            .try_into()
                            .map_err(|_| WireError::Truncated)?,
                    );
                }
            }
            WireMap {
                lane_count,
                lanes,
                blue_spawn: [
                    i16::from_le_bytes(
                        reader
                            .bytes(2)?
                            .try_into()
                            .map_err(|_| WireError::Truncated)?,
                    ),
                    i16::from_le_bytes(
                        reader
                            .bytes(2)?
                            .try_into()
                            .map_err(|_| WireError::Truncated)?,
                    ),
                ],
                red_spawn: [
                    i16::from_le_bytes(
                        reader
                            .bytes(2)?
                            .try_into()
                            .map_err(|_| WireError::Truncated)?,
                    ),
                    i16::from_le_bytes(
                        reader
                            .bytes(2)?
                            .try_into()
                            .map_err(|_| WireError::Truncated)?,
                    ),
                ],
                wall_count: reader.u8()?,
                walls: {
                    let mut walls = [WireWall::default(); MAX_WIRE_WALLS];
                    for wall in &mut walls {
                        wall.a = [
                            i16::from_le_bytes(
                                reader
                                    .bytes(2)?
                                    .try_into()
                                    .map_err(|_| WireError::Truncated)?,
                            ),
                            i16::from_le_bytes(
                                reader
                                    .bytes(2)?
                                    .try_into()
                                    .map_err(|_| WireError::Truncated)?,
                            ),
                        ];
                        wall.b = [
                            i16::from_le_bytes(
                                reader
                                    .bytes(2)?
                                    .try_into()
                                    .map_err(|_| WireError::Truncated)?,
                            ),
                            i16::from_le_bytes(
                                reader
                                    .bytes(2)?
                                    .try_into()
                                    .map_err(|_| WireError::Truncated)?,
                            ),
                        ];
                    }
                    walls
                },
            }
        },
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

/// High-level match phase carried in snapshots.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MatchPhase {
    /// Waiting for players; the simulation is paused.
    Lobby = 0,
    /// Pre-match countdown; the simulation is paused.
    Countdown = 1,
    /// The simulation is running.
    Playing = 2,
    /// Nexus destroyed; showing results before a rematch.
    Results = 3,
}

impl MatchPhase {
    /// Decodes a phase byte.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Lobby,
            1 => Self::Countdown,
            2 => Self::Playing,
            3 => Self::Results,
            _ => return None,
        })
    }
}

/// A player command as it travels on the wire.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WireCommand {
    /// Clear orders.
    Stop,
    /// Move to a quantised position.
    MoveTo {
        /// Quantised X.
        x: u16,
        /// Quantised Y.
        y: u16,
    },
    /// Attack a network entity id.
    Attack {
        /// Server-assigned id for this client.
        net_id: NetId,
    },
    /// Cast an ability at a quantised position.
    CastAbility {
        /// Q/W/E slot.
        slot: u8,
        /// Quantised X.
        x: u16,
        /// Quantised Y.
        y: u16,
    },
    /// Purchase an item from the shop.
    BuyItem {
        /// Catalogue index.
        item: u8,
    },
    /// Toggle lobby readiness.
    Ready {
        /// Requested ready state.
        ready: bool,
    },
    /// Vote to end the match for the sender's team.
    Surrender,
}

/// Client intent payload.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InputMessage {
    /// Client's estimate of the current server tick.
    pub client_tick: u32,
    /// Monotonic input sequence.
    pub input_seq: u32,
    /// Command.
    pub command: WireCommand,
}

/// Network entity identifier assigned per connection.
pub type NetId = u16;

/// Hero progression carried in snapshots.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct NetProgress {
    /// Current level.
    pub level: u8,
    /// Experience towards the next level.
    pub xp: u16,
    /// Gold (only filled for the receiving player's hero).
    pub gold: u16,
    /// Inventory item ids (`0xFF` empty).
    pub items: [u8; 6],
}

/// Quantised entity state carried in snapshots.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NetEntity {
    /// Connection-local id.
    pub net_id: NetId,
    /// Unit kind (`moba_sim::UnitKind` discriminant).
    pub kind: u8,
    /// Team discriminant.
    pub team: u8,
    /// Hero slot or `0xFF`.
    pub hero_slot: u8,
    /// Hero definition index or `0xFF`.
    pub hero_pick: u8,
    /// Flag bit 0: dead hero.
    pub flags: u8,
    /// Maximum health.
    pub max_health: u16,
    /// Maximum mana (zero for non-heroes).
    pub max_mana: u16,
    /// Quantised X.
    pub x: u16,
    /// Quantised Y.
    pub y: u16,
    /// Health.
    pub health: u16,
    /// Mana.
    pub mana: u16,
    /// Remaining Q/W/E cooldowns in ticks.
    pub cooldowns: [u16; 3],
    /// Level, XP, gold and inventory.
    pub progress: NetProgress,
}

/// Flag bit for dead heroes.
pub const ENTITY_FLAG_DEAD: u8 = 1;
/// A temporary shield is active.
pub const ENTITY_FLAG_SHIELD: u8 = 1 << 1;

impl NetEntity {
    /// Quantises an observed unit.
    #[must_use]
    pub fn from_observed(net_id: NetId, unit: &ObservedUnit) -> Self {
        let (x, y) = quantize_position(unit.position);
        Self {
            net_id,
            kind: unit.kind as u8,
            team: unit.team.index() as u8,
            hero_slot: unit.hero_slot.unwrap_or(0xFF),
            hero_pick: unit.hero_pick,
            flags: (if unit.dead { ENTITY_FLAG_DEAD } else { 0 })
                | (if unit.shielded { ENTITY_FLAG_SHIELD } else { 0 }),
            max_health: clamp_u16(unit.max_health),
            max_mana: clamp_u16(unit.max_mana),
            x,
            y,
            health: clamp_u16(unit.health),
            mana: clamp_u16(unit.mana),
            cooldowns: [
                clamp_u16_u32(unit.cooldowns[0]),
                clamp_u16_u32(unit.cooldowns[1]),
                clamp_u16_u32(unit.cooldowns[2]),
            ],
            progress: NetProgress {
                level: unit.level,
                xp: clamp_u16_u32(unit.xp),
                gold: clamp_u16_u32(unit.gold),
                items: unit.items,
            },
        }
    }

    /// Decodes the quantised position.
    #[must_use]
    pub fn position(self) -> FixVec2 {
        dequantize_position(self.x, self.y)
    }

    /// Whether this entity is a dead hero.
    #[must_use]
    pub const fn is_dead(self) -> bool {
        self.flags & ENTITY_FLAG_DEAD != 0
    }

    /// Returns the changed dynamic fields relative to `previous`.
    #[must_use]
    pub fn diff(&self, previous: &Self) -> NetUpdate {
        NetUpdate {
            net_id: self.net_id,
            position: (self.x != previous.x || self.y != previous.y).then_some((self.x, self.y)),
            health: (self.health != previous.health).then_some(self.health),
            mana: (self.mana != previous.mana).then_some(self.mana),
            flags: (self.flags != previous.flags).then_some(self.flags),
            cooldowns: (self.cooldowns != previous.cooldowns).then_some(self.cooldowns),
            progress: (self.progress != previous.progress).then_some(self.progress),
        }
    }

    /// Applies a delta update.
    pub fn apply(&mut self, update: &NetUpdate) {
        if let Some((x, y)) = update.position {
            self.x = x;
            self.y = y;
        }
        if let Some(health) = update.health {
            self.health = health;
        }
        if let Some(mana) = update.mana {
            self.mana = mana;
        }
        if let Some(flags) = update.flags {
            self.flags = flags;
        }
        if let Some(cooldowns) = update.cooldowns {
            self.cooldowns = cooldowns;
        }
        if let Some(progress) = update.progress {
            self.progress = progress;
        }
    }
}

/// Changed fields of one entity.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NetUpdate {
    /// Entity id.
    pub net_id: NetId,
    /// Position when changed.
    pub position: Option<(u16, u16)>,
    /// Health when changed.
    pub health: Option<u16>,
    /// Mana when changed.
    pub mana: Option<u16>,
    /// Flags when changed.
    pub flags: Option<u8>,
    /// Cooldowns when changed.
    pub cooldowns: Option<[u16; 3]>,
    /// Progression/inventory when changed.
    pub progress: Option<NetProgress>,
}

const UPDATE_POSITION: u8 = 1 << 0;
const UPDATE_HEALTH: u8 = 1 << 1;
const UPDATE_MANA: u8 = 1 << 2;
const UPDATE_FLAGS: u8 = 1 << 3;
const UPDATE_COOLDOWNS: u8 = 1 << 4;
const UPDATE_PROGRESS: u8 = 1 << 5;

/// One snapshot, either a full state (`baseline_seq == 0`) or a delta.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotDelta {
    /// Server tick that produced the snapshot.
    pub tick: u32,
    /// Last client input sequence the server processed.
    pub acked_input: u32,
    /// Snapshot sequence this delta is based on (0 = full).
    pub baseline_seq: u32,
    /// Blue team hero kills.
    pub blue_score: u32,
    /// Red team hero kills.
    pub red_score: u32,
    /// Match outcome: 0 none, 1 blue, 2 red.
    pub outcome: u8,
    /// Current match phase.
    pub phase: u8,
    /// Ticks remaining in countdown/results.
    pub countdown: u16,
    /// Bitmask of player slots that are ready.
    pub ready_mask: u16,
    /// Bitmask of connected player slots.
    pub player_mask: u16,
    /// Newly visible entities (always full state).
    pub spawns: Vec<NetEntity>,
    /// Changed entities.
    pub updates: Vec<NetUpdate>,
    /// Entities that left visibility or died.
    pub despawns: Vec<NetId>,
}

impl SnapshotDelta {
    /// Whether the delta carries no state at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spawns.is_empty() && self.updates.is_empty() && self.despawns.is_empty()
    }
}

/// Client snapshot store. Entities are sorted by id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotState {
    /// Snapshot sequence this state corresponds to.
    pub seq: u32,
    /// Server tick.
    pub tick: u32,
    /// Blue team hero kills.
    pub blue_score: u32,
    /// Red team hero kills.
    pub red_score: u32,
    /// Match outcome: 0 none, 1 blue, 2 red.
    pub outcome: u8,
    /// Current match phase.
    pub phase: u8,
    /// Ticks remaining in countdown/results.
    pub countdown: u16,
    /// Bitmask of player slots that are ready.
    pub ready_mask: u16,
    /// Bitmask of connected player slots.
    pub player_mask: u16,
    /// Entities sorted by network id.
    pub entities: Vec<NetEntity>,
}

impl SnapshotState {
    /// Builds a state from a full snapshot.
    #[must_use]
    pub fn from_full(seq: u32, delta: &SnapshotDelta) -> Self {
        let mut entities = delta.spawns.clone();
        entities.sort_unstable_by_key(|entity| entity.net_id);
        Self {
            seq,
            tick: delta.tick,
            blue_score: delta.blue_score,
            red_score: delta.red_score,
            outcome: delta.outcome,
            phase: delta.phase,
            countdown: delta.countdown,
            ready_mask: delta.ready_mask,
            player_mask: delta.player_mask,
            entities,
        }
    }

    /// Applies a delta that is based on this state.
    ///
    /// # Errors
    /// Returns [`WireError::MissingBaseline`] if an update/despawn references an entity
    /// this state does not know.
    pub fn apply(&self, delta: &SnapshotDelta, seq: u32) -> Result<Self, WireError> {
        if delta.baseline_seq == 0 {
            return Ok(Self::from_full(seq, delta));
        }
        if delta.baseline_seq != self.seq {
            return Err(WireError::MissingBaseline);
        }
        let mut entities = self.entities.clone();
        for &net_id in &delta.despawns {
            let index = entities
                .binary_search_by_key(&net_id, |entity| entity.net_id)
                .map_err(|_| WireError::MissingBaseline)?;
            entities.remove(index);
        }
        for update in &delta.updates {
            let index = entities
                .binary_search_by_key(&update.net_id, |entity| entity.net_id)
                .map_err(|_| WireError::MissingBaseline)?;
            entities[index].apply(update);
        }
        for spawn in &delta.spawns {
            let index = entities
                .binary_search_by_key(&spawn.net_id, |entity| entity.net_id)
                .unwrap_or_else(|index| index);
            entities.insert(index, *spawn);
        }
        Ok(Self {
            seq,
            tick: delta.tick,
            blue_score: delta.blue_score,
            red_score: delta.red_score,
            outcome: delta.outcome,
            phase: delta.phase,
            countdown: delta.countdown,
            ready_mask: delta.ready_mask,
            player_mask: delta.player_mask,
            entities,
        })
    }

    /// Finds an entity by network id.
    #[must_use]
    pub fn get(&self, net_id: NetId) -> Option<&NetEntity> {
        self.entities
            .binary_search_by_key(&net_id, |entity| entity.net_id)
            .ok()
            .map(|index| &self.entities[index])
    }
}

/// Builds a delta from the current visible state and an optional baseline.
#[must_use]
pub fn build_delta(
    current: &[NetEntity],
    baseline: Option<&[NetEntity]>,
    tick: u32,
    acked_input: u32,
    baseline_seq: u32,
) -> SnapshotDelta {
    let Some(baseline) = baseline else {
        return SnapshotDelta {
            tick,
            acked_input,
            baseline_seq: 0,
            blue_score: 0,
            red_score: 0,
            outcome: 0,
            phase: 0,
            countdown: 0,
            ready_mask: 0,
            player_mask: 0,
            spawns: current.to_vec(),
            updates: Vec::new(),
            despawns: Vec::new(),
        };
    };
    let mut delta = SnapshotDelta {
        tick,
        acked_input,
        baseline_seq,
        blue_score: 0,
        red_score: 0,
        outcome: 0,
        phase: 0,
        countdown: 0,
        ready_mask: 0,
        player_mask: 0,
        spawns: Vec::new(),
        updates: Vec::new(),
        despawns: Vec::new(),
    };
    let (mut old_index, mut new_index) = (0usize, 0usize);
    while old_index < baseline.len() || new_index < current.len() {
        match (baseline.get(old_index), current.get(new_index)) {
            (Some(old), Some(new)) if old.net_id == new.net_id => {
                let update = new.diff(old);
                if update.position.is_some()
                    || update.health.is_some()
                    || update.mana.is_some()
                    || update.flags.is_some()
                    || update.cooldowns.is_some()
                    || update.progress.is_some()
                {
                    delta.updates.push(update);
                }
                old_index += 1;
                new_index += 1;
            }
            (Some(old), Some(new)) if old.net_id < new.net_id => {
                delta.despawns.push(old.net_id);
                old_index += 1;
            }
            (_, Some(new)) => {
                delta.spawns.push(*new);
                new_index += 1;
            }
            (Some(old), None) => {
                delta.despawns.push(old.net_id);
                old_index += 1;
            }
            (None, None) => break,
        }
    }
    delta
}

/// Encodes an input message payload.
#[must_use]
pub fn encode_input(message: &InputMessage) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    put_u32(&mut out, message.client_tick);
    put_u32(&mut out, message.input_seq);
    encode_command(&mut out, message.command);
    out
}

/// Decodes an input message payload.
pub fn decode_input(data: &[u8]) -> Result<InputMessage, WireError> {
    let mut reader = Reader::new(data);
    let client_tick = reader.u32()?;
    let input_seq = reader.u32()?;
    let command = decode_command(&mut reader)?;
    if reader.remaining() != 0 {
        return Err(WireError::TrailingBytes);
    }
    Ok(InputMessage {
        client_tick,
        input_seq,
        command,
    })
}

fn encode_command(out: &mut Vec<u8>, command: WireCommand) {
    match command {
        WireCommand::Stop => out.push(0),
        WireCommand::MoveTo { x, y } => {
            out.push(1);
            put_u16(out, x);
            put_u16(out, y);
        }
        WireCommand::Attack { net_id } => {
            out.push(2);
            put_u16(out, net_id);
        }
        WireCommand::CastAbility { slot, x, y } => {
            out.push(3);
            out.push(slot);
            put_u16(out, x);
            put_u16(out, y);
        }
        WireCommand::BuyItem { item } => {
            out.push(4);
            out.push(item);
        }
        WireCommand::Ready { ready } => {
            out.push(5);
            out.push(u8::from(ready));
        }
        WireCommand::Surrender => out.push(6),
    }
}

fn decode_command(reader: &mut Reader<'_>) -> Result<WireCommand, WireError> {
    Ok(match reader.u8()? {
        0 => WireCommand::Stop,
        1 => WireCommand::MoveTo {
            x: reader.u16()?,
            y: reader.u16()?,
        },
        2 => WireCommand::Attack {
            net_id: reader.u16()?,
        },
        3 => WireCommand::CastAbility {
            slot: reader.u8()?,
            x: reader.u16()?,
            y: reader.u16()?,
        },
        4 => WireCommand::BuyItem { item: reader.u8()? },
        5 => WireCommand::Ready {
            ready: reader.u8()? != 0,
        },
        6 => WireCommand::Surrender,
        _ => return Err(WireError::BadValue),
    })
}

/// Encodes a snapshot payload.
///
/// # Errors
/// Returns [`WireError::TooMany`] when a count exceeds `u16::MAX`.
pub fn encode_snapshot(delta: &SnapshotDelta, out: &mut Vec<u8>) -> Result<(), WireError> {
    out.clear();
    out.reserve(40 + delta.spawns.len() * 36 + delta.updates.len() * 12 + delta.despawns.len() * 2);
    put_u32(out, delta.tick);
    put_u32(out, delta.acked_input);
    put_u32(out, delta.baseline_seq);
    put_u32(out, delta.blue_score);
    put_u32(out, delta.red_score);
    out.push(delta.outcome);
    out.push(delta.phase);
    put_u16(out, delta.countdown);
    put_u16(out, delta.ready_mask);
    put_u16(out, delta.player_mask);
    put_u16(out, count_u16(delta.spawns.len())?);
    put_u16(out, count_u16(delta.updates.len())?);
    put_u16(out, count_u16(delta.despawns.len())?);
    for spawn in &delta.spawns {
        put_u16(out, spawn.net_id);
        out.push(spawn.kind);
        out.push(spawn.team);
        out.push(spawn.hero_slot);
        out.push(spawn.hero_pick);
        out.push(spawn.flags);
        put_u16(out, spawn.max_health);
        put_u16(out, spawn.max_mana);
        put_u16(out, spawn.x);
        put_u16(out, spawn.y);
        put_u16(out, spawn.health);
        put_u16(out, spawn.mana);
        for cooldown in spawn.cooldowns {
            put_u16(out, cooldown);
        }
        out.push(spawn.progress.level);
        put_u16(out, spawn.progress.xp);
        put_u16(out, spawn.progress.gold);
        out.extend_from_slice(&spawn.progress.items);
    }
    for update in &delta.updates {
        put_u16(out, update.net_id);
        let mut mask = 0u8;
        if update.position.is_some() {
            mask |= UPDATE_POSITION;
        }
        if update.health.is_some() {
            mask |= UPDATE_HEALTH;
        }
        if update.mana.is_some() {
            mask |= UPDATE_MANA;
        }
        if update.flags.is_some() {
            mask |= UPDATE_FLAGS;
        }
        if update.cooldowns.is_some() {
            mask |= UPDATE_COOLDOWNS;
        }
        if update.progress.is_some() {
            mask |= UPDATE_PROGRESS;
        }
        out.push(mask);
        if let Some((x, y)) = update.position {
            put_u16(out, x);
            put_u16(out, y);
        }
        if let Some(health) = update.health {
            put_u16(out, health);
        }
        if let Some(mana) = update.mana {
            put_u16(out, mana);
        }
        if let Some(flags) = update.flags {
            out.push(flags);
        }
        if let Some(cooldowns) = update.cooldowns {
            for cooldown in cooldowns {
                put_u16(out, cooldown);
            }
        }
        if let Some(progress) = update.progress {
            out.push(progress.level);
            put_u16(out, progress.xp);
            put_u16(out, progress.gold);
            out.extend_from_slice(&progress.items);
        }
    }
    for net_id in &delta.despawns {
        put_u16(out, *net_id);
    }
    Ok(())
}

/// Decodes a snapshot payload.
#[allow(clippy::too_many_lines)] // Field order mirrors the encoder one-for-one.
pub fn decode_snapshot(data: &[u8]) -> Result<SnapshotDelta, WireError> {
    let mut reader = Reader::new(data);
    let tick = reader.u32()?;
    let acked_input = reader.u32()?;
    let baseline_seq = reader.u32()?;
    let blue_score = reader.u32()?;
    let red_score = reader.u32()?;
    let outcome = reader.u8()?;
    let phase = reader.u8()?;
    let countdown = reader.u16()?;
    let ready_mask = reader.u16()?;
    let player_mask = reader.u16()?;
    let spawn_count = usize::from(reader.u16()?);
    let update_count = usize::from(reader.u16()?);
    let despawn_count = usize::from(reader.u16()?);
    let mut spawns = Vec::with_capacity(spawn_count);
    for _ in 0..spawn_count {
        spawns.push(NetEntity {
            net_id: reader.u16()?,
            kind: reader.u8()?,
            team: reader.u8()?,
            hero_slot: reader.u8()?,
            hero_pick: reader.u8()?,
            flags: reader.u8()?,
            max_health: reader.u16()?,
            max_mana: reader.u16()?,
            x: reader.u16()?,
            y: reader.u16()?,
            health: reader.u16()?,
            mana: reader.u16()?,
            cooldowns: [reader.u16()?, reader.u16()?, reader.u16()?],
            progress: NetProgress {
                level: reader.u8()?,
                xp: reader.u16()?,
                gold: reader.u16()?,
                items: reader
                    .bytes(6)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            },
        });
    }
    let mut updates = Vec::with_capacity(update_count);
    for _ in 0..update_count {
        let net_id = reader.u16()?;
        let mask = reader.u8()?;
        let known = UPDATE_POSITION
            | UPDATE_HEALTH
            | UPDATE_MANA
            | UPDATE_FLAGS
            | UPDATE_COOLDOWNS
            | UPDATE_PROGRESS;
        if mask & !known != 0 {
            return Err(WireError::BadValue);
        }
        let position = if mask & UPDATE_POSITION != 0 {
            Some((reader.u16()?, reader.u16()?))
        } else {
            None
        };
        let health = (mask & UPDATE_HEALTH != 0)
            .then(|| reader.u16())
            .transpose()?;
        let mana = (mask & UPDATE_MANA != 0)
            .then(|| reader.u16())
            .transpose()?;
        let flags = (mask & UPDATE_FLAGS != 0)
            .then(|| reader.u8())
            .transpose()?;
        let cooldowns = (mask & UPDATE_COOLDOWNS != 0)
            .then(|| Ok::<[u16; 3], WireError>([reader.u16()?, reader.u16()?, reader.u16()?]))
            .transpose()?;
        let progress = (mask & UPDATE_PROGRESS != 0)
            .then(|| {
                Ok::<NetProgress, WireError>(NetProgress {
                    level: reader.u8()?,
                    xp: reader.u16()?,
                    gold: reader.u16()?,
                    items: reader
                        .bytes(6)?
                        .try_into()
                        .map_err(|_| WireError::Truncated)?,
                })
            })
            .transpose()?;
        updates.push(NetUpdate {
            net_id,
            position,
            health,
            mana,
            flags,
            cooldowns,
            progress,
        });
    }
    let mut despawns = Vec::with_capacity(despawn_count);
    for _ in 0..despawn_count {
        despawns.push(reader.u16()?);
    }
    if reader.remaining() != 0 {
        return Err(WireError::TrailingBytes);
    }
    Ok(SnapshotDelta {
        tick,
        acked_input,
        baseline_seq,
        blue_score,
        red_score,
        outcome,
        phase,
        countdown,
        ready_mask,
        player_mask,
        spawns,
        updates,
        despawns,
    })
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

fn clamp_u16(value: i32) -> u16 {
    value.clamp(0, i32::from(u16::MAX)) as u16
}

fn clamp_u16_u32(value: u32) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn count_u16(count: usize) -> Result<u16, WireError> {
    u16::try_from(count).map_err(|_| WireError::TooMany)
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    const fn remaining(&self) -> usize {
        self.data.len() - self.position
    }

    fn u8(&mut self) -> Result<u8, WireError> {
        let byte = *self.data.get(self.position).ok_or(WireError::Truncated)?;
        self.position += 1;
        Ok(byte)
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn bytes(&mut self, length: usize) -> Result<&'a [u8], WireError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(WireError::Truncated)?;
        let slice = self
            .data
            .get(self.position..end)
            .ok_or(WireError::Truncated)?;
        self.position = end;
        Ok(slice)
    }
}

/// Serial-number comparison (RFC 1982 style) for 32-bit packet sequences.
///
/// Returns `true` when `candidate` is newer than `reference`, tolerating wraparound.
#[must_use]
pub const fn sequence_after(candidate: u32, reference: u32) -> bool {
    let difference = candidate.wrapping_sub(reference);
    difference != 0 && difference < 0x8000_0000
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_ecs::Entity;
    use moba_game::Team;
    use moba_sim::UnitKind;

    fn header(kind: PacketKind, seq: u32) -> Header {
        Header {
            kind,
            flags: 0,
            session: 0xDEAD_BEEF,
            seq,
            ack: 7,
            ack_bits: 0b1010,
        }
    }

    #[test]
    fn catalog_hash_covers_item_and_hero_catalogue() {
        let mut items = [WireItem::default(); MAX_WIRE_ITEMS];
        items[0].name[..5].copy_from_slice(b"Blade");
        items[0].name_len = 5;
        let mut heroes = [WireHero::default(); MAX_WIRE_HEROES];
        heroes[0].name[..4].copy_from_slice(b"Hero");
        heroes[0].name_len = 4;
        let baseline = catalog_hash(&items[..1], &heroes[..1], &[], &WireMap::default());
        items[0].cost = 1;
        assert_ne!(
            catalog_hash(&items[..1], &heroes[..1], &[], &WireMap::default()),
            baseline
        );
        items[0].cost = 0;
        heroes[0].abilities[0] = 1;
        assert_ne!(
            catalog_hash(&items[..1], &heroes[..1], &[], &WireMap::default()),
            baseline
        );
    }

    #[test]
    fn progress_only_changes_are_delta_encoded() {
        let baseline = vec![entity(1, 10, 20, 600)];
        let mut current = baseline.clone();
        current[0].progress.gold = 350;
        current[0].progress.items[0] = 2;
        let delta = build_delta(&current, Some(&baseline), 2, 0, 1);
        assert_eq!(delta.updates.len(), 1);
        let progress = delta.updates[0].progress.expect("progress update");
        assert_eq!(progress.gold, 350);
        assert_eq!(progress.items[0], 2);
    }

    #[test]
    fn packet_round_trip_and_bounds() {
        let mut buffer = Vec::new();
        let payload = [1u8, 2, 3, 4, 5];
        let written = encode_packet(&header(PacketKind::Input, 9), &payload, &mut buffer).unwrap();
        assert_eq!(written, HEADER_LEN + payload.len());
        let (decoded, body) = decode_packet(&buffer).unwrap();
        assert_eq!(decoded, header(PacketKind::Input, 9));
        assert_eq!(body, payload);

        assert_eq!(decode_packet(&[]), Err(WireError::Truncated));
        let mut corrupt = buffer.clone();
        corrupt[0] = 0;
        assert_eq!(decode_packet(&corrupt), Err(WireError::BadMagic));
        let long = vec![0u8; MAX_PAYLOAD + 1];
        assert_eq!(
            encode_packet(&header(PacketKind::Snapshot, 1), &long, &mut buffer),
            Err(WireError::TooLarge)
        );
    }

    #[test]
    fn handshake_round_trip() {
        let hello = Hello {
            protocol: 1,
            nonce: 0xDEAD_BEEF_1234,
            reconnect: 0xABCD_1234,
            auth: [3; 32],
            spectate: true,
        };
        assert_eq!(decode_hello(&encode_hello(&hello)), Ok(hello));
    }

    #[test]
    fn commands_round_trip() {
        let commands = [
            WireCommand::Stop,
            WireCommand::MoveTo { x: 10, y: 20 },
            WireCommand::Attack { net_id: 99 },
            WireCommand::CastAbility {
                slot: 2,
                x: 500,
                y: 600,
            },
        ];
        for command in commands {
            let message = InputMessage {
                client_tick: 123,
                input_seq: 45,
                command,
            };
            let encoded = encode_input(&message);
            assert_eq!(decode_input(&encoded), Ok(message));
        }
    }

    fn entity(id: NetId, x: u16, y: u16, health: u16) -> NetEntity {
        NetEntity {
            net_id: id,
            kind: UnitKind::Hero as u8,
            team: Team::Blue.index() as u8,
            hero_slot: 0,
            hero_pick: 0,
            flags: 0,
            max_health: 600,
            max_mana: 300,
            x,
            y,
            health,
            mana: 100,
            cooldowns: [0, 0, 0],
            progress: NetProgress::default(),
        }
    }

    #[test]
    fn full_and_delta_snapshots_apply() {
        let baseline = vec![entity(1, 10, 20, 600), entity(2, 30, 40, 200)];
        let mut current = baseline.clone();
        current[0].x = 11;
        current[0].health = 500;
        current[0].progress = NetProgress {
            level: 5,
            xp: 120,
            gold: 750,
            items: [0, 1, 0xFF, 0xFF, 0xFF, 0xFF],
        };
        current[1].mana = 50;
        current.push(entity(3, 50, 60, 600));

        let mut full = build_delta(&baseline, None, 3, 42, 0);
        full.blue_score = 3;
        full.red_score = 2;
        full.outcome = 1;
        full.phase = 2;
        full.countdown = 0;
        let mut payload = Vec::new();
        encode_snapshot(&full, &mut payload).unwrap();
        let decoded = decode_snapshot(&payload).unwrap();
        let state = SnapshotState::from_full(3, &decoded);
        assert_eq!(state.entities.len(), 2);
        assert_eq!((state.blue_score, state.red_score), (3, 2));
        assert_eq!(state.outcome, 1);
        assert_eq!(state.phase, 2);

        let mut delta = build_delta(&current, Some(&baseline), 4, 43, 3);
        delta.blue_score = 4;
        delta.red_score = 2;
        delta.outcome = 2;
        delta.phase = 3;
        delta.countdown = 450;
        assert_eq!(delta.spawns.len(), 1);
        assert_eq!(delta.updates.len(), 2);
        encode_snapshot(&delta, &mut payload).unwrap();
        let decoded = decode_snapshot(&payload).unwrap();
        let next = state.apply(&decoded, 4).unwrap();
        assert_eq!(next.entities.len(), 3);
        assert_eq!(next.get(1).unwrap().health, 500);
        assert_eq!(next.get(1).unwrap().progress.level, 5);
        assert_eq!(next.get(1).unwrap().progress.gold, 750);
        assert_eq!(next.get(1).unwrap().progress.items[1], 1);
        assert_eq!(next.get(1).unwrap().x, 11);
        assert_eq!(next.get(2).unwrap().mana, 50);
        assert_eq!(next.get(3).unwrap().x, 50);
        assert_eq!((next.blue_score, next.red_score), (4, 2));
        assert_eq!(next.outcome, 2);
        assert_eq!(next.phase, 3);
        assert_eq!(next.countdown, 450);
    }

    #[test]
    fn delta_with_removals_and_wrong_baseline() {
        let baseline = vec![entity(1, 10, 20, 600), entity(2, 30, 40, 200)];
        let current = vec![entity(2, 30, 40, 200)];
        let delta = build_delta(&current, Some(&baseline), 2, 0, 1);
        assert_eq!(delta.despawns, vec![1]);
        let state = SnapshotState {
            seq: 1,
            tick: 1,
            blue_score: 0,
            red_score: 0,
            outcome: 0,
            phase: 0,
            countdown: 0,
            ready_mask: 0,
            player_mask: 0,
            entities: baseline,
        };
        let applied = state.apply(&delta, 2).unwrap();
        assert_eq!(applied.entities.len(), 1);
        assert_eq!(applied.get(2).unwrap().net_id, 2);
        let wrong = SnapshotState {
            seq: 9,
            tick: 9,
            blue_score: 0,
            red_score: 0,
            outcome: 0,
            phase: 0,
            countdown: 0,
            ready_mask: 0,
            player_mask: 0,
            entities: applied.entities.clone(),
        };
        assert_eq!(wrong.apply(&delta, 3), Err(WireError::MissingBaseline));
    }

    #[test]
    fn position_quantisation_is_bounded() {
        let position = FixVec2::from_ints(128, 64);
        let (x, y) = quantize_position(position);
        let round = dequantize_position(x, y);
        assert!(round.distance(position) <= Fix::from_f64(0.05));
        assert_eq!(
            quantize_position(FixVec2::from_ints(-10, 999)),
            (0, MAX_QUANTIZED_POSITION)
        );
    }

    #[test]
    fn decoders_never_panic_on_noise() {
        let mut rng = moba_core::Rng::from_seed(0xF022);
        for length in 0..256usize {
            for _ in 0..20 {
                let mut data = vec![0u8; length];
                rng.fill_bytes(&mut data);
                let _ = decode_packet(&data);
                let _ = decode_snapshot(&data);
                let _ = decode_input(&data);
                let _ = decode_hello(&data);
                let _ = decode_welcome(&data);
            }
        }
        // Random payloads behind a valid header exercise the message decoders too.
        let mut payload = vec![0u8; 512];
        for _ in 0..200 {
            rng.fill_bytes(&mut payload);
            let mut packet = Vec::new();
            encode_packet(&header(PacketKind::Snapshot, 1), &payload, &mut packet).unwrap();
            let (_, body) = decode_packet(&packet).unwrap();
            let _ = decode_snapshot(body);
            let _ = decode_input(body);
        }
    }

    #[test]
    fn serial_sequence_tolerates_wrap() {
        assert!(sequence_after(1, 0));
        assert!(!sequence_after(0, 1));
        assert!(!sequence_after(5, 5));
        assert!(sequence_after(2, u32::MAX));
        assert!(!sequence_after(u32::MAX, 2));
    }

    #[test]
    fn observed_unit_conversion() {
        let unit = ObservedUnit {
            entity: Entity::from_parts(3, 1),
            kind: UnitKind::Hero,
            team: Team::Red,
            position: FixVec2::from_ints(120, 80),
            health: 500,
            max_health: 600,
            mana: 200,
            max_mana: 300,
            dead: false,
            hero_slot: Some(7),
            hero_pick: 2,
            cooldowns: [10, 20, 30],
            level: 4,
            xp: 120,
            gold: 950,
            items: [0, 1, 2, 0xFF, 0xFF, 0xFF],
            shielded: true,
        };
        let entity = NetEntity::from_observed(4, &unit);
        assert_eq!(entity.net_id, 4);
        assert_eq!(entity.position(), unit.position);
        assert_eq!(entity.hero_pick, 2);
        assert_eq!(entity.progress.level, 4);
        assert_eq!(entity.progress.gold, 950);
        assert!(!entity.is_dead());
        assert!(entity.flags & ENTITY_FLAG_SHIELD != 0);
    }
}
