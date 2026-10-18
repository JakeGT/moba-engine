//! Wire format: packet header, commands and delta-compressed snapshots.
//!
//! All integers are little-endian and every decoder is bounds-checked. Packets are capped
//! at [`MAX_DATAGRAM`] bytes (fits a typical Ethernet MTU without IP fragmentation).
//! Large messages are split by the transport into fragments.

use core::fmt;

use moba_core::{Fix, FixVec2};
use moba_game::{
    AbilitySpec, AreaEffect, AreaEffectList, ContentManifest, DamageType, Effect, EffectList,
    GameContent, HeroSpec, ItemSpec, LaneSpec, MapSpec, ModifierDef, StructureKind, StructureSpec,
    Targeting, Team, WallSpec,
};
use moba_sim::ObservedUnit;

/// Datagram magic (`"MOBA"` little-endian).
pub const MAGIC: u32 = 0x4D4F_4241;
/// Wire protocol version.
pub const PROTOCOL_VERSION: u8 = 18;
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
    /// Whether a client public key/signature is present.
    pub identity: bool,
    /// Client Ed25519 public key.
    pub client_public: [u8; 32],
    /// Signature over the server challenge.
    pub client_signature: [u8; 64],
    /// Server challenge being answered (`0` on the first attempt).
    pub server_challenge: u64,
}

/// Minimum encoded bytes for one spawn record.
const MIN_SPAWN_BYTES: usize = 36;
/// Minimum encoded bytes for one update record (`net_id` + mask).
const MIN_UPDATE_BYTES: usize = 3;
/// Minimum encoded bytes for one despawn record.
const MIN_DESPAWN_BYTES: usize = 2;

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
/// Maximum structures in a handshake map.
pub const MAX_WIRE_STRUCTURES: usize = 32;
/// Maximum modifier definitions in a handshake catalogue.
pub const MAX_WIRE_MODIFIERS: usize = 16;
/// Fixed modifier-name bytes on the wire.
pub const WIRE_MODIFIER_NAME: usize = 16;
/// Maximum top-level ability effects on the wire.
pub const MAX_WIRE_EFFECTS: usize = 3;
/// Maximum area/on-hit effects per list on the wire.
pub const MAX_WIRE_AREA_EFFECTS: usize = 3;
/// Fixed manifest-field bytes on the wire.
pub const WIRE_MANIFEST_FIELD: usize = 32;

const _: () = {
    assert!(MAX_WIRE_ITEMS == moba_game::MAX_CONTENT_ITEMS);
    assert!(MAX_WIRE_HEROES == moba_game::MAX_CONTENT_HEROES);
    assert!(MAX_WIRE_ABILITIES == moba_game::MAX_CONTENT_ABILITIES);
    assert!(MAX_WIRE_LANES == moba_game::MAX_CONTENT_LANES);
    assert!(MAX_WIRE_LANE_POINTS == moba_game::MAX_LANE_POINTS);
    assert!(MAX_WIRE_WALLS == moba_game::MAX_CONTENT_WALLS);
    assert!(MAX_WIRE_STRUCTURES == moba_game::MAX_CONTENT_STRUCTURES);
    assert!(WIRE_MANIFEST_FIELD == moba_game::MAX_MANIFEST_FIELD);
    assert!(MAX_WIRE_MODIFIERS == moba_game::MAX_CONTENT_MODIFIERS);
    assert!(WIRE_MODIFIER_NAME == moba_game::MAX_ITEM_NAME);
    assert!(MAX_WIRE_EFFECTS == moba_game::MAX_ABILITY_EFFECTS);
    assert!(MAX_WIRE_AREA_EFFECTS == moba_game::MAX_AREA_EFFECTS);
    assert!(WIRE_ITEM_NAME == moba_game::MAX_ITEM_NAME);
    assert!(WIRE_HERO_NAME == moba_game::MAX_HERO_NAME);
    assert!(WIRE_ABILITY_NAME == moba_game::MAX_ABILITY_NAME);
};

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
    /// Maximum health.
    pub max_health: i32,
    /// Maximum mana.
    pub max_mana: i32,
    /// Health regenerated per tick.
    pub health_regen_per_tick: i32,
    /// Mana regenerated per tick.
    pub mana_regen_per_tick: i32,
    /// Basic attack damage.
    pub attack_damage: i32,
    /// Basic attack range in raw fixed-point bits.
    pub attack_range_bits: i64,
    /// Basic attack interval in ticks.
    pub attack_interval_ticks: u32,
    /// Movement speed in raw fixed-point bits.
    pub move_speed_bits: i64,
    /// Armour.
    pub armor: i32,
    /// Magic resistance.
    pub magic_resist: i32,
    /// Vision radius in raw fixed-point bits.
    pub vision_bits: i64,
    /// Ability ids for Q/W/E.
    pub abilities: [u8; 3],
    /// Health gained per level.
    pub health_per_level: i32,
    /// Mana gained per level.
    pub mana_per_level: i32,
    /// Attack damage gained per level.
    pub attack_damage_per_level: i32,
    /// Armour gained per level.
    pub armor_per_level: i32,
    /// Magic resistance gained per level.
    pub magic_resist_per_level: i32,
}

