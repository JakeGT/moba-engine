//! Client-local VFX descriptors and the procedural particle atlas (ADR-0013).
//!
//! An **effect** is a small TOML file, `<asset root>/vfx/<name>.toml`, holding up to
//! [`MAX_VFX_EMITTERS`] particle emitters:
//!
//! ```toml
//! duration = 0.6           # seconds the effect emits (0..=30)
//! importance = "high"      # low | normal | high: budget priority (default normal)
//! event_duration = false   # zones: emit for the event's lifetime instead of `duration`
//!
//! [[emitters]]
//! primitive = "sprite"     # sprite | flipbook | trail | beam | ring | mesh
//! blend = "additive"       # additive | alpha (weighted blended OIT; Low draws it unsorted)
//! sprite = "soft"          # an atlas cell, see VFX_SPRITES
//! anchor = "target"        # target | source | line (from the source to the target)
//! burst = 24               # particles at `delay`
//! rate = 0                 # particles per second while the effect runs
//! lifetime = [0.3, 0.6]    # seconds, uniform in the range
//! speed = [2.0, 6.0]       # metres per second
//! direction = "out"        # up | out | random | forward | none
//! shape = "sphere"         # point | sphere | disc | circle | line
//! radius = 0.5             # spawn shape radius, metres
//! size = [0.4, 1.2]        # start size, metres
//! size_end = 0.2           # size multiplier at the end of life
//! color = [1.0, 0.6, 0.2, 1.0]      # linear RGBA; RGB may exceed 1 (HDR, up to 32)
//! color_end = [0.8, 0.1, 0.0, 0.0]
//! gravity = -2.0           # m/s² along +Y
//! drag = 1.0               # 1/s
//! ```
//!
//! A **bindings** file, `<asset root>/vfx/bindings.toml`, maps catalogue ability names and
//! unit events to effect names (see [`VfxBindings`]). Both files are untrusted, client-local
//! and cosmetic: they never reach the simulation, the wire, replays or the content hash.
//! Every parser here reads at most a fixed number of bytes, rejects unknown keys and
//! range-checks every number, so a hostile file can only fail to load; the client then draws
//! no effect (or the bindings' default) and logs it once.
//!
//! [`procedural_atlas`] generates the built-in 512×512 particle atlas (an 8×8 grid of 64-pixel
//! cells, `RG8`: R is coverage, G is a hot-core intensity) that the renderer uses until a
//! `vfx/atlas.ktx2` replaces it. Its layout is [`VFX_SPRITES`].

use std::io::Read;
use std::path::Path;

use serde::Deserialize;

use crate::texture::{TextureData, TextureFormat, full_mip_chain, mip_size};

/// Largest effect descriptor accepted, in bytes.
pub const MAX_VFX_BYTES: u64 = 16 * 1024;
/// Largest bindings file accepted, in bytes.
pub const MAX_VFX_BINDINGS_BYTES: u64 = 32 * 1024;
/// Most emitters in one effect.
pub const MAX_VFX_EMITTERS: usize = 8;
/// Most ability bindings in one bindings file.
pub const MAX_VFX_ABILITY_BINDINGS: usize = 128;
/// Longest effect, seconds.
pub const MAX_EFFECT_SECONDS: f32 = 30.0;
/// Longest particle lifetime, seconds.
pub const MAX_PARTICLE_SECONDS: f32 = 10.0;
/// Most particles one emitter bursts at once.
pub const MAX_VFX_BURST: u32 = 512;
/// Highest continuous emission rate, particles per second.
pub const MAX_VFX_RATE: f32 = 1_024.0;
/// Longest effect, ability or socket name.
pub const MAX_VFX_NAME: usize = 48;
/// Atlas cells per row (and rows) of the particle atlas.
pub const VFX_ATLAS_GRID: u32 = 8;
/// Pixels per atlas cell in the built-in atlas.
pub const VFX_ATLAS_CELL: u32 = 64;
/// Atlas cell of the `solid` sprite (full coverage; mesh particles use it).
pub const VFX_SOLID_CELL: u8 = 52;

const SPEED_RANGE: (f32, f32) = (0.0, 200.0);
const SIZE_RANGE: (f32, f32) = (0.0, 100.0);
const SIZE_END_RANGE: (f32, f32) = (0.0, 16.0);
const COLOR_RANGE: (f32, f32) = (0.0, 32.0);
const GRAVITY_RANGE: (f32, f32) = (-200.0, 200.0);
const DRAG_RANGE: (f32, f32) = (0.0, 50.0);
const SPIN_RANGE: (f32, f32) = (-3_600.0, 3_600.0);
const RADIUS_RANGE: (f32, f32) = (0.0, 50.0);
const OFFSET_RANGE: (f32, f32) = (-50.0, 50.0);
const WIDTH_RANGE: (f32, f32) = (0.0, 20.0);
const FPS_RANGE: (f32, f32) = (0.0, 120.0);
const SPREAD_RANGE: (f32, f32) = (0.0, 180.0);
const DELAY_RANGE: (f32, f32) = (0.0, MAX_EFFECT_SECONDS);
const TINT_RANGE: (f32, f32) = (0.0, 4.0);

/// One named region of the particle atlas: a single cell or a flipbook strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VfxSprite {
    /// Sprite name as written in descriptors.
    pub name: &'static str,
    /// First cell (row-major in the 8×8 grid).
    pub cell: u8,
    /// Flipbook frames (consecutive cells); 1 for a still sprite.
    pub frames: u8,
}

const fn sprite(name: &'static str, cell: u8, frames: u8) -> VfxSprite {
    VfxSprite { name, cell, frames }
}

/// Every sprite of the atlas layout, by name. A replacement `vfx/atlas.ktx2` must keep this
/// layout (an 8×8 grid of equal cells).
pub const VFX_SPRITES: &[VfxSprite] = &[
    sprite("soft", 0, 1),
    sprite("dot", 1, 1),
    sprite("ring", 2, 1),
    sprite("spark", 3, 1),
    sprite("star", 4, 1),
    sprite("shard", 5, 1),
    sprite("rune", 6, 1),
    sprite("beam", 7, 1),
    sprite("burst", 8, 16),
    sprite("smoke", 24, 16),
    sprite("lightning", 40, 8),
    sprite("noise", 48, 1),
    sprite("swirl", 49, 1),
    sprite("cross", 50, 1),
    sprite("halo", 51, 1),
    sprite("solid", VFX_SOLID_CELL, 1),
];