impl Default for WireHero {
    fn default() -> Self {
        Self {
            name: [0; WIRE_HERO_NAME],
            name_len: 0,
            max_health: 0,
            max_mana: 0,
            health_regen_per_tick: 0,
            mana_regen_per_tick: 0,
            attack_damage: 0,
            attack_range_bits: 0,
            attack_interval_ticks: 0,
            move_speed_bits: 0,
            armor: 0,
            magic_resist: 0,
            vision_bits: 0,
            abilities: [0; 3],
            health_per_level: 0,
            mana_per_level: 0,
            attack_damage_per_level: 0,
            armor_per_level: 0,
            magic_resist_per_level: 0,
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

/// One area/on-hit effect carried in the handshake.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireAreaEffect {
    /// 0 none, 1 damage, 2 apply-modifier.
    pub kind: u8,
    /// Modifier definition id.
    pub modifier: u16,
    /// Duration in ticks.
    pub duration_ticks: u32,
    /// Movement factor in raw fixed-point bits.
    pub factor_bits: i64,
    /// Shield/burn amount.
    pub amount: i32,
    /// Damage type discriminant.
    pub damage_type: u8,
}

/// One top-level ability effect carried in the handshake.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireEffect {
    /// 0 none, 1 damage, 2 heal, 3 modifier, 4 displace, 5 projectile, 6 area, 7 zone.
    pub kind: u8,
    /// Damage/heal amount.
    pub amount: i32,
    /// Damage type discriminant.
    pub damage_type: u8,
    /// Projectile speed in raw fixed-point bits.
    pub speed_bits: i64,
    /// Range/radius/displace in raw fixed-point bits.
    pub range_bits: i64,
    /// Radius in raw fixed-point bits.
    pub radius_bits: i64,
    /// Visual projectile arc height in raw fixed-point bits.
    pub arc_bits: i64,
    /// Duration in ticks.
    pub duration_ticks: u32,
    /// Period in ticks.
    pub period_ticks: u32,
    /// Number of valid `area` entries.
    pub area_count: u8,
    /// Area/on-hit effects.
    pub area: [WireAreaEffect; MAX_WIRE_AREA_EFFECTS],
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
    pub mana_cost: i32,
    /// Summary maximum cast distance in raw fixed-point bits.
    pub cast_range_bits: i64,
    /// Summary display radius in raw fixed-point bits.
    pub radius_bits: i64,
    /// 0 self, 1 point, 2 direction.
    pub targeting_kind: u8,
    /// Targeting range in raw fixed-point bits.
    pub targeting_range_bits: i64,
    /// Number of valid `effects` entries.
    pub effect_count: u8,
    /// Ordered effects.
    pub effects: [WireEffect; MAX_WIRE_EFFECTS],
}

impl Default for WireAbility {
    fn default() -> Self {
        Self {
            name: [0; WIRE_ABILITY_NAME],
            name_len: 0,
            cooldown_ticks: 0,
            mana_cost: 0,
            cast_range_bits: 0,
            radius_bits: 0,
            targeting_kind: 0,
            targeting_range_bits: 0,
            effect_count: 0,
            effects: [WireEffect::default(); MAX_WIRE_EFFECTS],
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

/// One lane path carried in the handshake map./// One lane path carried in the handshake map.
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
    /// Square world extent in world units.
    pub extent: u16,
    /// Number of valid lanes.
    pub lane_count: u8,
    /// Lane paths.
    pub lanes: [WireLane; MAX_WIRE_LANES],
    /// Blue spawn.
    pub blue_spawn: [i16; 2],
    /// Red spawn.
    pub red_spawn: [i16; 2],
    /// Number of valid structures.
    pub structure_count: u8,
    /// Placed structures.
    pub structures: [WireStructure; MAX_WIRE_STRUCTURES],
    /// Number of valid walls.
    pub wall_count: u8,
    /// Vision-blocking walls.
    pub walls: [WireWall; MAX_WIRE_WALLS],
}

/// One placed structure carried in the handshake map.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireStructure {
    /// Team index (blue 0, red 1).
    pub team: u8,
    /// Structure kind (tower 0, nexus 1).
    pub kind: u8,
    /// World position.
    pub position: [i16; 2],
    /// Lane index, or `0xFF` for none.
    pub lane: u8,
}

impl Default for WireMap {
    fn default() -> Self {
        Self {
            extent: 256,
            lane_count: 0,
            lanes: [WireLane::default(); MAX_WIRE_LANES],
            blue_spawn: [0; 2],
            red_spawn: [0; 2],
            structure_count: 0,
            structures: [WireStructure::default(); MAX_WIRE_STRUCTURES],
            wall_count: 0,
            walls: [WireWall::default(); MAX_WIRE_WALLS],
        }
    }
}

/// One modifier definition carried in the handshake.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireModifier {
    /// Name bytes, padded with zeroes.
    pub name: [u8; WIRE_MODIFIER_NAME],
    /// Used bytes in `name`.
    pub name_len: u8,
    /// Behaviour kind.
    pub kind: u8,
}

impl WireModifier {
    /// Decodes the name as UTF-8, falling back to an empty string.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(WIRE_MODIFIER_NAME);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// Complete runtime catalogue carried in the handshake.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireCatalog {
    /// Manifest name bytes.
    pub manifest_name: [u8; WIRE_MANIFEST_FIELD],
    /// Used manifest name bytes.
    pub manifest_name_len: u8,
    /// Manifest version bytes.
    pub manifest_version: [u8; WIRE_MANIFEST_FIELD],
    /// Used manifest version bytes.
    pub manifest_version_len: u8,
    /// Manifest author bytes.
    pub manifest_author: [u8; WIRE_MANIFEST_FIELD],
    /// Used manifest author bytes.
    pub manifest_author_len: u8,
    /// Number of valid items.
    pub item_count: u8,
    /// Item catalogue.
    pub items: [WireItem; MAX_WIRE_ITEMS],
    /// Number of valid heroes.
    pub hero_count: u8,
    /// Hero catalogue.
    pub heroes: [WireHero; MAX_WIRE_HEROES],
    /// Number of valid abilities.
    pub ability_count: u8,
    /// Ability catalogue.
    pub abilities: [WireAbility; MAX_WIRE_ABILITIES],
    /// Number of valid modifiers.
    pub modifier_count: u8,
    /// Modifier definitions.
    pub modifiers: [WireModifier; MAX_WIRE_MODIFIERS],
    /// Map geometry.
    pub map: WireMap,
}

impl Default for WireCatalog {
    fn default() -> Self {
        Self {
            manifest_name: [0; WIRE_MANIFEST_FIELD],
            manifest_name_len: 0,
            manifest_version: [0; WIRE_MANIFEST_FIELD],
            manifest_version_len: 0,
            manifest_author: [0; WIRE_MANIFEST_FIELD],
            manifest_author_len: 0,
            item_count: 0,
            items: [WireItem::default(); MAX_WIRE_ITEMS],
            hero_count: 0,
            heroes: [WireHero::default(); MAX_WIRE_HEROES],
            ability_count: 0,
            abilities: [WireAbility::default(); MAX_WIRE_ABILITIES],
            modifier_count: 0,
            modifiers: [WireModifier::default(); MAX_WIRE_MODIFIERS],
            map: WireMap::default(),
        }
    }
}

fn encode_area_effects(effects: &AreaEffectList) -> (u8, [WireAreaEffect; MAX_WIRE_AREA_EFFECTS]) {
    let mut area = [WireAreaEffect::default(); MAX_WIRE_AREA_EFFECTS];
    let mut count = 0usize;
    for effect in effects.iter() {
        if count >= MAX_WIRE_AREA_EFFECTS {
            break;
        }
        area[count] = match effect {
            AreaEffect::None => WireAreaEffect::default(),
            AreaEffect::Damage {
                amount,
                damage_type,
            } => WireAreaEffect {
                kind: 1,
                amount: *amount,
                damage_type: damage_type_code(*damage_type),
                ..WireAreaEffect::default()
            },
            AreaEffect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            } => WireAreaEffect {
                kind: 2,
                modifier: *modifier,
                duration_ticks: *duration_ticks,
                factor_bits: factor.to_bits(),
                amount: *amount,
                damage_type: damage_type_code(*damage_type),
            },
        };
        count += 1;
    }
    (u8::try_from(count).unwrap_or(u8::MAX), area)
}

impl From<&GameContent> for WireCatalog {
    #[allow(clippy::too_many_lines)] // One exhaustive destructured conversion.
    fn from(content: &GameContent) -> Self {
        let GameContent {
            manifest,
            items: content_items,
            heroes: content_heroes,
            abilities: content_abilities,
            modifiers: content_modifiers,
            map: content_map,
        } = content;
        let ContentManifest {
            name,
            version,
            author,
        } = manifest;
        let mut catalog = Self::default();
        catalog.manifest_name_len = put_wire_name(&mut catalog.manifest_name, name);
        catalog.manifest_version_len = put_wire_name(&mut catalog.manifest_version, version);
        catalog.manifest_author_len = put_wire_name(&mut catalog.manifest_author, author);

        catalog.item_count = u8::try_from(content_items.len().min(MAX_WIRE_ITEMS))
            .unwrap_or(u8::try_from(MAX_WIRE_ITEMS).unwrap_or(u8::MAX));
        for (wire, item) in catalog.items.iter_mut().zip(content_items) {
            let ItemSpec {
                name,
                cost,
                health,
                mana,
                attack_damage,
                armor,
                magic_resist,
                move_speed_per_sec,
            } = item;
            wire.name_len = put_wire_name(&mut wire.name, name);
            wire.cost = *cost;
            wire.health = *health;
            wire.mana = *mana;
            wire.attack_damage = *attack_damage;
            wire.armor = *armor;
            wire.magic_resist = *magic_resist;
            wire.move_speed_bits = move_speed_per_sec.to_bits();
        }

        catalog.hero_count = u8::try_from(content_heroes.len().min(MAX_WIRE_HEROES))
            .unwrap_or(u8::try_from(MAX_WIRE_HEROES).unwrap_or(u8::MAX));
        for (wire, hero) in catalog.heroes.iter_mut().zip(content_heroes) {
            let HeroSpec {
                name,
                max_health,
                max_mana,
                health_regen_per_tick,
                mana_regen_per_tick,
                attack_damage,
                attack_range,
                attack_interval_ticks,
                move_speed_per_sec,
                armor,
                magic_resist,
                vision_range,
                abilities,
                health_per_level,
                mana_per_level,
                attack_damage_per_level,
                armor_per_level,
                magic_resist_per_level,
            } = hero;
            wire.name_len = put_wire_name(&mut wire.name, name);
            wire.max_health = *max_health;
            wire.max_mana = *max_mana;
            wire.health_regen_per_tick = *health_regen_per_tick;
            wire.mana_regen_per_tick = *mana_regen_per_tick;
            wire.attack_damage = *attack_damage;
            wire.attack_range_bits = attack_range.to_bits();
            wire.attack_interval_ticks = *attack_interval_ticks;
            wire.move_speed_bits = move_speed_per_sec.to_bits();
            wire.armor = *armor;
            wire.magic_resist = *magic_resist;
            wire.vision_bits = vision_range.to_bits();
            wire.abilities = *abilities;
            wire.health_per_level = *health_per_level;
            wire.mana_per_level = *mana_per_level;
            wire.attack_damage_per_level = *attack_damage_per_level;
            wire.armor_per_level = *armor_per_level;
            wire.magic_resist_per_level = *magic_resist_per_level;
        }

        catalog.ability_count = u8::try_from(content_abilities.len().min(MAX_WIRE_ABILITIES))
            .unwrap_or(u8::try_from(MAX_WIRE_ABILITIES).unwrap_or(u8::MAX));
        for (wire, ability) in catalog.abilities.iter_mut().zip(content_abilities) {
            let AbilitySpec {
                name,
                cooldown_ticks,
                mana_cost,
                targeting,
                effects,
            } = ability;
            wire.name_len = put_wire_name(&mut wire.name, name);
            wire.cooldown_ticks = *cooldown_ticks;
            wire.mana_cost = *mana_cost;
            wire.cast_range_bits = targeting.range().to_bits();
            wire.radius_bits = ability.display_radius().to_bits();
            match targeting {
                Targeting::SelfCast => wire.targeting_kind = 0,
                Targeting::Point { range } => {
                    wire.targeting_kind = 1;
                    wire.targeting_range_bits = range.to_bits();
                }
                Targeting::Direction { range } => {
                    wire.targeting_kind = 2;
                    wire.targeting_range_bits = range.to_bits();
                }
            }
            let mut count = 0usize;
            for effect in effects.iter() {
                if count >= MAX_WIRE_EFFECTS {
                    break;
                }
                let mut encoded = WireEffect::default();
                match effect {
                    Effect::None => continue,
                    Effect::Damage {
                        amount,
                        damage_type,
                    } => {
                        encoded.kind = 1;
                        encoded.amount = *amount;
                        encoded.damage_type = damage_type_code(*damage_type);
                    }
                    Effect::Heal { amount } => {
                        encoded.kind = 2;
                        encoded.amount = *amount;
                    }
                    Effect::ApplyModifier {
                        modifier,
                        duration_ticks,
                        factor,
                        amount,
                        damage_type,
                    } => {
                        encoded.kind = 3;
                        encoded.amount = *amount;
                        encoded.damage_type = damage_type_code(*damage_type);
                        encoded.range_bits = factor.to_bits();
                        encoded.duration_ticks = *duration_ticks;
                        encoded.area[0] = WireAreaEffect {
                            kind: 2,
                            modifier: *modifier,
                            duration_ticks: *duration_ticks,
                            factor_bits: factor.to_bits(),
                            amount: *amount,
                            damage_type: damage_type_code(*damage_type),
                        };
                        encoded.area_count = 1;
                    }
                    Effect::Displace { range } => {
                        encoded.kind = 4;
                        encoded.range_bits = range.to_bits();
                    }
                    Effect::Projectile {
                        speed_per_sec,
                        range,
                        radius,
                        arc,
                        on_hit,
                    } => {
                        encoded.kind = 5;
                        encoded.speed_bits = speed_per_sec.to_bits();
                        encoded.range_bits = range.to_bits();
                        encoded.radius_bits = radius.to_bits();
                        encoded.arc_bits = arc.to_bits();
                        let (area_count, area) = encode_area_effects(on_hit);
                        encoded.area_count = area_count;
                        encoded.area = area;
                    }
                    Effect::Area { radius, effects } => {
                        encoded.kind = 6;
                        encoded.radius_bits = radius.to_bits();
                        let (area_count, area) = encode_area_effects(effects);
                        encoded.area_count = area_count;
                        encoded.area = area;
                    }
                    Effect::Zone {
                        radius,
                        duration_ticks,
                        period_ticks,
                        effects,
                    } => {
                        encoded.kind = 7;
                        encoded.radius_bits = radius.to_bits();
                        encoded.duration_ticks = *duration_ticks;
                        encoded.period_ticks = *period_ticks;
                        let (area_count, area) = encode_area_effects(effects);
                        encoded.area_count = area_count;
                        encoded.area = area;
                    }
                }
                wire.effects[count] = encoded;
                count += 1;
            }
            wire.effect_count = u8::try_from(count).unwrap_or(u8::MAX);
        }

        catalog.modifier_count = u8::try_from(content_modifiers.len().min(MAX_WIRE_MODIFIERS))
            .unwrap_or(u8::try_from(MAX_WIRE_MODIFIERS).unwrap_or(u8::MAX));
        for (wire, modifier) in catalog.modifiers.iter_mut().zip(content_modifiers) {
            let ModifierDef { name, kind } = modifier;
            wire.name_len = put_wire_name(&mut wire.name, name);
            wire.kind = match kind {
                moba_game::ModifierKind::Shield => 0,
                moba_game::ModifierKind::Slow => 1,
                moba_game::ModifierKind::Haste => 2,
                moba_game::ModifierKind::Root => 3,
                moba_game::ModifierKind::Silence => 4,
                moba_game::ModifierKind::Burn => 5,
            };
        }

        let MapSpec {
            extent,
            lanes: content_lanes,
            structures: content_structures,
            walls: content_walls,
            blue_spawn,
            red_spawn,
        } = content_map;
        let map = &mut catalog.map;
        map.lane_count = u8::try_from(content_lanes.len().min(MAX_WIRE_LANES))
            .unwrap_or(u8::try_from(MAX_WIRE_LANES).unwrap_or(u8::MAX));
        for (wire, lane) in map.lanes.iter_mut().zip(content_lanes) {
            let LaneSpec { name, path } = lane;
            wire.name_len = put_wire_name(&mut wire.name, name);
            wire.point_count = u8::try_from(path.len().min(MAX_WIRE_LANE_POINTS))
                .unwrap_or(u8::try_from(MAX_WIRE_LANE_POINTS).unwrap_or(u8::MAX));
            for (target, point) in wire.points.iter_mut().zip(path) {
                target[0] = i16::try_from(point.x.to_i64()).unwrap_or(0);
                target[1] = i16::try_from(point.y.to_i64()).unwrap_or(0);
            }
        }
        map.extent = u16::try_from(extent.to_i64()).unwrap_or(256);
        map.blue_spawn = [
            i16::try_from(blue_spawn.x.to_i64()).unwrap_or(0),
            i16::try_from(blue_spawn.y.to_i64()).unwrap_or(0),
        ];
        map.red_spawn = [
            i16::try_from(red_spawn.x.to_i64()).unwrap_or(0),
            i16::try_from(red_spawn.y.to_i64()).unwrap_or(0),
        ];
        map.structure_count = u8::try_from(content_structures.len().min(MAX_WIRE_STRUCTURES))
            .unwrap_or(u8::try_from(MAX_WIRE_STRUCTURES).unwrap_or(u8::MAX));
        for (wire, structure) in map.structures.iter_mut().zip(content_structures) {
            let StructureSpec {
                team,
                position,
                kind,
                lane,
            } = structure;
            wire.team = team.index() as u8;
            wire.kind = match kind {
                StructureKind::Tower => 0,
                StructureKind::Nexus => 1,
            };
            wire.position = [
                i16::try_from(position.x.to_i64()).unwrap_or(0),
                i16::try_from(position.y.to_i64()).unwrap_or(0),
            ];
            wire.lane = lane.unwrap_or(0xFF);
        }
        map.wall_count = u8::try_from(content_walls.len().min(MAX_WIRE_WALLS))
            .unwrap_or(u8::try_from(MAX_WIRE_WALLS).unwrap_or(u8::MAX));
        for (wire, wall) in map.walls.iter_mut().zip(content_walls) {
            let WallSpec { a, b } = wall;
            wire.a = [
                i16::try_from(a.x.to_i64()).unwrap_or(0),
                i16::try_from(a.y.to_i64()).unwrap_or(0),
            ];
            wire.b = [
                i16::try_from(b.x.to_i64()).unwrap_or(0),
                i16::try_from(b.y.to_i64()).unwrap_or(0),
            ];
        }
        catalog
    }
}