/// The sprite called `name`, if the atlas has one.
#[must_use]
pub fn vfx_sprite(name: &str) -> Option<VfxSprite> {
    VFX_SPRITES
        .iter()
        .copied()
        .find(|sprite| sprite.name.eq_ignore_ascii_case(name))
}

/// What an emitter draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum VfxPrimitive {
    /// A camera-facing quad with one atlas cell.
    #[default]
    Sprite,
    /// A camera-facing quad that plays a multi-frame sprite.
    Flipbook,
    /// Segments from the anchor's previous to its current position each frame: a ribbon
    /// behind projectiles and dashes.
    Trail,
    /// A segment from the source to the target, re-emitted while the effect runs.
    Beam,
    /// A quad flat on the ground (shockwaves, area telegraphs).
    Ring,
    /// A registered mesh (`<asset root>/models/<mesh>.glb`, or the built-in `cube`).
    Mesh,
}

/// How an emitter's particles blend into the HDR scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum VfxBlend {
    /// Added to the scene: order-independent, never darkens.
    #[default]
    Additive,
    /// Alpha blended through weighted blended order-independent transparency.
    Alpha,
}

/// Where an emitter spawns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum VfxAnchor {
    /// The event's position (the cast target, impact or zone centre); a projectile's
    /// current position for projectile effects.
    #[default]
    Target,
    /// The event's source unit, followed while the effect runs (at its socket when the
    /// binding names one).
    Source,
    /// Along the segment from the source to the target.
    Line,
}

/// Initial particle direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum VfxDirection {
    /// Straight up, widened by `spread`.
    Up,
    /// Away from the spawn centre, in the horizontal plane for discs and circles.
    #[default]
    Out,
    /// Uniformly random.
    Random,
    /// Along the anchor's travel (projectiles) or the source → target direction, widened by
    /// `spread`.
    Forward,
    /// No initial velocity.
    None,
}

/// Spawn-position distribution around the anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum VfxShape {
    /// Exactly at the anchor.
    #[default]
    Point,
    /// Inside a sphere of `radius`.
    Sphere,
    /// Inside a horizontal disc of `radius`.
    Disc,
    /// On a horizontal circle of `radius`.
    Circle,
}

/// Budget priority of an effect. Enemy telegraphs rank above every class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[non_exhaustive]
pub enum VfxImportance {
    /// Cosmetic (basic-attack sparks, minion deaths): first to be culled.
    Low,
    /// The default.
    #[default]
    Normal,
    /// Hero abilities and deaths.
    High,
}

/// One validated emitter.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct VfxEmitter {
    /// What it draws.
    pub primitive: VfxPrimitive,
    /// How it blends.
    pub blend: VfxBlend,
    /// Atlas sprite (`solid` for meshes).
    pub sprite: VfxSprite,
    /// Mesh name for [`VfxPrimitive::Mesh`] (`cube` or a model name).
    pub mesh: Option<String>,
    /// Where it spawns.
    pub anchor: VfxAnchor,
    /// Particles emitted at `delay`.
    pub burst: u32,
    /// Particles per second from `delay` until the effect (or `stop`) ends.
    pub rate: f32,
    /// Seconds after the effect starts before this emitter starts.
    pub delay: f32,
    /// Seconds after the effect starts at which continuous emission stops (`None`: with the
    /// effect).
    pub stop: Option<f32>,
    /// Particle lifetime range, seconds.
    pub lifetime: [f32; 2],
    /// Initial speed range, metres per second.
    pub speed: [f32; 2],
    /// Initial direction.
    pub direction: VfxDirection,
    /// Cone half-angle around `up`/`forward`, degrees.
    pub spread: f32,
    /// Spawn shape.
    pub shape: VfxShape,
    /// Shape radius, metres.
    pub radius: f32,
    /// Scale `radius` and particle sizes by the event's radius (zones, areas).
    pub scale_with_radius: bool,
    /// Offset from the anchor, metres (`y` is up).
    pub offset: [f32; 3],
    /// Start size range, metres.
    pub size: [f32; 2],
    /// Size multiplier at the end of life.
    pub size_end: f32,
    /// Start colour, linear RGBA.
    pub color: [f32; 4],
    /// End colour, linear RGBA.
    pub color_end: [f32; 4],
    /// How much the colour is pulled towards the team colour, `0..=1`.
    pub team_tint: f32,
    /// Acceleration along +Y, m/s².
    pub gravity: f32,
    /// Linear drag, 1/s.
    pub drag: f32,
    /// Rotation speed range, degrees per second.
    pub spin: [f32; 2],
    /// Segment width for trails and beams, metres.
    pub width: f32,
    /// Flipbook frames per second (`0` spreads the frames over the particle's life).
    pub fps: f32,
    /// An area telegraph: enemy instances keep a minimum opacity and are never culled by
    /// budgets (roadmap §2 readability rules).
    pub telegraph: bool,
}

/// A validated effect descriptor.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct VfxEffect {
    /// Seconds the effect emits.
    pub duration: f32,
    /// Budget priority.
    pub importance: VfxImportance,
    /// Emit for the triggering event's lifetime (a zone's duration) instead of `duration`.
    pub event_duration: bool,
    /// Emitters, in file order.
    pub emitters: Vec<VfxEmitter>,
}

/// Why a VFX descriptor or bindings file could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum VfxError {
    /// The file does not exist or cannot be read; the text is the I/O error.
    Io(String),
    /// The path is not a regular file.
    NotAFile,
    /// The file is larger than its limit.
    TooLarge,
    /// TOML syntax or schema error (unknown key, wrong type, missing field).
    Toml(String),
    /// A value is out of range; the text names it.
    Invalid(&'static str),
}

impl core::fmt::Display for VfxError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "missing or unreadable ({error})"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::TooLarge => f.write_str("larger than its size limit"),
            Self::Toml(error) => write!(f, "VFX TOML: {}", error.trim_end()),
            Self::Invalid(what) => write!(f, "VFX: {what}"),
        }
    }
}

impl std::error::Error for VfxError {}