fn decode_area_effects(wire: &WireEffect) -> Result<AreaEffectList, WireError> {
    let count = usize::from(wire.area_count);
    if count > MAX_WIRE_AREA_EFFECTS {
        return Err(WireError::BadValue);
    }
    let mut effects = AreaEffectList::EMPTY;
    for (slot, encoded) in wire.area[..count].iter().enumerate() {
        effects.effects[slot] = match encoded.kind {
            0 => AreaEffect::None,
            1 => AreaEffect::Damage {
                amount: encoded.amount,
                damage_type: damage_type_from_code(encoded.damage_type)?,
            },
            2 => AreaEffect::ApplyModifier {
                modifier: encoded.modifier,
                duration_ticks: encoded.duration_ticks,
                factor: Fix::from_bits(encoded.factor_bits),
                amount: encoded.amount,
                damage_type: damage_type_from_code(encoded.damage_type)?,
            },
            _ => return Err(WireError::BadValue),
        };
    }
    Ok(effects)
}

fn decode_effect(wire: &WireEffect) -> Result<Effect, WireError> {
    Ok(match wire.kind {
        0 => Effect::None,
        1 => Effect::Damage {
            amount: wire.amount,
            damage_type: damage_type_from_code(wire.damage_type)?,
        },
        2 => Effect::Heal {
            amount: wire.amount,
        },
        3 => {
            let area = decode_area_effects(wire)?;
            let Some(AreaEffect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            }) = area.effects.first().copied()
            else {
                return Err(WireError::BadValue);
            };
            Effect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            }
        }
        4 => Effect::Displace {
            range: Fix::from_bits(wire.range_bits),
        },
        5 => Effect::Projectile {
            speed_per_sec: Fix::from_bits(wire.speed_bits),
            range: Fix::from_bits(wire.range_bits),
            radius: Fix::from_bits(wire.radius_bits),
            arc: Fix::from_bits(wire.arc_bits),
            on_hit: decode_area_effects(wire)?,
        },
        6 => Effect::Area {
            radius: Fix::from_bits(wire.radius_bits),
            effects: decode_area_effects(wire)?,
        },
        7 => Effect::Zone {
            radius: Fix::from_bits(wire.radius_bits),
            duration_ticks: wire.duration_ticks,
            period_ticks: wire.period_ticks,
            effects: decode_area_effects(wire)?,
        },
        _ => return Err(WireError::BadValue),
    })
}