/// `true` for 1..=[`MAX_VFX_NAME`] ASCII letters, digits, `_` or `-`: an effect name can
/// never hold a separator or dot, so `vfx/<name>.toml` cannot leave the asset root.
#[must_use]
pub fn valid_vfx_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_VFX_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// `<root>/vfx/<name>.toml`, or `None` for an invalid name.
#[must_use]
pub fn vfx_path(root: &Path, name: &str) -> Option<std::path::PathBuf> {
    valid_vfx_name(name).then(|| root.join("vfx").join(format!("{name}.toml")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEffect {
    duration: f32,
    #[serde(default)]
    importance: Option<String>,
    #[serde(default)]
    event_duration: bool,
    #[serde(default)]
    emitters: Vec<RawEmitter>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEmitter {
    #[serde(default)]
    primitive: Option<String>,
    #[serde(default)]
    blend: Option<String>,
    #[serde(default)]
    sprite: Option<String>,
    #[serde(default)]
    mesh: Option<String>,
    #[serde(default)]
    anchor: Option<String>,
    #[serde(default)]
    burst: Option<u32>,
    #[serde(default)]
    rate: Option<f32>,
    #[serde(default)]
    delay: Option<f32>,
    #[serde(default)]
    stop: Option<f32>,
    lifetime: [f32; 2],
    #[serde(default)]
    speed: Option<[f32; 2]>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    spread: Option<f32>,
    #[serde(default)]
    shape: Option<String>,
    #[serde(default)]
    radius: Option<f32>,
    #[serde(default)]
    scale_with_radius: bool,
    #[serde(default)]
    offset: Option<[f32; 3]>,
    size: [f32; 2],
    #[serde(default)]
    size_end: Option<f32>,
    color: [f32; 4],
    #[serde(default)]
    color_end: Option<[f32; 4]>,
    #[serde(default)]
    team_tint: Option<f32>,
    #[serde(default)]
    gravity: Option<f32>,
    #[serde(default)]
    drag: Option<f32>,
    #[serde(default)]
    spin: Option<[f32; 2]>,
    #[serde(default)]
    width: Option<f32>,
    #[serde(default)]
    fps: Option<f32>,
    #[serde(default)]
    telegraph: bool,
}

fn check(value: f32, (min, max): (f32, f32), what: &'static str) -> Result<f32, VfxError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(VfxError::Invalid(what))
    }
}

fn check_pair(pair: [f32; 2], range: (f32, f32), what: &'static str) -> Result<[f32; 2], VfxError> {
    let [low, high] = [check(pair[0], range, what)?, check(pair[1], range, what)?];
    if low > high {
        return Err(VfxError::Invalid(what));
    }
    Ok([low, high])
}

fn check_color(color: [f32; 4], what: &'static str) -> Result<[f32; 4], VfxError> {
    for channel in &color[..3] {
        check(*channel, COLOR_RANGE, what)?;
    }
    check(color[3], (0.0, 1.0), what)?;
    Ok(color)
}

/// Parses one of a fixed set of lower-case keywords.
fn keyword<T: Copy + Default>(
    value: Option<&str>,
    table: &[(&str, T)],
    what: &'static str,
) -> Result<T, VfxError> {
    match value {
        None => Ok(T::default()),
        Some(text) => table
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(text))
            .map(|&(_, value)| value)
            .ok_or(VfxError::Invalid(what)),
    }
}

#[allow(clippy::too_many_lines)] // One validation per field, in schema order.
fn emitter(raw: RawEmitter) -> Result<VfxEmitter, VfxError> {
    let primitive = keyword(
        raw.primitive.as_deref(),
        &[
            ("sprite", VfxPrimitive::Sprite),
            ("flipbook", VfxPrimitive::Flipbook),
            ("trail", VfxPrimitive::Trail),
            ("beam", VfxPrimitive::Beam),
            ("ring", VfxPrimitive::Ring),
            ("mesh", VfxPrimitive::Mesh),
        ],
        "primitive must be sprite, flipbook, trail, beam, ring or mesh",
    )?;
    let blend = keyword(
        raw.blend.as_deref(),
        &[("additive", VfxBlend::Additive), ("alpha", VfxBlend::Alpha)],
        "blend must be additive or alpha",
    )?;
    let default_sprite = match primitive {
        VfxPrimitive::Mesh => "solid",
        VfxPrimitive::Trail | VfxPrimitive::Beam => "beam",
        VfxPrimitive::Ring => "ring",
        VfxPrimitive::Flipbook => "burst",
        VfxPrimitive::Sprite => "soft",
    };
    let sprite = match raw.sprite.as_deref() {
        Some(name) if name.len() <= MAX_VFX_NAME => vfx_sprite(name),
        Some(_) => None,
        None => vfx_sprite(default_sprite),
    }
    .ok_or(VfxError::Invalid("unknown sprite name"))?;
    if primitive == VfxPrimitive::Flipbook && sprite.frames < 2 {
        return Err(VfxError::Invalid(
            "a flipbook needs a multi-frame sprite (burst, smoke, lightning)",
        ));
    }
    let mesh = match (primitive, raw.mesh) {
        (VfxPrimitive::Mesh, Some(mesh)) if crate::valid_mesh_name(&mesh) => Some(mesh),
        (VfxPrimitive::Mesh, Some(_)) => {
            return Err(VfxError::Invalid(
                "mesh names must be 1-32 of [A-Za-z0-9_-]",
            ));
        }
        (VfxPrimitive::Mesh, None) => Some(String::from("cube")),
        (_, Some(_)) => return Err(VfxError::Invalid("only mesh emitters name a mesh")),
        (_, None) => None,
    };
    let anchor = keyword(
        raw.anchor.as_deref(),
        &[
            ("target", VfxAnchor::Target),
            ("source", VfxAnchor::Source),
            ("line", VfxAnchor::Line),
        ],
        "anchor must be target, source or line",
    )?;
    let direction = keyword(
        raw.direction.as_deref(),
        &[
            ("up", VfxDirection::Up),
            ("out", VfxDirection::Out),
            ("random", VfxDirection::Random),
            ("forward", VfxDirection::Forward),
            ("none", VfxDirection::None),
        ],
        "direction must be up, out, random, forward or none",
    )?;
    let shape = keyword(
        raw.shape.as_deref(),
        &[
            ("point", VfxShape::Point),
            ("sphere", VfxShape::Sphere),
            ("disc", VfxShape::Disc),
            ("circle", VfxShape::Circle),
        ],
        "shape must be point, sphere, disc or circle",
    )?;
    let burst = raw.burst.unwrap_or(0);
    if burst > MAX_VFX_BURST {
        return Err(VfxError::Invalid("burst must be at most 512"));
    }
    let rate = check(
        raw.rate.unwrap_or(0.0),
        (0.0, MAX_VFX_RATE),
        "rate must be 0..=1024 per second",
    )?;
    if burst == 0 && rate == 0.0 {
        return Err(VfxError::Invalid("an emitter needs a burst or a rate"));
    }
    let delay = check(
        raw.delay.unwrap_or(0.0),
        DELAY_RANGE,
        "delay must be 0..=30",
    )?;
    let stop = raw
        .stop
        .map(|stop| check(stop, DELAY_RANGE, "stop must be 0..=30"))
        .transpose()?;
    let color = check_color(raw.color, "color must be RGB 0..=32, alpha 0..=1")?;
    Ok(VfxEmitter {
        primitive,
        blend,
        sprite,
        mesh,
        anchor,
        burst,
        rate,
        delay,
        stop,
        lifetime: check_pair(
            raw.lifetime,
            (0.01, MAX_PARTICLE_SECONDS),
            "lifetime must be 0.01..=10 seconds, low <= high",
        )?,
        speed: check_pair(
            raw.speed.unwrap_or([0.0; 2]),
            SPEED_RANGE,
            "speed must be 0..=200, low <= high",
        )?,
        direction,
        spread: check(
            raw.spread.unwrap_or(30.0),
            SPREAD_RANGE,
            "spread must be 0..=180 degrees",
        )?,
        shape,
        radius: check(
            raw.radius.unwrap_or(0.0),
            RADIUS_RANGE,
            "radius must be 0..=50",
        )?,
        scale_with_radius: raw.scale_with_radius,
        offset: {
            let offset = raw.offset.unwrap_or([0.0; 3]);
            for value in offset {
                check(value, OFFSET_RANGE, "offset must be -50..=50")?;
            }
            offset
        },
        size: check_pair(raw.size, SIZE_RANGE, "size must be 0..=100, low <= high")?,
        size_end: check(
            raw.size_end.unwrap_or(1.0),
            SIZE_END_RANGE,
            "size_end must be 0..=16",
        )?,
        color,
        color_end: check_color(
            raw.color_end.unwrap_or([color[0], color[1], color[2], 0.0]),
            "color_end must be RGB 0..=32, alpha 0..=1",
        )?,
        team_tint: check(
            raw.team_tint.unwrap_or(0.0),
            (0.0, 1.0),
            "team_tint must be 0..=1",
        )?,
        gravity: check(
            raw.gravity.unwrap_or(0.0),
            GRAVITY_RANGE,
            "gravity must be -200..=200",
        )?,
        drag: check(raw.drag.unwrap_or(0.0), DRAG_RANGE, "drag must be 0..=50")?,
        spin: check_pair(
            raw.spin.unwrap_or([0.0; 2]),
            SPIN_RANGE,
            "spin must be -3600..=3600 degrees/s, low <= high",
        )?,
        width: check(
            raw.width.unwrap_or(0.25),
            WIDTH_RANGE,
            "width must be 0..=20",
        )?,
        fps: check(raw.fps.unwrap_or(0.0), FPS_RANGE, "fps must be 0..=120")?,
        telegraph: raw.telegraph,
    })
}

fn importance(value: Option<&str>) -> Result<VfxImportance, VfxError> {
    keyword(
        value,
        &[
            ("low", VfxImportance::Low),
            ("normal", VfxImportance::Normal),
            ("high", VfxImportance::High),
        ],
        "importance must be low, normal or high",
    )
}

/// Reads at most `limit` + 1 bytes of a regular file as UTF-8.
fn read_bounded(path: &Path, limit: u64) -> Result<String, VfxError> {
    let file = std::fs::File::open(path).map_err(|error| VfxError::Io(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| VfxError::Io(error.to_string()))?;
    if !metadata.is_file() {
        return Err(VfxError::NotAFile);
    }
    if metadata.len() > limit {
        return Err(VfxError::TooLarge);
    }
    let mut text = String::new();
    file.take(limit + 1)
        .read_to_string(&mut text)
        .map_err(|error| VfxError::Io(error.to_string()))?;
    Ok(text)
}

impl VfxEffect {
    /// Parses and validates an effect descriptor.
    ///
    /// # Errors
    /// [`VfxError::TooLarge`], a TOML error or the first out-of-range value.
    pub fn parse(text: &str) -> Result<Self, VfxError> {
        if text.len() as u64 > MAX_VFX_BYTES {
            return Err(VfxError::TooLarge);
        }
        let raw: RawEffect =
            toml::from_str(text).map_err(|error| VfxError::Toml(error.to_string()))?;
        if raw.emitters.is_empty() || raw.emitters.len() > MAX_VFX_EMITTERS {
            return Err(VfxError::Invalid("an effect needs 1..=8 emitters"));
        }
        let duration = check(
            raw.duration,
            (0.0, MAX_EFFECT_SECONDS),
            "duration must be 0..=30 seconds",
        )?;
        let importance = importance(raw.importance.as_deref())?;
        let mut emitters = Vec::with_capacity(raw.emitters.len());
        for raw in raw.emitters {
            emitters.push(emitter(raw)?);
        }
        Ok(Self {
            duration,
            importance,
            event_duration: raw.event_duration,
            emitters,
        })
    }

    /// Reads and parses a descriptor, reading at most [`MAX_VFX_BYTES`] + 1 bytes.
    ///
    /// # Errors
    /// An I/O, size, TOML or range error.
    pub fn load_file(path: &Path) -> Result<Self, VfxError> {
        Self::parse(&read_bounded(path, MAX_VFX_BYTES)?)
    }

    /// Whether any emitter is an area telegraph.
    #[must_use]
    pub fn has_telegraph(&self) -> bool {
        self.emitters.iter().any(|emitter| emitter.telegraph)
    }

    /// The longest time after the start at which a particle can still be alive, seconds
    /// (`event_seconds` stands in for `duration` when [`Self::event_duration`] is set).
    #[must_use]
    pub fn lifespan(&self, event_seconds: f32) -> f32 {
        let duration = self.emit_seconds(event_seconds);
        self.emitters
            .iter()
            .map(|emitter| {
                let emitting = if emitter.rate > 0.0 {
                    emitter.stop.map_or(duration, |stop| stop.min(duration))
                } else {
                    emitter.delay
                };
                emitting.max(emitter.delay) + emitter.lifetime[1]
            })
            .fold(duration, f32::max)
    }

    /// Seconds the effect emits for an event lasting `event_seconds`.
    #[must_use]
    pub fn emit_seconds(&self, event_seconds: f32) -> f32 {
        if self.event_duration && event_seconds.is_finite() && event_seconds > 0.0 {
            event_seconds.min(MAX_EFFECT_SECONDS)
        } else {
            self.duration
        }
    }

    /// Distinct mesh names the effect's mesh emitters use.
    pub fn mesh_names(&self) -> impl Iterator<Item = &str> {
        let mut seen: Vec<&str> = Vec::new();
        self.emitters
            .iter()
            .filter_map(|emitter| emitter.mesh.as_deref())
            .filter(move |name| {
                if seen.contains(name) {
                    false
                } else {
                    seen.push(name);
                    true
                }
            })
    }
}

/// Effects for one catalogue ability.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct VfxAbilityBinding {
    /// Catalogue ability name (ASCII case-insensitive match).
    pub name: String,
    /// Effect when the ability is cast.
    pub cast: Option<String>,
    /// Caster socket the cast effect's `source` emitters attach to (animated heroes).
    pub cast_socket: Option<String>,
    /// Effect carried by the ability's projectile while it flies.
    pub projectile: Option<String>,
    /// Effect where the projectile hits.
    pub hit: Option<String>,
    /// Effect for the ability's zone, for the zone's lifetime.
    pub zone: Option<String>,
    /// Colour multiplier applied to every effect of this binding.
    pub tint: [f32; 3],
}

/// Effects for unit events and the fallbacks for unbound abilities.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct VfxUnitEffects {
    /// Cast effect for an ability without a `cast` binding.
    pub cast: Option<String>,
    /// Projectile effect for an ability without a `projectile` binding.
    pub projectile: Option<String>,
    /// Hit effect for an ability without a `hit` binding.
    pub hit: Option<String>,
    /// Zone effect for an ability without a `zone` binding.
    pub zone: Option<String>,
    /// A hero dies.
    pub hero_death: Option<String>,
    /// A minion dies.
    pub minion_death: Option<String>,
    /// A tower or nexus is destroyed.
    pub structure_destroyed: Option<String>,
    /// A basic attack lands (at the target).
    pub attack: Option<String>,
}

/// A validated `vfx/bindings.toml`.
///
/// ```toml
/// [defaults]               # every key optional
/// cast = "cast-flash"
/// projectile = "bolt"
/// hit = "impact"
/// zone = "zone"
/// hero_death = "hero-death"
/// minion_death = "puff"
/// structure_destroyed = "collapse"
/// attack = "attack-hit"
///
/// [[abilities]]            # at most 128
/// name = "Arc Bolt"        # the catalogue ability name
/// cast = "arc-cast"
/// cast_socket = "spear_tip"
/// projectile = "arc-bolt"
/// hit = "arc-hit"
/// zone = "cinder-zone"
/// tint = [1.0, 1.0, 1.0]   # 0..=4 per channel
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct VfxBindings {
    /// Unit events and fallbacks.
    pub defaults: VfxUnitEffects,
    /// Ability bindings, in file order (the first of two equal names wins).
    pub abilities: Vec<VfxAbilityBinding>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawDefaults {
    #[serde(default)]
    cast: Option<String>,
    #[serde(default)]
    projectile: Option<String>,
    #[serde(default)]
    hit: Option<String>,
    #[serde(default)]
    zone: Option<String>,
    #[serde(default)]
    hero_death: Option<String>,
    #[serde(default)]
    minion_death: Option<String>,
    #[serde(default)]
    structure_destroyed: Option<String>,
    #[serde(default)]
    attack: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAbility {
    name: String,
    #[serde(default)]
    cast: Option<String>,
    #[serde(default)]
    cast_socket: Option<String>,
    #[serde(default)]
    projectile: Option<String>,
    #[serde(default)]
    hit: Option<String>,
    #[serde(default)]
    zone: Option<String>,
    #[serde(default)]
    tint: Option<[f32; 3]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBindings {
    #[serde(default)]
    defaults: Option<RawDefaults>,
    #[serde(default)]
    abilities: Vec<RawAbility>,
}

fn effect_name(name: Option<String>) -> Result<Option<String>, VfxError> {
    match name {
        Some(name) if !valid_vfx_name(&name) => Err(VfxError::Invalid(
            "effect names must be 1-48 of [A-Za-z0-9_-]",
        )),
        other => Ok(other),
    }
}

impl VfxBindings {
    /// Parses and validates a bindings file.
    ///
    /// # Errors
    /// [`VfxError::TooLarge`], a TOML error or the first invalid name or value.
    pub fn parse(text: &str) -> Result<Self, VfxError> {
        if text.len() as u64 > MAX_VFX_BINDINGS_BYTES {
            return Err(VfxError::TooLarge);
        }
        let raw: RawBindings =
            toml::from_str(text).map_err(|error| VfxError::Toml(error.to_string()))?;
        if raw.abilities.len() > MAX_VFX_ABILITY_BINDINGS {
            return Err(VfxError::Invalid("at most 128 ability bindings"));
        }
        let defaults = raw.defaults.unwrap_or_default();
        let defaults = VfxUnitEffects {
            cast: effect_name(defaults.cast)?,
            projectile: effect_name(defaults.projectile)?,
            hit: effect_name(defaults.hit)?,
            zone: effect_name(defaults.zone)?,
            hero_death: effect_name(defaults.hero_death)?,
            minion_death: effect_name(defaults.minion_death)?,
            structure_destroyed: effect_name(defaults.structure_destroyed)?,
            attack: effect_name(defaults.attack)?,
        };
        let mut abilities = Vec::with_capacity(raw.abilities.len());
        for ability in raw.abilities {
            let name = ability.name.trim();
            if name.is_empty() || name.len() > MAX_VFX_NAME || name.chars().any(char::is_control) {
                return Err(VfxError::Invalid(
                    "ability names must be 1-48 characters without control characters",
                ));
            }
            let socket = match ability.cast_socket {
                Some(socket) if !crate::valid_animation_name(&socket) => {
                    return Err(VfxError::Invalid(
                        "socket names must be 1-32 of [A-Za-z0-9_-]",
                    ));
                }
                other => other,
            };
            let tint = ability.tint.unwrap_or([1.0; 3]);
            for channel in tint {
                check(channel, TINT_RANGE, "tint must be 0..=4 per channel")?;
            }
            abilities.push(VfxAbilityBinding {
                name: String::from(name),
                cast: effect_name(ability.cast)?,
                cast_socket: socket,
                projectile: effect_name(ability.projectile)?,
                hit: effect_name(ability.hit)?,
                zone: effect_name(ability.zone)?,
                tint,
            });
        }
        Ok(Self {
            defaults,
            abilities,
        })
    }

    /// Reads and parses a bindings file, reading at most [`MAX_VFX_BINDINGS_BYTES`] + 1
    /// bytes.
    ///
    /// # Errors
    /// An I/O, size, TOML or validation error.
    pub fn load_file(path: &Path) -> Result<Self, VfxError> {
        Self::parse(&read_bounded(path, MAX_VFX_BINDINGS_BYTES)?)
    }

    /// The binding for the ability called `name` (ASCII case-insensitive).
    #[must_use]
    pub fn ability(&self, name: &str) -> Option<&VfxAbilityBinding> {
        self.abilities
            .iter()
            .find(|binding| binding.name.eq_ignore_ascii_case(name))
    }

    /// Every distinct effect name the bindings reference, in file order.
    pub fn effect_names(&self) -> impl Iterator<Item = &str> {
        let defaults = &self.defaults;
        let all = [
            &defaults.cast,
            &defaults.projectile,
            &defaults.hit,
            &defaults.zone,
            &defaults.hero_death,
            &defaults.minion_death,
            &defaults.structure_destroyed,
            &defaults.attack,
        ]
        .into_iter()
        .filter_map(Option::as_deref)
        .chain(self.abilities.iter().flat_map(|binding| {
            [
                &binding.cast,
                &binding.projectile,
                &binding.hit,
                &binding.zone,
            ]
            .into_iter()
            .filter_map(Option::as_deref)
        }));
        let mut seen: Vec<&str> = Vec::new();
        all.filter(move |name| {
            if seen.contains(name) {
                false
            } else {
                seen.push(name);
                true
            }
        })
    }
}

/// Deterministic integer hash used by the atlas generator.
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_215.0
}

/// Smooth value noise in `0..=1`.
#[allow(clippy::many_single_char_names)] // The four lattice corners.
fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = hash(xi, yi, seed);
    let b = hash(xi + 1, yi, seed);
    let c = hash(xi, yi + 1, seed);
    let d = hash(xi + 1, yi + 1, seed);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sy
}

fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amplitude = 0.5;
    let mut frequency = 1.0;
    for octave in 0..4 {
        sum += amplitude * noise(x * frequency, y * frequency, seed + octave);
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    sum / 0.9375
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Coverage and core intensity of atlas cell `cell` at `(u, v)` in `-1..=1`.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // One table of shapes.
fn cell_texel(cell: u8, u: f32, v: f32) -> (f32, f32) {
    let r = (u * u + v * v).sqrt();
    let angle = v.atan2(u);
    match cell {
        // soft: a gaussian blob.
        0 => {
            let a = (-r * r * 4.0).exp() * smoothstep(1.0, 0.85, r);
            (a, (-r * r * 12.0).exp())
        }
        // dot: a hard disc with a soft rim.
        1 => (smoothstep(0.8, 0.65, r), smoothstep(0.5, 0.0, r)),
        // ring: a thin bright ring.
        2 => {
            let band = (-((r - 0.78) / 0.07).powi(2)).exp();
            (band, band * band)
        }
        // spark: a four-pointed streak.
        3 => {
            let streak = (-(v / 0.06).powi(2)).exp() * smoothstep(1.0, 0.0, u.abs());
            let cross = (-(u / 0.08).powi(2)).exp() * smoothstep(0.6, 0.0, v.abs()) * 0.5;
            let a = (streak + cross).min(1.0);
            (a, (streak * 1.2).min(1.0))
        }
        // star: five soft points.
        4 => {
            let points = (angle * 2.5).cos().abs().powf(8.0);
            let limit = 0.35 + 0.6 * points;
            let a = smoothstep(limit, limit * 0.6, r);
            (a, smoothstep(0.4, 0.0, r))
        }
        // shard: a diamond.
        5 => {
            let d = u.abs() * 1.6 + v.abs();
            (smoothstep(0.95, 0.8, d), smoothstep(0.6, 0.0, d))
        }
        // rune: a ring with tick marks and an inner circle (area telegraph).
        6 => {
            let outer = (-((r - 0.86) / 0.035).powi(2)).exp();
            let inner = (-((r - 0.62) / 0.02).powi(2)).exp() * 0.7;
            let ticks = if (r - 0.74).abs() < 0.08 && (angle * 6.0).cos() > 0.92 {
                0.8
            } else {
                0.0
            };
            let fill = smoothstep(0.9, 0.0, r) * 0.18;
            let a = (outer + inner + ticks + fill).min(1.0);
            (a, outer.max(inner))
        }
        // beam: bright along U, falling off across V (segments sample U along length).
        7 => {
            let core = (-(v / 0.18).powi(2)).exp();
            let glow = (-(v / 0.5).powi(2)).exp() * 0.6;
            ((core + glow).min(1.0), core)
        }
        // burst flipbook: 16 frames of an expanding fiery puff.
        8..=23 => {
            let t = f32::from(cell - 8) / 15.0;
            let radius = 0.5 + 0.45 * t;
            let n = fbm(u * 3.0 + 7.0, v * 3.0 + 3.0, 11 + u32::from(cell));
            let edge = radius * (0.75 + 0.5 * n);
            let shell = smoothstep(edge, edge * 0.55, r);
            let hollow = if t > 0.4 {
                smoothstep(edge * (t - 0.3), edge * t, r)
            } else {
                1.0
            };
            let a = shell * hollow * (1.0 - t * 0.6);
            (a, (1.0 - t) * smoothstep(edge * 0.8, 0.0, r))
        }
        // smoke flipbook: 16 frames of a slowly expanding, thinning puff.
        24..=39 => {
            let t = f32::from(cell - 24) / 15.0;
            let radius = 0.6 + 0.35 * t;
            let n = fbm(u * 2.5 + t * 1.5, v * 2.5 - t, 101);
            let a = smoothstep(radius, radius * 0.3, r * (0.8 + 0.5 * n)) * (1.0 - 0.5 * t);
            (a * (0.6 + 0.4 * n), 0.0)
        }
        // lightning flipbook: 8 frames of jagged bolts across the cell.
        40..=47 => {
            let seed = u32::from(cell) * 7 + 3;
            let wobble = (fbm(u * 4.0, 0.5, seed) - 0.5) * 0.9;
            let branch = (fbm(u * 6.0, 3.5, seed + 1) - 0.5) * 0.6 + 0.3;
            let bolt = (-((v - wobble) / 0.05).powi(2)).exp();
            let fork = (-((v - branch) / 0.035).powi(2)).exp() * smoothstep(-0.2, 0.6, u);
            let a = (bolt + fork * 0.7).min(1.0) * smoothstep(1.0, 0.8, u.abs());
            (a, bolt)
        }
        // noise: a soft noisy blob.
        48 => {
            let n = fbm(u * 3.0 + 1.0, v * 3.0 + 2.0, 48);
            (smoothstep(1.0, 0.2, r) * n, 0.0)
        }
        // swirl: a spiral arm.
        49 => {
            let arm = ((angle * 2.0 + r * 9.0).sin() * 0.5 + 0.5).powf(3.0);
            let a = arm * smoothstep(1.0, 0.3, r) * smoothstep(0.0, 0.2, r);
            (a, arm * smoothstep(0.6, 0.0, r))
        }
        // cross: a plus sign.
        50 => {
            let bar = |d: f32, along: f32| {
                smoothstep(0.16, 0.1, d.abs()) * smoothstep(0.9, 0.75, along.abs())
            };
            let a = bar(u, v).max(bar(v, u));
            (a, a * 0.5)
        }
        // halo: a wide soft ring.
        51 => {
            let band = (-((r - 0.6) / 0.22).powi(2)).exp();
            (band, band * 0.3)
        }
        // solid: full coverage (mesh particles, flat quads).
        VFX_SOLID_CELL => (1.0, 0.0),
        // Unused cells repeat the soft blob.
        _ => cell_texel(0, u, v),
    }
}

/// Generates the built-in particle atlas: 512×512 `RG8` with a full mip chain (box
/// filtered), in the [`VFX_SPRITES`] layout. Deterministic and allocation-bounded (about
/// 700 KiB); called once at renderer creation and by `texture-bake generate vfx`.
#[must_use]
pub fn procedural_atlas() -> TextureData {
    let size = VFX_ATLAS_GRID * VFX_ATLAS_CELL;
    let mut base = vec![0u8; (size * size * 2) as usize];
    for cell in 0..(VFX_ATLAS_GRID * VFX_ATLAS_GRID) {
        let (cx, cy) = (cell % VFX_ATLAS_GRID, cell / VFX_ATLAS_GRID);
        for py in 0..VFX_ATLAS_CELL {
            for px in 0..VFX_ATLAS_CELL {
                // Texel centres, with a one-texel transparent border so bilinear filtering
                // and mips do not bleed between cells.
                let border =
                    px == 0 || py == 0 || px == VFX_ATLAS_CELL - 1 || py == VFX_ATLAS_CELL - 1;
                let (coverage, core) = if border {
                    (0.0, 0.0)
                } else {
                    let u = (px as f32 + 0.5) / VFX_ATLAS_CELL as f32 * 2.0 - 1.0;
                    let v = (py as f32 + 0.5) / VFX_ATLAS_CELL as f32 * 2.0 - 1.0;
                    cell_texel(cell as u8, u, v)
                };
                let x = cx * VFX_ATLAS_CELL + px;
                let y = cy * VFX_ATLAS_CELL + py;
                let index = ((y * size + x) * 2) as usize;
                base[index] = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
                base[index + 1] = (core.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    let levels = full_mip_chain(size, size);
    let mut chain = Vec::with_capacity(levels as usize);
    chain.push(base);
    for level in 1..levels {
        let (width, height) = mip_size(size, size, level);
        let (source_width, _) = mip_size(size, size, level - 1);
        let source = &chain[level as usize - 1];
        let mut next = vec![0u8; (width * height * 2) as usize];
        for y in 0..height {
            for x in 0..width {
                for channel in 0..2 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(source_width - 1);
                        let sy = y * 2 + dy;
                        sum += u32::from(source[((sy * source_width + sx) * 2 + channel) as usize]);
                    }
                    next[((y * width + x) * 2 + channel) as usize] = ((sum + 2) / 4) as u8;
                }
            }
        }
        chain.push(next);
    }
    TextureData::new(TextureFormat::Rg8Unorm, size, size, &chain)
        .expect("the atlas generator builds a valid mip chain")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOLT: &str = r#"
duration = 0.8
importance = "high"

[[emitters]]
primitive = "trail"
lifetime = [0.2, 0.3]
size = [0.3, 0.3]
color = [0.4, 0.7, 4.0, 1.0]
rate = 60
width = 0.4

[[emitters]]
primitive = "flipbook"
sprite = "burst"
blend = "alpha"
burst = 12
lifetime = [0.4, 0.6]
speed = [1, 3]
shape = "sphere"
radius = 0.3
size = [0.5, 1.0]
size_end = 2.0
color = [1, 0.5, 0.2, 0.8]
color_end = [0.2, 0.1, 0.1, 0]
gravity = 1.5
drag = 2
spin = [-90, 90]
fps = 24

[[emitters]]
primitive = "mesh"
mesh = "crystal"
burst = 4
lifetime = [1, 1]
size = [0.2, 0.4]
color = [0.3, 0.8, 1.0, 1.0]

[[emitters]]
primitive = "ring"
burst = 1
lifetime = [1, 1]
size = [2, 2]
color = [1, 0.2, 0.2, 0.6]
telegraph = true
scale_with_radius = true
"#;

    #[test]
    fn parses_every_primitive_with_defaults() {
        let effect = VfxEffect::parse(BOLT).expect("effect");
        assert_eq!(effect.importance, VfxImportance::High);
        assert_eq!(effect.emitters.len(), 4);
        let trail = &effect.emitters[0];
        assert_eq!(trail.primitive, VfxPrimitive::Trail);
        assert_eq!(trail.sprite.name, "beam");
        assert_eq!(trail.blend, VfxBlend::Additive);
        assert_eq!(trail.color_end, [0.4, 0.7, 4.0, 0.0]);
        let flipbook = &effect.emitters[1];
        assert_eq!(flipbook.sprite.frames, 16);
        assert_eq!(flipbook.shape, VfxShape::Sphere);
        assert_eq!(effect.emitters[2].mesh.as_deref(), Some("crystal"));
        assert_eq!(effect.emitters[2].sprite.cell, VFX_SOLID_CELL);
        assert!(effect.has_telegraph());
        assert_eq!(effect.emitters[3].sprite.name, "ring");
        assert_eq!(effect.mesh_names().collect::<Vec<_>>(), ["crystal"]);
        // The trail emits for the whole 0.8 s, then lives 0.3 s.
        assert!((effect.lifespan(0.0) - 1.1).abs() < 1e-6);
        let zone = VfxEffect::parse(
            "duration = 1\nevent_duration = true\n[[emitters]]\nrate = 5\nlifetime = [1, 2]\n\
             size = [1, 1]\ncolor = [1, 1, 1, 1]",
        )
        .expect("zone");
        assert_eq!(zone.emit_seconds(4.0), 4.0);
        assert_eq!(zone.emit_seconds(0.0), 1.0);
        assert!((zone.lifespan(4.0) - 6.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_hostile_or_out_of_range_effects() {
        let emitter = |body: &str| {
            format!(
                "duration = 1\n[[emitters]]\nburst = 1\nlifetime = [0.5, 1]\nsize = [1, 1]\n\
                 color = [1, 1, 1, 1]\n{body}"
            )
        };
        for bad in [
            String::new(),
            String::from("duration = 1"),
            String::from(
                "duration = 31\n[[emitters]]\nburst = 1\nlifetime = [1,1]\nsize = [1,1]\ncolor = [1,1,1,1]",
            ),
            emitter("extra = 1"),
            emitter("primitive = \"laser\""),
            emitter("sprite = \"missing\""),
            emitter("primitive = \"flipbook\"\nsprite = \"soft\""),
            emitter("mesh = \"crystal\""),
            emitter("primitive = \"mesh\"\nmesh = \"../x\""),
            emitter("burst = 513"),
            emitter("rate = 1e9"),
            emitter("speed = [5, 1]"),
            emitter("speed = [nan, 1]"),
            emitter("gravity = inf"),
            emitter("color_end = [1, 1, 1, 2]"),
            emitter("offset = [0, 99, 0]"),
            emitter("direction = \"sideways\""),
            emitter("blend = \"multiply\""),
            emitter("anchor = \"sky\""),
            emitter("spin = [10, -10]"),
            String::from(
                "duration = 1\n[[emitters]]\nlifetime = [1, 1]\nsize = [1, 1]\ncolor = [1, 1, 1, 1]",
            ),
            String::from(
                "duration = 1\n[[emitters]]\nburst = 1\nlifetime = [0, 1]\nsize = [1, 1]\ncolor = [1, 1, 1, 1]",
            ),
            format!(
                "duration = 1\n{}",
                "[[emitters]]\nburst = 1\nlifetime = [1, 1]\nsize = [1, 1]\ncolor = [1, 1, 1, 1]\n"
                    .repeat(9)
            ),
            format!("{}#{}", emitter(""), "x".repeat(17 * 1024)),
        ] {
            assert!(VfxEffect::parse(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn bindings_resolve_abilities_and_list_effects() {
        let bindings = VfxBindings::parse(
            r#"
[defaults]
cast = "flash"
hero_death = "death"
attack = "spark"

[[abilities]]
name = "Arc Bolt"
cast = "flash"
cast_socket = "spear_tip"
projectile = "arc-bolt"
hit = "arc-hit"
tint = [0.5, 1, 2]

[[abilities]]
name = "Cinder Zone"
zone = "cinder"
"#,
        )
        .expect("bindings");
        let arc = bindings.ability("arc bolt").expect("case-insensitive");
        assert_eq!(arc.projectile.as_deref(), Some("arc-bolt"));
        assert_eq!(arc.cast_socket.as_deref(), Some("spear_tip"));
        assert_eq!(arc.tint, [0.5, 1.0, 2.0]);
        assert!(bindings.ability("Nova").is_none());
        assert_eq!(
            bindings.effect_names().collect::<Vec<_>>(),
            ["flash", "death", "spark", "arc-bolt", "arc-hit", "cinder"]
        );
        for bad in [
            "[defaults]\ncast = \"../escape\"",
            "[defaults]\nunknown = \"x\"",
            "[[abilities]]\nname = \"\"",
            "[[abilities]]\nname = \"A\"\ncast_socket = \"a b\"",
            "[[abilities]]\nname = \"A\"\ntint = [5, 1, 1]",
            "[[abilities]]\nname = \"A\"\nbogus = 1",
        ] {
            assert!(VfxBindings::parse(bad).is_err(), "{bad:?}");
        }
        let many = "[[abilities]]\nname = \"A\"\n".repeat(129);
        assert!(VfxBindings::parse(&many).is_err());
        assert_eq!(
            VfxBindings::parse("").expect("empty"),
            VfxBindings::default()
        );
    }

    #[test]
    fn atlas_is_deterministic_with_a_full_chain_and_clean_borders() {
        let atlas = procedural_atlas();
        assert_eq!(atlas.format(), TextureFormat::Rg8Unorm);
        assert_eq!((atlas.width(), atlas.height()), (512, 512));
        assert_eq!(atlas.level_count(), 10);
        assert_eq!(atlas, procedural_atlas());
        let (_, _, base) = atlas.level(0).expect("level 0");
        // Cell borders are transparent; the soft blob's centre is opaque.
        assert_eq!(base[0], 0);
        let centre = ((32 * 512 + 32) * 2) as usize;
        assert!(base[centre] > 200, "{}", base[centre]);
        // The solid cell is fully covered inside its border.
        let (sx, sy) = (
            u32::from(VFX_SOLID_CELL % 8) * 64 + 10,
            u32::from(VFX_SOLID_CELL / 8) * 64 + 10,
        );
        assert_eq!(base[((sy * 512 + sx) * 2) as usize], 255);
        for sprite in VFX_SPRITES {
            assert!(u32::from(sprite.cell) + u32::from(sprite.frames) <= 64);
            assert_eq!(vfx_sprite(sprite.name), Some(*sprite));
        }
    }

    #[test]
    fn names_stay_inside_the_vfx_directory() {
        let root = Path::new("assets");
        assert_eq!(
            vfx_path(root, "arc-bolt"),
            Some(root.join("vfx").join("arc-bolt.toml"))
        );
        for bad in ["", "../x", "a.b", "a/b", "c:x", &"x".repeat(49)] {
            assert!(vfx_path(root, bad).is_none(), "{bad:?}");
        }
    }
}