impl WireCatalog {
    /// Reconstructs the runtime content represented by this catalogue.
    ///
    /// # Errors
    /// Returns [`WireError::BadValue`] for invalid enum/UTF-8/count fields.
    #[allow(clippy::too_many_lines)] // One exhaustive reconstruction.
    pub fn to_content(&self) -> Result<GameContent, WireError> {
        macro_rules! count {
            ($value:expr, $max:expr) => {{
                let count = usize::from($value);
                if count > $max {
                    return Err(WireError::BadValue);
                }
                count
            }};
        }
        let items = (0..count!(self.item_count, MAX_WIRE_ITEMS))
            .map(|index| {
                let item = &self.items[index];
                Ok(ItemSpec {
                    name: read_wire_name(&item.name, item.name_len, WIRE_ITEM_NAME)?,
                    cost: item.cost,
                    health: item.health,
                    mana: item.mana,
                    attack_damage: item.attack_damage,
                    armor: item.armor,
                    magic_resist: item.magic_resist,
                    move_speed_per_sec: Fix::from_bits(item.move_speed_bits),
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let heroes = (0..count!(self.hero_count, MAX_WIRE_HEROES))
            .map(|index| {
                let hero = &self.heroes[index];
                Ok(HeroSpec {
                    name: read_wire_name(&hero.name, hero.name_len, WIRE_HERO_NAME)?,
                    max_health: hero.max_health,
                    max_mana: hero.max_mana,
                    health_regen_per_tick: hero.health_regen_per_tick,
                    mana_regen_per_tick: hero.mana_regen_per_tick,
                    attack_damage: hero.attack_damage,
                    attack_range: Fix::from_bits(hero.attack_range_bits),
                    attack_interval_ticks: hero.attack_interval_ticks,
                    move_speed_per_sec: Fix::from_bits(hero.move_speed_bits),
                    armor: hero.armor,
                    magic_resist: hero.magic_resist,
                    vision_range: Fix::from_bits(hero.vision_bits),
                    abilities: hero.abilities,
                    health_per_level: hero.health_per_level,
                    mana_per_level: hero.mana_per_level,
                    attack_damage_per_level: hero.attack_damage_per_level,
                    armor_per_level: hero.armor_per_level,
                    magic_resist_per_level: hero.magic_resist_per_level,
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let abilities = (0..count!(self.ability_count, MAX_WIRE_ABILITIES))
            .map(|index| {
                let ability = &self.abilities[index];
                let targeting = match ability.targeting_kind {
                    0 => Targeting::SelfCast,
                    1 => Targeting::Point {
                        range: Fix::from_bits(ability.targeting_range_bits),
                    },
                    2 => Targeting::Direction {
                        range: Fix::from_bits(ability.targeting_range_bits),
                    },
                    _ => return Err(WireError::BadValue),
                };
                let effect_count = count!(ability.effect_count, MAX_WIRE_EFFECTS);
                let mut effects = EffectList::EMPTY;
                for (slot, wire) in ability.effects[..effect_count].iter().enumerate() {
                    effects.effects[slot] = decode_effect(wire)?;
                }
                Ok(AbilitySpec {
                    name: read_wire_name(&ability.name, ability.name_len, WIRE_ABILITY_NAME)?,
                    cooldown_ticks: ability.cooldown_ticks,
                    mana_cost: ability.mana_cost,
                    targeting,
                    effects,
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let modifiers = (0..count!(self.modifier_count, MAX_WIRE_MODIFIERS))
            .map(|index| {
                let modifier = &self.modifiers[index];
                Ok(ModifierDef {
                    name: read_wire_name(&modifier.name, modifier.name_len, WIRE_MODIFIER_NAME)?,
                    kind: match modifier.kind {
                        0 => moba_game::ModifierKind::Shield,
                        1 => moba_game::ModifierKind::Slow,
                        2 => moba_game::ModifierKind::Haste,
                        3 => moba_game::ModifierKind::Root,
                        4 => moba_game::ModifierKind::Silence,
                        5 => moba_game::ModifierKind::Burn,
                        _ => return Err(WireError::BadValue),
                    },
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let map = &self.map;
        let lanes = (0..count!(map.lane_count, MAX_WIRE_LANES))
            .map(|index| {
                let lane = &map.lanes[index];
                let point_count = count!(lane.point_count, MAX_WIRE_LANE_POINTS);
                Ok(LaneSpec {
                    name: read_wire_name(&lane.name, lane.name_len, WIRE_LANE_NAME)?,
                    path: lane.points[..point_count]
                        .iter()
                        .map(|point| FixVec2::from_ints(i32::from(point[0]), i32::from(point[1])))
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let structures = (0..count!(map.structure_count, MAX_WIRE_STRUCTURES))
            .map(|index| {
                let structure = &map.structures[index];
                Ok(StructureSpec {
                    team: match structure.team {
                        0 => Team::Blue,
                        1 => Team::Red,
                        _ => return Err(WireError::BadValue),
                    },
                    position: FixVec2::from_ints(
                        i32::from(structure.position[0]),
                        i32::from(structure.position[1]),
                    ),
                    kind: match structure.kind {
                        0 => StructureKind::Tower,
                        1 => StructureKind::Nexus,
                        _ => return Err(WireError::BadValue),
                    },
                    lane: (structure.lane != 0xFF).then_some(structure.lane),
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let walls = (0..count!(map.wall_count, MAX_WIRE_WALLS))
            .map(|index| {
                let wall = &map.walls[index];
                Ok(WallSpec {
                    a: FixVec2::from_ints(i32::from(wall.a[0]), i32::from(wall.a[1])),
                    b: FixVec2::from_ints(i32::from(wall.b[0]), i32::from(wall.b[1])),
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        Ok(GameContent {
            manifest: ContentManifest {
                name: read_wire_name(
                    &self.manifest_name,
                    self.manifest_name_len,
                    WIRE_MANIFEST_FIELD,
                )?,
                version: read_wire_name(
                    &self.manifest_version,
                    self.manifest_version_len,
                    WIRE_MANIFEST_FIELD,
                )?,
                author: read_wire_name(
                    &self.manifest_author,
                    self.manifest_author_len,
                    WIRE_MANIFEST_FIELD,
                )?,
            },
            items,
            heroes,
            abilities,
            modifiers,
            map: MapSpec {
                extent: Fix::from_i32(i32::from(map.extent)),
                lanes,
                structures,
                walls,
                blue_spawn: FixVec2::from_ints(
                    i32::from(map.blue_spawn[0]),
                    i32::from(map.blue_spawn[1]),
                ),
                red_spawn: FixVec2::from_ints(
                    i32::from(map.red_spawn[0]),
                    i32::from(map.red_spawn[1]),
                ),
            },
        })
    }

    /// Hashes the reconstructed catalogue with [`GameContent::hash`].
    ///
    /// # Errors
    /// Returns [`WireError::BadValue`] for an invalid catalogue.
    pub fn content_hash(&self) -> Result<u64, WireError> {
        Ok(self.to_content()?.hash())
    }
}

fn put_wire_name<const N: usize>(target: &mut [u8; N], name: &str) -> u8 {
    let bytes = name.as_bytes();
    let length = bytes.len().min(N);
    target[..length].copy_from_slice(&bytes[..length]);
    u8::try_from(length).unwrap_or(u8::MAX)
}

fn read_wire_name(bytes: &[u8], used: u8, capacity: usize) -> Result<String, WireError> {
    let used = usize::from(used);
    if used > capacity || used > bytes.len() {
        return Err(WireError::BadValue);
    }
    String::from_utf8(bytes[..used].to_vec()).map_err(|_| WireError::BadValue)
}

const fn damage_type_code(damage_type: DamageType) -> u8 {
    match damage_type {
        DamageType::Physical => 0,
        DamageType::Magic => 1,
    }
}

const fn damage_type_from_code(code: u8) -> Result<DamageType, WireError> {
    match code {
        0 => Ok(DamageType::Physical),
        1 => Ok(DamageType::Magic),
        _ => Err(WireError::BadValue),
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

#[allow(clippy::too_many_lines)] // Wire field order mirrors `decode_catalog`.
fn encode_catalog(catalog: &WireCatalog, out: &mut Vec<u8>) {
    out.extend_from_slice(&catalog.manifest_name);
    out.push(catalog.manifest_name_len);
    out.extend_from_slice(&catalog.manifest_version);
    out.push(catalog.manifest_version_len);
    out.extend_from_slice(&catalog.manifest_author);
    out.push(catalog.manifest_author_len);
    out.push(catalog.item_count);
    for item in &catalog.items {
        out.extend_from_slice(&item.name);
        out.push(item.name_len);
        put_u32(out, item.cost);
        put_u32(out, item.health as u32);
        put_u32(out, item.mana as u32);
        put_u32(out, item.attack_damage as u32);
        put_u32(out, item.armor as u32);
        put_u32(out, item.magic_resist as u32);
        out.extend_from_slice(&item.move_speed_bits.to_le_bytes());
    }
    out.push(catalog.hero_count);
    for hero in &catalog.heroes {
        out.extend_from_slice(&hero.name);
        out.push(hero.name_len);
        for value in [
            hero.max_health,
            hero.max_mana,
            hero.health_regen_per_tick,
            hero.mana_regen_per_tick,
            hero.attack_damage,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&hero.attack_range_bits.to_le_bytes());
        out.extend_from_slice(&hero.attack_interval_ticks.to_le_bytes());
        out.extend_from_slice(&hero.move_speed_bits.to_le_bytes());
        out.extend_from_slice(&hero.armor.to_le_bytes());
        out.extend_from_slice(&hero.magic_resist.to_le_bytes());
        out.extend_from_slice(&hero.vision_bits.to_le_bytes());
        out.extend_from_slice(&hero.abilities);
        for value in [
            hero.health_per_level,
            hero.mana_per_level,
            hero.attack_damage_per_level,
            hero.armor_per_level,
            hero.magic_resist_per_level,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out.push(catalog.ability_count);
    for ability in &catalog.abilities {
        out.extend_from_slice(&ability.name);
        out.push(ability.name_len);
        out.extend_from_slice(&ability.cooldown_ticks.to_le_bytes());
        out.extend_from_slice(&ability.mana_cost.to_le_bytes());
        out.extend_from_slice(&ability.cast_range_bits.to_le_bytes());
        out.extend_from_slice(&ability.radius_bits.to_le_bytes());
        out.push(ability.targeting_kind);
        out.extend_from_slice(&ability.targeting_range_bits.to_le_bytes());
        out.push(ability.effect_count);
        for effect in ability
            .effects
            .iter()
            .take(usize::from(ability.effect_count))
        {
            out.push(effect.kind);
            out.extend_from_slice(&effect.amount.to_le_bytes());
            out.push(effect.damage_type);
            out.extend_from_slice(&effect.speed_bits.to_le_bytes());
            out.extend_from_slice(&effect.range_bits.to_le_bytes());
            out.extend_from_slice(&effect.radius_bits.to_le_bytes());
            out.extend_from_slice(&effect.arc_bits.to_le_bytes());
            out.extend_from_slice(&effect.duration_ticks.to_le_bytes());
            out.extend_from_slice(&effect.period_ticks.to_le_bytes());
            out.push(effect.area_count);
            for area in effect.area.iter().take(usize::from(effect.area_count)) {
                out.push(area.kind);
                out.extend_from_slice(&area.modifier.to_le_bytes());
                out.extend_from_slice(&area.duration_ticks.to_le_bytes());
                out.extend_from_slice(&area.factor_bits.to_le_bytes());
                out.extend_from_slice(&area.amount.to_le_bytes());
                out.push(area.damage_type);
            }
        }
    }
    out.push(catalog.modifier_count);
    for modifier in &catalog.modifiers {
        out.extend_from_slice(&modifier.name);
        out.push(modifier.name_len);
        out.push(modifier.kind);
    }
    let map = &catalog.map;
    out.extend_from_slice(&map.extent.to_le_bytes());
    out.push(map.lane_count);
    for lane in &map.lanes {
        out.extend_from_slice(&lane.name);
        out.push(lane.name_len);
        out.push(lane.point_count);
        for point in &lane.points {
            out.extend_from_slice(&point[0].to_le_bytes());
            out.extend_from_slice(&point[1].to_le_bytes());
        }
    }
    for point in [map.blue_spawn, map.red_spawn] {
        out.extend_from_slice(&point[0].to_le_bytes());
        out.extend_from_slice(&point[1].to_le_bytes());
    }
    out.push(map.structure_count);
    for structure in &map.structures {
        out.push(structure.team);
        out.push(structure.kind);
        out.extend_from_slice(&structure.position[0].to_le_bytes());
        out.extend_from_slice(&structure.position[1].to_le_bytes());
        out.push(structure.lane);
    }
    out.push(map.wall_count);
    for wall in &map.walls {
        for point in [wall.a, wall.b] {
            out.extend_from_slice(&point[0].to_le_bytes());
            out.extend_from_slice(&point[1].to_le_bytes());
        }
    }
}

#[allow(clippy::too_many_lines)] // Wire field order mirrors `encode_catalog`.
fn decode_catalog(reader: &mut Reader<'_>) -> Result<WireCatalog, WireError> {
    let mut catalog = WireCatalog::default();
    catalog
        .manifest_name
        .copy_from_slice(reader.bytes(WIRE_MANIFEST_FIELD)?);
    catalog.manifest_name_len = reader.u8()?;
    catalog
        .manifest_version
        .copy_from_slice(reader.bytes(WIRE_MANIFEST_FIELD)?);
    catalog.manifest_version_len = reader.u8()?;
    catalog
        .manifest_author
        .copy_from_slice(reader.bytes(WIRE_MANIFEST_FIELD)?);
    catalog.manifest_author_len = reader.u8()?;
    catalog.item_count = reader.u8()?;
    for item in &mut catalog.items {
        item.name.copy_from_slice(reader.bytes(WIRE_ITEM_NAME)?);
        item.name_len = reader.u8()?;
        item.cost = reader.u32()?;
        item.health = reader.i32()?;
        item.mana = reader.i32()?;
        item.attack_damage = reader.i32()?;
        item.armor = reader.i32()?;
        item.magic_resist = reader.i32()?;
        item.move_speed_bits = reader.i64()?;
    }
    catalog.hero_count = reader.u8()?;
    for hero in &mut catalog.heroes {
        hero.name.copy_from_slice(reader.bytes(WIRE_HERO_NAME)?);
        hero.name_len = reader.u8()?;
        hero.max_health = reader.i32()?;
        hero.max_mana = reader.i32()?;
        hero.health_regen_per_tick = reader.i32()?;
        hero.mana_regen_per_tick = reader.i32()?;
        hero.attack_damage = reader.i32()?;
        hero.attack_range_bits = reader.i64()?;
        hero.attack_interval_ticks = reader.u32()?;
        hero.move_speed_bits = reader.i64()?;
        hero.armor = reader.i32()?;
        hero.magic_resist = reader.i32()?;
        hero.vision_bits = reader.i64()?;
        hero.abilities.copy_from_slice(reader.bytes(3)?);
        hero.health_per_level = reader.i32()?;
        hero.mana_per_level = reader.i32()?;
        hero.attack_damage_per_level = reader.i32()?;
        hero.armor_per_level = reader.i32()?;
        hero.magic_resist_per_level = reader.i32()?;
    }
    catalog.ability_count = reader.u8()?;
    for ability in &mut catalog.abilities {
        ability
            .name
            .copy_from_slice(reader.bytes(WIRE_ABILITY_NAME)?);
        ability.name_len = reader.u8()?;
        ability.cooldown_ticks = reader.u32()?;
        ability.mana_cost = reader.i32()?;
        ability.cast_range_bits = reader.i64()?;
        ability.radius_bits = reader.i64()?;
        ability.targeting_kind = reader.u8()?;
        ability.targeting_range_bits = reader.i64()?;
        ability.effect_count = reader.u8()?;
        if usize::from(ability.effect_count) > MAX_WIRE_EFFECTS {
            return Err(WireError::BadValue);
        }
        for index in 0..usize::from(ability.effect_count) {
            let effect = &mut ability.effects[index];
            effect.kind = reader.u8()?;
            effect.amount = reader.i32()?;
            effect.damage_type = reader.u8()?;
            effect.speed_bits = reader.i64()?;
            effect.range_bits = reader.i64()?;
            effect.radius_bits = reader.i64()?;
            effect.arc_bits = reader.i64()?;
            effect.duration_ticks = reader.u32()?;
            effect.period_ticks = reader.u32()?;
            effect.area_count = reader.u8()?;
            if usize::from(effect.area_count) > MAX_WIRE_AREA_EFFECTS {
                return Err(WireError::BadValue);
            }
            for area_index in 0..usize::from(effect.area_count) {
                let area = &mut effect.area[area_index];
                area.kind = reader.u8()?;
                area.modifier = reader.u16()?;
                area.duration_ticks = reader.u32()?;
                area.factor_bits = reader.i64()?;
                area.amount = reader.i32()?;
                area.damage_type = reader.u8()?;
            }
        }
    }
    catalog.modifier_count = reader.u8()?;
    for modifier in &mut catalog.modifiers {
        modifier
            .name
            .copy_from_slice(reader.bytes(WIRE_MODIFIER_NAME)?);
        modifier.name_len = reader.u8()?;
        modifier.kind = reader.u8()?;
    }
    let map = &mut catalog.map;
    map.extent = reader.u16()?;
    map.lane_count = reader.u8()?;
    for lane in &mut map.lanes {
        lane.name.copy_from_slice(reader.bytes(WIRE_LANE_NAME)?);
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
    for point in [&mut map.blue_spawn, &mut map.red_spawn] {
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
    map.structure_count = reader.u8()?;
    for structure in &mut map.structures {
        structure.team = reader.u8()?;
        structure.kind = reader.u8()?;
        structure.position[0] = i16::from_le_bytes(
            reader
                .bytes(2)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        );
        structure.position[1] = i16::from_le_bytes(
            reader
                .bytes(2)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        );
        structure.lane = reader.u8()?;
    }
    map.wall_count = reader.u8()?;
    for wall in &mut map.walls {
        for point in [&mut wall.a, &mut wall.b] {
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
    Ok(catalog)
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
    /// Legacy u8 flags (dead + standard status tint bits).
    pub flags: u8,
    /// Status bitset (wide; includes burn/haste and future tags).
    pub status_bits: u16,
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
/// A movement slow is active.
pub const ENTITY_FLAG_SLOW: u8 = 1 << 2;
/// The unit is rooted.
pub const ENTITY_FLAG_ROOT: u8 = 1 << 3;
/// The unit is silenced.
pub const ENTITY_FLAG_SILENCE: u8 = 1 << 4;
/// The unit is burning.
pub const ENTITY_FLAG_BURN: u8 = 1 << 5;
/// A movement haste is active.
pub const ENTITY_FLAG_HASTE: u8 = 1 << 6;

/// Wide status bit: dead hero.
pub const STATUS_DEAD: u16 = 1 << 0;
/// Wide status bit: shielded.
pub const STATUS_SHIELD: u16 = 1 << 1;
/// Wide status bit: slowed.
pub const STATUS_SLOW: u16 = 1 << 2;
/// Wide status bit: rooted.
pub const STATUS_ROOT: u16 = 1 << 3;
/// Wide status bit: silenced.
pub const STATUS_SILENCE: u16 = 1 << 4;
/// Wide status bit: burning.
pub const STATUS_BURN: u16 = 1 << 5;
/// Wide status bit: hasted.
pub const STATUS_HASTE: u16 = 1 << 6;

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
                | (if unit.shielded { ENTITY_FLAG_SHIELD } else { 0 })
                | (if unit.slowed { ENTITY_FLAG_SLOW } else { 0 })
                | (if unit.rooted { ENTITY_FLAG_ROOT } else { 0 })
                | (if unit.silenced {
                    ENTITY_FLAG_SILENCE
                } else {
                    0
                })
                | (if unit.burning { ENTITY_FLAG_BURN } else { 0 })
                | (if unit.hasted { ENTITY_FLAG_HASTE } else { 0 }),
            status_bits: (if unit.dead { STATUS_DEAD } else { 0 })
                | (if unit.shielded { STATUS_SHIELD } else { 0 })
                | (if unit.slowed { STATUS_SLOW } else { 0 })
                | (if unit.rooted { STATUS_ROOT } else { 0 })
                | (if unit.silenced { STATUS_SILENCE } else { 0 })
                | (if unit.burning { STATUS_BURN } else { 0 })
                | (if unit.hasted { STATUS_HASTE } else { 0 }),
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
            status_bits: (self.status_bits != previous.status_bits).then_some(self.status_bits),
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
        if let Some(status_bits) = update.status_bits {
            self.status_bits = status_bits;
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
    /// Wide status bitset when changed.
    pub status_bits: Option<u16>,
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
const UPDATE_STATUS_BITS: u8 = 1 << 6;

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
    /// Presentation events since the previous snapshot.
    pub events: Vec<NetEvent>,
}

/// Maximum presentation events per snapshot.
pub const MAX_SNAPSHOT_EVENTS: usize = 16;

/// One presentation event in a snapshot.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct NetEvent {
    /// Event kind (see `EVENT_*` constants).
    pub kind: u8,
    /// Primary team (caster/owner/victim depending on kind).
    pub team: u8,
    /// Source entity net id (`0xFFFF` none).
    pub source: u16,
    /// Target entity net id (`0xFFFF` none).
    pub target: u16,
    /// Source hero slot (`0xFF` none).
    pub source_slot: u8,
    /// Target hero slot (`0xFF` none).
    pub target_slot: u8,
    /// Quantised primary position.
    pub x: u16,
    /// Quantised primary position.
    pub y: u16,
    /// Quantised secondary/end position.
    pub x2: u16,
    /// Quantised secondary/end position.
    pub y2: u16,
    /// Kind-specific value (ability id, unit kind, speed).
    pub value: u16,
    /// Kind-specific auxiliary value (radius, wave).
    pub aux: u16,
    /// Normalised direction X (`i16::MIN..=i16::MAX`).
    pub dx: i16,
    /// Normalised direction Y (`i16::MIN..=i16::MAX`).
    pub dy: i16,
    /// Visual projectile arc height in 1/16 world units.
    pub arc: i16,
}

/// Ability cast event.
pub const EVENT_CAST: u8 = 1;
/// Projectile spawned event.
pub const EVENT_PROJECTILE_SPAWN: u8 = 2;
/// Projectile hit event.
pub const EVENT_PROJECTILE_HIT: u8 = 3;
/// Unit died event.
pub const EVENT_UNIT_DIED: u8 = 4;
/// Structure destroyed event.
pub const EVENT_STRUCTURE_DESTROYED: u8 = 5;
/// Match ended event.
pub const EVENT_MATCH_ENDED: u8 = 6;
/// Zone spawned event.
pub const EVENT_ZONE_SPAWNED: u8 = 7;

impl SnapshotDelta {
    /// Whether the delta carries no state at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spawns.is_empty()
            && self.updates.is_empty()
            && self.despawns.is_empty()
            && self.events.is_empty()
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
    let mut delta = SnapshotDelta::default();
    build_delta_into(
        current,
        baseline,
        tick,
        acked_input,
        baseline_seq,
        &mut delta,
    );
    delta
}

/// Builds a snapshot into a reused delta, retaining vector capacity.
pub fn build_delta_into(
    current: &[NetEntity],
    baseline: Option<&[NetEntity]>,
    tick: u32,
    acked_input: u32,
    baseline_seq: u32,
    delta: &mut SnapshotDelta,
) {
    delta.clear();
    delta.tick = tick;
    delta.acked_input = acked_input;
    let Some(baseline) = baseline else {
        delta.baseline_seq = 0;
        delta.spawns.extend_from_slice(current);
        return;
    };
    delta.baseline_seq = baseline_seq;
    let (mut old_index, mut new_index) = (0usize, 0usize);
    while old_index < baseline.len() || new_index < current.len() {
        match (baseline.get(old_index), current.get(new_index)) {
            (Some(old), Some(new)) if old.net_id == new.net_id => {
                let update = new.diff(old);
                if update.position.is_some()
                    || update.health.is_some()
                    || update.mana.is_some()
                    || update.flags.is_some()
                    || update.status_bits.is_some()
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
}

impl SnapshotDelta {
    fn clear(&mut self) {
        self.tick = 0;
        self.acked_input = 0;
        self.baseline_seq = 0;
        self.blue_score = 0;
        self.red_score = 0;
        self.outcome = 0;
        self.phase = 0;
        self.countdown = 0;
        self.ready_mask = 0;
        self.player_mask = 0;
        self.spawns.clear();
        self.updates.clear();
        self.despawns.clear();
        self.events.clear();
    }
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
#[allow(clippy::too_many_lines)] // Field order mirrors the decoder one-for-one.
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
        put_u16(out, spawn.status_bits);
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
        if update.status_bits.is_some() {
            mask |= UPDATE_STATUS_BITS;
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
        if let Some(status_bits) = update.status_bits {
            put_u16(out, status_bits);
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
    out.push(u8::try_from(delta.events.len()).map_err(|_| WireError::TooMany)?);
    for event in &delta.events {
        out.push(event.kind);
        out.push(event.team);
        put_u16(out, event.source);
        put_u16(out, event.target);
        out.push(event.source_slot);
        out.push(event.target_slot);
        put_u16(out, event.x);
        put_u16(out, event.y);
        put_u16(out, event.x2);
        put_u16(out, event.y2);
        put_u16(out, event.value);
        put_u16(out, event.aux);
        out.extend_from_slice(&event.dx.to_le_bytes());
        out.extend_from_slice(&event.dy.to_le_bytes());
        out.extend_from_slice(&event.arc.to_le_bytes());
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
    if spawn_count > reader.remaining() / MIN_SPAWN_BYTES
        || update_count > reader.remaining() / MIN_UPDATE_BYTES
        || despawn_count > reader.remaining() / MIN_DESPAWN_BYTES
    {
        return Err(WireError::BadValue);
    }
    let mut spawns = Vec::with_capacity(spawn_count);
    for _ in 0..spawn_count {
        spawns.push(NetEntity {
            net_id: reader.u16()?,
            kind: reader.u8()?,
            team: reader.u8()?,
            hero_slot: reader.u8()?,
            hero_pick: reader.u8()?,
            flags: reader.u8()?,
            status_bits: reader.u16()?,
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
            | UPDATE_PROGRESS
            | UPDATE_STATUS_BITS;
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
        let status_bits = (mask & UPDATE_STATUS_BITS != 0)
            .then(|| reader.u16())
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
            status_bits,
            cooldowns,
            progress,
        });
    }
    let mut despawns = Vec::with_capacity(despawn_count);
    for _ in 0..despawn_count {
        despawns.push(reader.u16()?);
    }
    let event_count = usize::from(reader.u8()?);
    if event_count > MAX_SNAPSHOT_EVENTS {
        return Err(WireError::BadValue);
    }
    let mut events = Vec::with_capacity(event_count);
    for _ in 0..event_count {
        events.push(NetEvent {
            kind: reader.u8()?,
            team: reader.u8()?,
            source: reader.u16()?,
            target: reader.u16()?,
            source_slot: reader.u8()?,
            target_slot: reader.u8()?,
            x: reader.u16()?,
            y: reader.u16()?,
            x2: reader.u16()?,
            y2: reader.u16()?,
            value: reader.u16()?,
            aux: reader.u16()?,
            dx: i16::from_le_bytes(
                reader
                    .bytes(2)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            ),
            dy: i16::from_le_bytes(
                reader
                    .bytes(2)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            ),
            arc: i16::from_le_bytes(
                reader
                    .bytes(2)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            ),
        });
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
        events,
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

    fn i32(&mut self) -> Result<i32, WireError> {
        Ok(i32::from_le_bytes(
            self.bytes(4)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        ))
    }

    fn i64(&mut self) -> Result<i64, WireError> {
        Ok(i64::from_le_bytes(
            self.bytes(8)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        ))
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
    fn welcome_round_trip_carries_verified_catalogue() {
        let content = GameContent::default();
        let welcome = Welcome {
            slot: 3,
            tick_rate: 30,
            snapshot_interval: 1,
            tick: 42,
            seed: 0x1234,
            hero_pick: 2,
            nonce: 7,
            server_nonce: 9,
            auth: [0; 32],
            spectator: false,
            catalog: WireCatalog::from(&content),
            content_hash: content.hash(),
            identity: false,
            server_public_key: [0; 32],
            server_signature: [0; 64],
        };
        let encoded = encode_welcome(&welcome);
        let decoded = decode_welcome(&encoded).expect("welcome decodes");
        assert_eq!(decoded, welcome);
        assert_eq!(
            decoded.catalog.content_hash().expect("catalogue hash"),
            decoded.content_hash
        );
    }

    #[test]
    fn catalogue_round_trip_preserves_content_hash() {
        let content = GameContent::default();
        let wire = WireCatalog::from(&content);
        assert_eq!(wire.content_hash().expect("catalogue hash"), content.hash());
        let mut bytes = Vec::new();
        encode_catalog(&wire, &mut bytes);
        let mut reader = Reader::new(&bytes);
        let decoded = decode_catalog(&mut reader).expect("decode catalogue");
        assert_eq!(reader.remaining(), 0);
        assert_eq!(decoded, wire);
        assert_eq!(
            decoded.to_content().expect("content").hash(),
            content.hash()
        );

        let mut changed = GameContent::default();
        changed.heroes[0].attack_damage += 1;
        assert_ne!(
            WireCatalog::from(&changed).content_hash(),
            wire.content_hash()
        );
        let mut changed = GameContent::default();
        if let moba_game::Targeting::Direction { range } = changed.abilities[0].targeting {
            changed.abilities[0].targeting = moba_game::Targeting::Direction {
                range: range + Fix::ONE,
            };
        }
        assert_ne!(
            WireCatalog::from(&changed).content_hash(),
            wire.content_hash()
        );
    }

    #[test]
    fn oversized_snapshot_counts_are_rejected() {
        let delta = SnapshotDelta {
            tick: 1,
            acked_input: 0,
            baseline_seq: 0,
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
            events: Vec::new(),
        };
        let mut payload = Vec::new();
        encode_snapshot(&delta, &mut payload).expect("encode");
        let mut corrupted = payload.clone();
        corrupted[28..30].copy_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(decode_snapshot(&corrupted), Err(WireError::BadValue));
        let mut corrupted = payload.clone();
        corrupted[30..32].copy_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(decode_snapshot(&corrupted), Err(WireError::BadValue));
        let mut corrupted = payload;
        corrupted[32..34].copy_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(decode_snapshot(&corrupted), Err(WireError::BadValue));
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
            identity: false,
            client_public: [0; 32],
            client_signature: [0; 64],
            server_challenge: 0,
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
            status_bits: 0,
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
    fn wire_catalogue_round_trips_map_extent() {
        use moba_game::GameContent;
        let mut content = GameContent::default();
        content.map.extent = moba_core::Fix::from_i32(128);
        content.map.blue_spawn = moba_core::FixVec2::from_ints(10, 10);
        content.map.red_spawn = moba_core::FixVec2::from_ints(118, 118);
        let wire = WireCatalog::from(&content);
        assert_eq!(wire.map.extent, 128);
        let decoded = wire.to_content().expect("round trip");
        assert_eq!(decoded.map.extent, content.map.extent);
        assert_eq!(decoded.hash(), content.hash());
    }

    #[test]
    fn wire_catalogue_round_trips_nonzero_projectile_arc() {
        use moba_game::{Effect, GameContent};
        let mut content = GameContent::default();
        content.abilities[2].effects.effects[0] = Effect::Projectile {
            speed_per_sec: moba_core::Fix::from_i32(26),
            range: moba_core::Fix::from_i32(11),
            radius: moba_core::Fix::ONE,
            arc: moba_core::Fix::from_i32(3),
            on_hit: moba_game::AreaEffectList::one(moba_game::AreaEffect::Damage {
                amount: 10,
                damage_type: moba_game::DamageType::Physical,
            }),
        };
        let wire = WireCatalog::from(&content);
        assert_eq!(wire.abilities[2].effects[0].arc_bits, 3i64 << 32);
        let decoded = wire.to_content().expect("round trip");
        assert_eq!(decoded.hash(), content.hash());
    }

    #[test]
    fn delta_builder_reuses_capacity() {
        let baseline = vec![entity(1, 10, 20, 600), entity(2, 30, 40, 200)];
        let mut delta = SnapshotDelta::default();
        build_delta_into(&baseline, None, 1, 0, 0, &mut delta);
        let spawn_capacity = delta.spawns.capacity();
        assert_eq!(delta.spawns.len(), 2);
        build_delta_into(&baseline, None, 2, 0, 0, &mut delta);
        assert_eq!(delta.spawns.capacity(), spawn_capacity);
        assert_eq!(delta.spawns.len(), 2);
        assert_eq!(delta.tick, 2);
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
        delta.events.push(NetEvent {
            kind: EVENT_PROJECTILE_SPAWN,
            team: 0,
            source: 1,
            target: 0xFFFF,
            source_slot: 0,
            target_slot: 0xFF,
            x: 10,
            y: 20,
            x2: 30,
            y2: 40,
            value: 24,
            aux: 1,
            dx: 16_383,
            dy: -16_383,
            arc: 40,
        });
        assert_eq!(delta.spawns.len(), 1);
        assert_eq!(delta.updates.len(), 2);
        encode_snapshot(&delta, &mut payload).unwrap();
        let decoded = decode_snapshot(&payload).unwrap();
        assert_eq!(decoded.events, delta.events);
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
            slowed: false,
            rooted: false,
            silenced: false,
            burning: true,
            hasted: true,
        };
        let entity = NetEntity::from_observed(4, &unit);
        assert_eq!(entity.net_id, 4);
        assert_eq!(entity.position(), unit.position);
        assert_eq!(entity.hero_pick, 2);
        assert_eq!(entity.progress.level, 4);
        assert_eq!(entity.progress.gold, 950);
        assert!(!entity.is_dead());
        assert!(entity.flags & ENTITY_FLAG_SHIELD != 0);
        assert!(entity.flags & ENTITY_FLAG_BURN != 0);
        assert!(entity.flags & ENTITY_FLAG_HASTE != 0);
        assert!(entity.status_bits & STATUS_SHIELD != 0);
        assert!(entity.status_bits & STATUS_BURN != 0);
        assert!(entity.status_bits & STATUS_HASTE != 0);
    }
}
