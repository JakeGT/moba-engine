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
//!                          # | distortion (heat haze, below; High and up)
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
//! A **distortion** emitter (heat haze, ADR-0019) writes a screen-space offset instead of
//! colour: camera-facing sprites that bend the scene behind them on High and Ultra. It takes
//! no `color`, `color_end`, `team_tint`, `width`, `fps` or `telegraph`, and only the
//! `sprite` primitive:
//!
//! ```toml
//! [[emitters]]
//! blend = "distortion"
//! distortion = 0.6         # strength at birth, a fraction of the tier's pixel cap, 0.05..=1
//! distortion_end = 0.0     # strength at the end of life, 0..=1 (default 0)
//! distortion_scale = 2.0   # noise cells across the sprite, 0.5..=8 (default 2)
//! distortion_speed = 1.0   # rise speed, sprite heights per second, 0..=8 (default 1)
//! fallback_color = [1.0, 0.4, 0.1, 0.1]  # optional faint additive sprite (`sprite`) where
//!                          # distortion is off (Low, Medium): RGB 0..=4, alpha 0..=0.5;
//!                          # without it the emitter is skipped there
//! ```
//!
//! An effect may also project up to [`MAX_VFX_DECALS`] ground decals (ADR-0017): telegraph
//! markers, zones, scorch marks. An effect needs at least one emitter or one decal.
//!
//! ```toml
//! [[decals]]
//! shape = "ring"           # disc | ring | cone | line | splat
//! style = "glow"           # glow (after lighting, never dimmed) | paint (into the albedo);
//!                          # default glow, paint for splats
//! anchor = "target"        # target | source | line (from the source to the target)
//! radius = 3.0             # metres, 0.1..=50 (a line's length when not anchored on a line)
//! width = 0.4              # ring / line band width, metres, 0.05..=20
//! angle = 60.0             # cone angle, degrees, 1..=360
//! delay = 0.0              # seconds after the effect starts, 0..=30
//! lifetime = 1.2           # seconds, 0.05..=30 (default: the effect's emit window)
//! fade_in = 0.1            # seconds, 0..=10
//! fade_out = 0.25          # seconds, 0..=10
//! color = [1.0, 0.3, 0.2, 0.8]   # linear RGBA; RGB may exceed 1 (HDR, up to 32)
//! team_tint = 0.0          # 0..=1
//! scale_with_radius = true # radius and width times the event's radius (zones, areas)
//! telegraph = true         # an area telegraph (glow only; never stamped)
//! progress = true          # an inner fill grows to the edge over the decal's life
//! stamp = false            # copied into the persistent ring at `delay` (needs `lifetime`)
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
/// Brightest effect light, a multiplier on its linear colour (ADR-0016).
pub const MAX_VFX_LIGHT_INTENSITY: f32 = 64.0;
/// Largest effect-light radius, metres. The renderer's light grid has 32 × 32 columns of
/// about 4.6 m at the default 46 m camera, so a 24 m light is binned into about 11 × 11
/// columns (a typical 4–6 m effect light into 2–3 × 2–3); keep radii as small as the look
/// allows.
pub const MAX_VFX_LIGHT_RADIUS: f32 = 24.0;
const LIGHT_RADIUS_RANGE: (f32, f32) = (0.25, MAX_VFX_LIGHT_RADIUS);
const LIGHT_COLOR_RANGE: (f32, f32) = (0.0, 4.0);
const LIGHT_FADE_RANGE: (f32, f32) = (0.0, 10.0);
const DECAL_RADIUS_RANGE: (f32, f32) = (0.1, 50.0);
const DECAL_WIDTH_RANGE: (f32, f32) = (0.05, 20.0);
const DECAL_ANGLE_RANGE: (f32, f32) = (1.0, 360.0);
const DECAL_LIFETIME_RANGE: (f32, f32) = (0.05, MAX_DECAL_SECONDS);
const DECAL_FADE_RANGE: (f32, f32) = (0.0, 10.0);
/// Most decals one effect may project.
pub const MAX_VFX_DECALS: usize = 4;
/// Longest decal lifetime, seconds (also the longest a stamped decal persists).
pub const MAX_DECAL_SECONDS: f32 = 30.0;
/// Decal radius when a `[[decals]]` entry gives none, metres.
pub const DEFAULT_DECAL_RADIUS: f32 = 2.0;
/// Ring / line band width when a `[[decals]]` entry gives none, metres.
pub const DEFAULT_DECAL_WIDTH: f32 = 0.5;
/// Cone angle when a `[[decals]]` entry gives none, degrees.
pub const DEFAULT_DECAL_ANGLE: f32 = 90.0;
/// Fade-in when a `[[decals]]` entry gives none, seconds.
pub const DEFAULT_DECAL_FADE_IN: f32 = 0.1;
/// Fade-out when a `[[decals]]` entry gives none, seconds.
pub const DEFAULT_DECAL_FADE_OUT: f32 = 0.25;
/// How long past a stamped decal's delay its effect lives, seconds
/// ([`VfxEffect::lifespan`]): longer than the client's largest VFX frame step (0.1 s), so
/// some frame always lands between the delay and the effect's end and copies the stamp.
pub const STAMP_LATCH_SECONDS: f32 = 0.125;
const _: () = assert!(
    STAMP_LATCH_SECONDS > 0.1,
    "longer than the client's largest VFX step"
);

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
    /// Heat haze (ADR-0019): refracts the scene behind the sprite by a capped screen-space
    /// offset instead of adding colour. High and Ultra only; see [`VfxDistortion`].
    Distortion,
}

/// The authored parameters of a [`VfxBlend::Distortion`] emitter (ADR-0019).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct VfxDistortion {
    /// Strength at birth, a fraction of the tier's pixel cap, `0.05..=1`.
    pub strength: f32,
    /// Strength at the end of life, `0..=1`.
    pub strength_end: f32,
    /// Noise cells across the sprite, `0.5..=8`.
    pub scale: f32,
    /// Noise rise speed, sprite heights per second, `0..=8`.
    pub speed: f32,
    /// The faint additive sprite drawn instead where distortion is off (Low, Medium, or
    /// switched off), linear RGBA (RGB `0..=4`, alpha `0..=0.5`); `None` skips the emitter
    /// there.
    pub fallback_color: Option<[f32; 4]>,
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
    /// The heat-haze parameters; `Some` exactly when `blend` is [`VfxBlend::Distortion`]
    /// (whose `color` and `color_end` are then unused zeros).
    pub distortion: Option<VfxDistortion>,
}

/// The footprint of a ground decal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum VfxDecalShape {
    /// A filled disc of `radius`.
    #[default]
    Disc,
    /// A band of `width` just inside `radius`.
    Ring,
    /// A sector of `radius` and `angle`, opening along the source → target direction.
    Cone,
    /// A band of `width`: from the source to the target when anchored on a line, else
    /// `radius` long along the source → target direction.
    Line,
    /// An irregular splat of about `radius` (its outline comes from a per-instance seed).
    Splat,
}

/// How a ground decal is composited (ADR-0017 §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum VfxDecalStyle {
    /// Composited after shading, fog of war and height fog: nothing dims it (telegraphs,
    /// zones).
    Glow,
    /// Blended into the surface albedo before lighting: shadowed and fogged like the ground
    /// (scorch, goo).
    Paint,
}

/// One validated ground decal of an effect (`[[decals]]`).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)] // Independent descriptor flags, as in the TOML.
pub struct VfxDecal {
    /// Footprint.
    pub shape: VfxDecalShape,
    /// Compositing style.
    pub style: VfxDecalStyle,
    /// Where it is placed.
    pub anchor: VfxAnchor,
    /// Radius (a line's length unless anchored on a line), metres.
    pub radius: f32,
    /// Ring / line band width, metres.
    pub width: f32,
    /// Cone angle, degrees.
    pub angle: f32,
    /// Seconds after the effect starts before it appears.
    pub delay: f32,
    /// Seconds it lasts once it appears; `None`: the effect's emit window.
    pub lifetime: Option<f32>,
    /// Fade-in, seconds.
    pub fade_in: f32,
    /// Fade-out, seconds.
    pub fade_out: f32,
    /// Linear RGBA (RGB up to 32, alpha 0..=1).
    pub color: [f32; 4],
    /// How much the colour is pulled towards the team colour, `0..=1`.
    pub team_tint: f32,
    /// Scale `radius` and `width` by the event's radius (zones, areas).
    pub scale_with_radius: bool,
    /// An area telegraph: enemy instances keep a minimum opacity and the contrast rim and are
    /// never culled by budgets. Always [`VfxDecalStyle::Glow`], never stamped.
    pub telegraph: bool,
    /// An inner fill grows from the centre to the edge over the decal's life (a cast timer).
    pub progress: bool,
    /// Copied into the persistent stamp ring at `delay`, outliving the effect for `lifetime`.
    pub stamp: bool,
}

impl VfxDecal {
    /// Seconds the decal lasts once it appears, for an effect emitting `emit_seconds`
    /// ([`VfxEffect::emit_seconds`]).
    #[must_use]
    pub fn seconds(&self, emit_seconds: f32) -> f32 {
        self.lifetime.unwrap_or(emit_seconds)
    }
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
    /// Emitters, in file order (at most [`MAX_VFX_EMITTERS`]).
    pub emitters: Vec<VfxEmitter>,
    /// Ground decals, in file order (at most [`MAX_VFX_DECALS`]).
    pub decals: Vec<VfxDecal>,
    /// The optional dynamic point light the effect casts (ADR-0016).
    pub light: Option<VfxLight>,
}

/// A validated `[light]` block: the dynamic point light an effect casts while it runs
/// (ADR-0016). Client-local presentation like the rest of the descriptor: it is attached to
/// an effect instance the client already shows, so it can never light (or reveal) a unit
/// the client does not have.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct VfxLight {
    /// Linear RGB, `0..=4` per channel.
    pub color: [f32; 3],
    /// Multiplier on `color`, `0..=64`.
    pub intensity: f32,
    /// Distance at which the light reaches zero, metres (`0.25..=24`).
    pub radius: f32,
    /// Where it sits: the event's target position or its source unit (`line` is not
    /// allowed).
    pub anchor: VfxAnchor,
    /// Offset from the anchor, metres (`y` is up; default one metre up).
    pub offset: [f32; 3],
    /// Seconds after the effect starts before the light switches on.
    pub delay: f32,
    /// Seconds the light lasts after `delay` (`None`: until the effect stops emitting).
    pub duration: Option<f32>,
    /// Seconds to ramp up from zero after `delay`.
    pub fade_in: f32,
    /// Seconds to ramp down to zero after the light ends (default 0.2).
    pub fade_out: f32,
    /// Random intensity flicker, `0..=1` (fraction of `intensity`).
    pub flicker: f32,
    /// How much the colour is pulled towards the team colour, `0..=1`.
    pub team_tint: f32,
    /// Scale `radius` by the event's radius (zones, areas).
    pub scale_with_radius: bool,
}

impl VfxLight {
    fn on_seconds(&self, emit_seconds: f32) -> f32 {
        self.duration
            .unwrap_or_else(|| (emit_seconds - self.delay).max(0.0))
    }

    /// The seconds after the effect starts at which the light has fully faded out, for an
    /// effect emitting `emit_seconds`.
    #[must_use]
    pub fn end(&self, emit_seconds: f32) -> f32 {
        self.delay + self.on_seconds(emit_seconds) + self.fade_out
    }

    /// The fade envelope at `age` seconds into an effect emitting `emit_seconds`, `0..=1`
    /// (flicker excluded).
    #[must_use]
    pub fn envelope(&self, age: f32, emit_seconds: f32) -> f32 {
        if !age.is_finite() || age < self.delay {
            return 0.0;
        }
        let local = age - self.delay;
        let on = self.on_seconds(emit_seconds);
        let rise = if self.fade_in > 0.0 {
            (local / self.fade_in).min(1.0)
        } else {
            1.0
        };
        let fall = if local <= on {
            1.0
        } else if self.fade_out > 0.0 {
            1.0 - ((local - on) / self.fade_out).min(1.0)
        } else {
            0.0
        };
        (rise * fall).clamp(0.0, 1.0)
    }
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

/// What [`check_vfx_dir`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct VfxDirReport {
    /// Effect files that loaded.
    pub effects: usize,
    /// Effect or bindings files that do not load, and effects referenced but missing.
    pub errors: Vec<String>,
}

/// Validates a content root's client-local effects (`<root>/vfx`, `content-check`): every
/// `vfx/<name>.toml` must load ([`VfxEffect::load_file`], so every field of every emitter,
/// heat haze included, is checked), `vfx/bindings.toml` must load when present and every
/// effect it names must exist, as must each of `referenced` (`(who, effect)` pairs, such as
/// a map's `[[effects]]`). A missing `vfx` directory is fine (no effects) unless something
/// references an effect.
#[must_use]
pub fn check_vfx_dir(root: &Path, referenced: &[(&str, &str)]) -> VfxDirReport {
    let mut report = VfxDirReport::default();
    let dir = root.join("vfx");
    let mut known: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut paths: Vec<std::path::PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "toml")
            })
            .collect();
        paths.sort();
        for path in paths {
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if stem == "bindings" {
                continue;
            }
            if !valid_vfx_name(stem) {
                report.errors.push(format!(
                    "vfx/{stem}.toml: not a valid effect name (1-{MAX_VFX_NAME} of A-Z a-z 0-9 _ -)"
                ));
                continue;
            }
            match VfxEffect::load_file(&path) {
                Ok(_) => {
                    report.effects += 1;
                    known.push(String::from(stem));
                }
                Err(error) => report.errors.push(format!("vfx/{stem}.toml: {error}")),
            }
        }
    }
    let exists = |name: &str| known.iter().any(|known| known == name);
    let bindings = dir.join("bindings.toml");
    if bindings.exists() {
        match VfxBindings::load_file(&bindings) {
            Ok(bindings) => {
                for name in bindings.effect_names() {
                    if !exists(name) {
                        report.errors.push(format!(
                            "vfx/bindings.toml names effect '{name}', but vfx/{name}.toml does \
                             not exist or does not load"
                        ));
                    }
                }
            }
            Err(error) => report.errors.push(format!("vfx/bindings.toml: {error}")),
        }
    }
    for (who, name) in referenced {
        if !exists(name) {
            report.errors.push(format!(
                "{who} names effect '{name}', but vfx/{name}.toml does not exist or does not load"
            ));
        }
    }
    report
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
    #[serde(default)]
    light: Option<RawLight>,
    #[serde(default)]
    decals: Vec<RawDecal>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLight {
    color: [f32; 3],
    #[serde(default)]
    intensity: Option<f32>,
    radius: f32,
    #[serde(default)]
    anchor: Option<String>,
    #[serde(default)]
    offset: Option<[f32; 3]>,
    #[serde(default)]
    delay: Option<f32>,
    #[serde(default)]
    duration: Option<f32>,
    #[serde(default)]
    fade_in: Option<f32>,
    #[serde(default)]
    fade_out: Option<f32>,
    #[serde(default)]
    flicker: Option<f32>,
    #[serde(default)]
    team_tint: Option<f32>,
    #[serde(default)]
    scale_with_radius: bool,
}

fn light(raw: &RawLight) -> Result<VfxLight, VfxError> {
    for channel in raw.color {
        check(channel, LIGHT_COLOR_RANGE, "light color must be RGB 0..=4")?;
    }
    let anchor = keyword(
        raw.anchor.as_deref(),
        &[("target", VfxAnchor::Target), ("source", VfxAnchor::Source)],
        "light anchor must be target or source",
    )?;
    let offset = raw.offset.unwrap_or([0.0, 1.0, 0.0]);
    for value in offset {
        check(value, OFFSET_RANGE, "light offset must be -50..=50")?;
    }
    Ok(VfxLight {
        color: raw.color,
        intensity: check(
            raw.intensity.unwrap_or(1.0),
            (0.0, MAX_VFX_LIGHT_INTENSITY),
            "light intensity must be 0..=64",
        )?,
        radius: check(
            raw.radius,
            LIGHT_RADIUS_RANGE,
            "light radius must be 0.25..=24 metres",
        )?,
        anchor,
        offset,
        delay: check(
            raw.delay.unwrap_or(0.0),
            DELAY_RANGE,
            "light delay must be 0..=30",
        )?,
        duration: raw
            .duration
            .map(|duration| check(duration, DELAY_RANGE, "light duration must be 0..=30"))
            .transpose()?,
        fade_in: check(
            raw.fade_in.unwrap_or(0.0),
            LIGHT_FADE_RANGE,
            "light fade_in must be 0..=10",
        )?,
        fade_out: check(
            raw.fade_out.unwrap_or(0.2),
            LIGHT_FADE_RANGE,
            "light fade_out must be 0..=10",
        )?,
        flicker: check(
            raw.flicker.unwrap_or(0.0),
            (0.0, 1.0),
            "light flicker must be 0..=1",
        )?,
        team_tint: check(
            raw.team_tint.unwrap_or(0.0),
            (0.0, 1.0),
            "light team_tint must be 0..=1",
        )?,
        scale_with_radius: raw.scale_with_radius,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Mirrors the TOML schema.
struct RawDecal {
    #[serde(default)]
    shape: Option<String>,
    #[serde(default)]
    style: Option<String>,
    #[serde(default)]
    anchor: Option<String>,
    #[serde(default)]
    radius: Option<f32>,
    #[serde(default)]
    width: Option<f32>,
    #[serde(default)]
    angle: Option<f32>,
    #[serde(default)]
    delay: Option<f32>,
    #[serde(default)]
    lifetime: Option<f32>,
    #[serde(default)]
    fade_in: Option<f32>,
    #[serde(default)]
    fade_out: Option<f32>,
    color: [f32; 4],
    #[serde(default)]
    team_tint: Option<f32>,
    #[serde(default)]
    scale_with_radius: bool,
    #[serde(default)]
    telegraph: bool,
    #[serde(default)]
    progress: bool,
    #[serde(default)]
    stamp: bool,
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
    #[serde(default)]
    color: Option<[f32; 4]>,
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
    #[serde(default)]
    distortion: Option<f32>,
    #[serde(default)]
    distortion_end: Option<f32>,
    #[serde(default)]
    distortion_scale: Option<f32>,
    #[serde(default)]
    distortion_speed: Option<f32>,
    #[serde(default)]
    fallback_color: Option<[f32; 4]>,
}

impl RawEmitter {
    /// Whether any heat-haze key is present.
    const fn has_distortion_keys(&self) -> bool {
        self.distortion.is_some()
            || self.distortion_end.is_some()
            || self.distortion_scale.is_some()
            || self.distortion_speed.is_some()
            || self.fallback_color.is_some()
    }
}

/// Validates a distortion emitter's heat-haze keys and the keys it may not have (ADR-0019).
fn distortion(raw: &RawEmitter, primitive: VfxPrimitive) -> Result<VfxDistortion, VfxError> {
    if primitive != VfxPrimitive::Sprite {
        return Err(VfxError::Invalid(
            "a distortion emitter must use the sprite primitive",
        ));
    }
    if raw.color.is_some() || raw.color_end.is_some() || raw.team_tint.is_some() {
        return Err(VfxError::Invalid(
            "a distortion emitter has no color, color_end or team_tint (use fallback_color)",
        ));
    }
    if raw.width.is_some() || raw.fps.is_some() {
        return Err(VfxError::Invalid(
            "a distortion emitter has no width or fps",
        ));
    }
    if raw.telegraph {
        return Err(VfxError::Invalid(
            "a distortion emitter cannot be a telegraph: telegraphs must stay crisp",
        ));
    }
    let strength = raw.distortion.ok_or(VfxError::Invalid(
        "a distortion emitter needs a distortion strength",
    ))?;
    let fallback_color = match raw.fallback_color {
        Some(color) => {
            for channel in &color[..3] {
                check(
                    *channel,
                    (0.0, 4.0),
                    "fallback_color must be RGB 0..=4, alpha 0..=0.5",
                )?;
            }
            check(
                color[3],
                (0.0, 0.5),
                "fallback_color must be RGB 0..=4, alpha 0..=0.5",
            )?;
            Some(color)
        }
        None => None,
    };
    Ok(VfxDistortion {
        strength: check(strength, (0.05, 1.0), "distortion must be 0.05..=1")?,
        strength_end: check(
            raw.distortion_end.unwrap_or(0.0),
            (0.0, 1.0),
            "distortion_end must be 0..=1",
        )?,
        scale: check(
            raw.distortion_scale.unwrap_or(2.0),
            (0.5, 8.0),
            "distortion_scale must be 0.5..=8",
        )?,
        speed: check(
            raw.distortion_speed.unwrap_or(1.0),
            (0.0, 8.0),
            "distortion_speed must be 0..=8",
        )?,
        fallback_color,
    })
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
        &[
            ("additive", VfxBlend::Additive),
            ("alpha", VfxBlend::Alpha),
            ("distortion", VfxBlend::Distortion),
        ],
        "blend must be additive, alpha or distortion",
    )?;
    let distortion = if blend == VfxBlend::Distortion {
        Some(distortion(&raw, primitive)?)
    } else if raw.has_distortion_keys() {
        return Err(VfxError::Invalid(
            "distortion keys need blend = \"distortion\"",
        ));
    } else {
        None
    };
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
    let anchor = anchor(raw.anchor.as_deref())?;
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
    let color = match (raw.color, distortion) {
        (Some(color), None) => check_color(color, "color must be RGB 0..=32, alpha 0..=1")?,
        (None, None) => return Err(VfxError::Invalid("an emitter needs a color")),
        // A distortion emitter writes an offset, not colour (rejected above if given).
        (_, Some(_)) => [0.0; 4],
    };
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
        distortion,
    })
}

fn anchor(value: Option<&str>) -> Result<VfxAnchor, VfxError> {
    keyword(
        value,
        &[
            ("target", VfxAnchor::Target),
            ("source", VfxAnchor::Source),
            ("line", VfxAnchor::Line),
        ],
        "anchor must be target, source or line",
    )
}

fn decal(raw: &RawDecal) -> Result<VfxDecal, VfxError> {
    let shape = keyword(
        raw.shape.as_deref(),
        &[
            ("disc", VfxDecalShape::Disc),
            ("ring", VfxDecalShape::Ring),
            ("cone", VfxDecalShape::Cone),
            ("line", VfxDecalShape::Line),
            ("splat", VfxDecalShape::Splat),
        ],
        "decal shape must be disc, ring, cone, line or splat",
    )?;
    let style = match raw.style.as_deref() {
        None if shape == VfxDecalShape::Splat => VfxDecalStyle::Paint,
        None => VfxDecalStyle::Glow,
        Some(text) if text.eq_ignore_ascii_case("glow") => VfxDecalStyle::Glow,
        Some(text) if text.eq_ignore_ascii_case("paint") => VfxDecalStyle::Paint,
        Some(_) => return Err(VfxError::Invalid("decal style must be glow or paint")),
    };
    if raw.stamp && raw.telegraph {
        return Err(VfxError::Invalid(
            "a telegraph decal cannot be stamped: it must end with its effect",
        ));
    }
    if raw.stamp && raw.progress {
        return Err(VfxError::Invalid("a stamped decal cannot show progress"));
    }
    if raw.stamp && raw.lifetime.is_none() {
        return Err(VfxError::Invalid("a stamped decal needs a lifetime"));
    }
    if raw.telegraph && style == VfxDecalStyle::Paint {
        return Err(VfxError::Invalid(
            "a telegraph decal must use the glow style: paint is shadowed and fogged",
        ));
    }
    Ok(VfxDecal {
        shape,
        style,
        anchor: anchor(raw.anchor.as_deref())?,
        radius: check(
            raw.radius.unwrap_or(DEFAULT_DECAL_RADIUS),
            DECAL_RADIUS_RANGE,
            "decal radius must be 0.1..=50 metres",
        )?,
        width: check(
            raw.width.unwrap_or(DEFAULT_DECAL_WIDTH),
            DECAL_WIDTH_RANGE,
            "decal width must be 0.05..=20 metres",
        )?,
        angle: check(
            raw.angle.unwrap_or(DEFAULT_DECAL_ANGLE),
            DECAL_ANGLE_RANGE,
            "decal angle must be 1..=360 degrees",
        )?,
        delay: check(
            raw.delay.unwrap_or(0.0),
            DELAY_RANGE,
            "decal delay must be 0..=30 seconds",
        )?,
        lifetime: raw
            .lifetime
            .map(|seconds| {
                check(
                    seconds,
                    DECAL_LIFETIME_RANGE,
                    "decal lifetime must be 0.05..=30 seconds",
                )
            })
            .transpose()?,
        fade_in: check(
            raw.fade_in.unwrap_or(DEFAULT_DECAL_FADE_IN),
            DECAL_FADE_RANGE,
            "decal fade_in must be 0..=10 seconds",
        )?,
        fade_out: check(
            raw.fade_out.unwrap_or(DEFAULT_DECAL_FADE_OUT),
            DECAL_FADE_RANGE,
            "decal fade_out must be 0..=10 seconds",
        )?,
        color: check_color(
            raw.color,
            "decal color must be RGB in 0..=32 with alpha in 0..=1",
        )?,
        team_tint: check(
            raw.team_tint.unwrap_or(0.0),
            (0.0, 1.0),
            "decal team_tint must be 0..=1",
        )?,
        scale_with_radius: raw.scale_with_radius,
        telegraph: raw.telegraph,
        progress: raw.progress,
        stamp: raw.stamp,
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
        if raw.emitters.len() > MAX_VFX_EMITTERS {
            return Err(VfxError::Invalid("an effect has at most 8 emitters"));
        }
        if raw.decals.len() > MAX_VFX_DECALS {
            return Err(VfxError::Invalid("an effect has at most 4 decals"));
        }
        if raw.emitters.is_empty() && raw.decals.is_empty() {
            return Err(VfxError::Invalid(
                "an effect needs at least one emitter or decal",
            ));
        }
        let duration = check(
            raw.duration,
            (0.0, MAX_EFFECT_SECONDS),
            "duration must be 0..=30 seconds",
        )?;
        let importance = importance(raw.importance.as_deref())?;
        let light = raw.light.as_ref().map(light).transpose()?;
        let mut emitters = Vec::with_capacity(raw.emitters.len());
        for raw in raw.emitters {
            emitters.push(emitter(raw)?);
        }
        let mut decals = Vec::with_capacity(raw.decals.len());
        for raw in raw.decals {
            decals.push(decal(&raw)?);
        }
        Ok(Self {
            duration,
            importance,
            event_duration: raw.event_duration,
            emitters,
            light,
            decals,
        })
    }

    /// Reads and parses a descriptor, reading at most [`MAX_VFX_BYTES`] + 1 bytes.
    ///
    /// # Errors
    /// An I/O, size, TOML or range error.
    pub fn load_file(path: &Path) -> Result<Self, VfxError> {
        Self::parse(&read_bounded(path, MAX_VFX_BYTES)?)
    }

    /// Whether any emitter or decal is an area telegraph.
    #[must_use]
    pub fn has_telegraph(&self) -> bool {
        self.emitters.iter().any(|emitter| emitter.telegraph)
            || self.decals.iter().any(|decal| decal.telegraph)
    }

    /// The longest time after the start at which a particle or an unstamped decal can still
    /// be alive, seconds (`event_seconds` stands in for `duration` when
    /// [`Self::event_duration`] is set). A stamped decal needs the effect only until its
    /// delay has passed on some frame (the stamp ring keeps it after that), so it counts
    /// as its delay plus [`STAMP_LATCH_SECONDS`]: a stamp whose delay is the effect's
    /// longest part still fires.
    #[must_use]
    pub fn lifespan(&self, event_seconds: f32) -> f32 {
        let duration = self.emit_seconds(event_seconds);
        let decals = self
            .decals
            .iter()
            .map(|decal| {
                if decal.stamp {
                    decal.delay + STAMP_LATCH_SECONDS
                } else {
                    decal.delay + decal.seconds(duration)
                }
            })
            .fold(duration, f32::max);
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
            .chain(self.light.map(|light| light.end(duration)))
            .fold(decals, f32::max)
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
    fn parses_an_effect_light_and_its_envelope() {
        let text = "duration = 1\n[light]\ncolor = [1, 0.5, 0.2]\nintensity = 6\nradius = 5\n\
                    fade_in = 0.1\nfade_out = 0.5\nflicker = 0.3\n\
                    [[emitters]]\nburst = 1\nlifetime = [0.2, 0.2]\nsize = [1, 1]\n\
                    color = [1, 1, 1, 1]";
        let effect = VfxEffect::parse(text).expect("effect");
        let light = effect.light.expect("light");
        assert_eq!(light.anchor, VfxAnchor::Target);
        assert_eq!(light.offset, [0.0, 1.0, 0.0]);
        assert!((light.intensity - 6.0).abs() < 1e-6);
        assert!((light.envelope(0.05, 1.0) - 0.5).abs() < 1e-5);
        assert!((light.envelope(0.5, 1.0) - 1.0).abs() < 1e-6);
        assert!((light.envelope(1.25, 1.0) - 0.5).abs() < 1e-5);
        assert_eq!(light.envelope(2.0, 1.0), 0.0);
        assert_eq!(light.envelope(f32::NAN, 1.0), 0.0);
        // The light outlives the particles, so the instance must too.
        assert!((effect.lifespan(0.0) - 1.5).abs() < 1e-6);
        assert!(VfxEffect::parse(BOLT).expect("bolt").light.is_none());
    }

    #[test]
    fn rejects_out_of_range_effect_lights() {
        let base = "duration = 1\n[[emitters]]\nburst = 1\nlifetime = [0.2, 0.2]\n\
                    size = [1, 1]\ncolor = [1, 1, 1, 1]\n[light]\n";
        for bad in [
            "color = [1, 1, 1]\nradius = 0",
            "color = [1, 1, 1]\nradius = 100",
            "color = [9, 1, 1]\nradius = 4",
            "color = [1, 1, 1]\nradius = 4\nintensity = 1000",
            "color = [1, 1, 1]\nradius = 4\nanchor = \"line\"",
            "color = [1, 1, 1]\nradius = 4\nflicker = 2",
            "color = [1, 1, 1]\nradius = 4\nfade_out = -1",
            "color = [1, 1, 1]\nradius = nan",
            "color = [1, 1, 1]\nradius = 4\nbrightness = 2",
        ] {
            assert!(
                VfxEffect::parse(&format!("{base}{bad}")).is_err(),
                "accepted {bad}"
            );
        }
    }

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

    const ZONE: &str = r#"
duration = 3
event_duration = true

[[decals]]
shape = "ring"
radius = 1.0
width = 0.3
color = [1.0, 0.25, 0.1, 0.8]
scale_with_radius = true
telegraph = true
progress = true

[[decals]]
shape = "splat"
delay = 0.5
lifetime = 12
color = [0.05, 0.03, 0.02, 0.9]
stamp = true
"#;

    /// A stamp whose delay is the effect's longest part keeps the effect alive past the
    /// delay by the latch, so a frame lands inside it whatever the frame step.
    #[test]
    fn late_stamps_extend_the_lifespan_past_their_delay() {
        let effect = VfxEffect::parse(
            "duration = 0.1\n[[emitters]]\nburst = 1\nlifetime = [0.2, 0.2]\nsize = [1, 1]\n\
             color = [1, 1, 1, 1]\n[[decals]]\nshape = \"splat\"\ndelay = 0.45\nlifetime = 4\n\
             stamp = true\ncolor = [0.1, 0.1, 0.1, 1]",
        )
        .expect("a late stamp");
        assert!((effect.lifespan(0.0) - (0.45 + STAMP_LATCH_SECONDS)).abs() < 1e-6);
    }

    #[test]
    fn decals_parse_with_defaults_and_count_towards_lifespan() {
        let effect = VfxEffect::parse(ZONE).expect("decal-only effect");
        assert_eq!(effect.emitters.len(), 0);
        assert_eq!(effect.decals.len(), 2);
        let ring = &effect.decals[0];
        assert_eq!(ring.shape, VfxDecalShape::Ring);
        assert_eq!(ring.style, VfxDecalStyle::Glow);
        assert_eq!(ring.anchor, VfxAnchor::Target);
        assert!(ring.telegraph && ring.progress && ring.scale_with_radius && !ring.stamp);
        assert_eq!(ring.lifetime, None);
        assert!((ring.angle - DEFAULT_DECAL_ANGLE).abs() < 1e-6);
        assert!((ring.fade_in - DEFAULT_DECAL_FADE_IN).abs() < 1e-6);
        assert!((ring.fade_out - DEFAULT_DECAL_FADE_OUT).abs() < 1e-6);
        let splat = &effect.decals[1];
        assert_eq!(splat.style, VfxDecalStyle::Paint, "splats default to paint");
        assert!((splat.radius - DEFAULT_DECAL_RADIUS).abs() < 1e-6);
        assert!((splat.width - DEFAULT_DECAL_WIDTH).abs() < 1e-6);
        assert!(effect.has_telegraph());
        // The ring lasts the zone (event_duration); the stamp only needs the effect until
        // its delay.
        assert!((ring.seconds(effect.emit_seconds(5.0)) - 5.0).abs() < 1e-6);
        assert!((effect.lifespan(5.0) - 5.0).abs() < 1e-6);
        assert!((effect.lifespan(0.0) - 3.0).abs() < 1e-6);
        // A decal that outlasts the emitters extends the lifespan.
        let late = VfxEffect::parse(
            "duration = 0.5\n[[emitters]]\nburst = 1\nlifetime = [0.2, 0.2]\nsize = [1, 1]\n\
             color = [1, 1, 1, 1]\n[[decals]]\nshape = \"cone\"\nangle = 45\ndelay = 0.25\n\
             lifetime = 2\nanchor = \"source\"\ncolor = [1, 1, 1, 1]",
        )
        .expect("emitter and decal");
        assert!((late.lifespan(0.0) - 2.25).abs() < 1e-6);
        assert!(!late.has_telegraph());
        assert_eq!(late.decals[0].anchor, VfxAnchor::Source);
    }

    #[test]
    fn rejects_hostile_or_contradictory_decals() {
        let decal = |body: &str| format!("duration = 1\n[[decals]]\ncolor = [1, 1, 1, 1]\n{body}");
        assert!(VfxEffect::parse(&decal("")).is_ok());
        for bad in [
            decal("extra = 1"),
            decal("shape = \"star\""),
            decal("style = \"neon\""),
            decal("anchor = \"sky\""),
            decal("radius = 0"),
            decal("radius = 51"),
            decal("radius = nan"),
            decal("width = 0.01"),
            decal("width = 21"),
            decal("angle = 0"),
            decal("angle = 361"),
            decal("delay = -1"),
            decal("delay = 31"),
            decal("lifetime = 0"),
            decal("lifetime = 31"),
            decal("fade_in = 11"),
            decal("fade_out = -0.1"),
            decal("team_tint = 2"),
            decal("color = [33, 1, 1, 1]"),
            decal("color = [1, 1, 1, 1.5]"),
            String::from("duration = 1\n[[decals]]\nradius = 1"),
            // A stamp needs a lifetime, and is never a telegraph or a progress fill.
            decal("stamp = true"),
            decal("stamp = true\nlifetime = 5\ntelegraph = true"),
            decal("stamp = true\nlifetime = 5\nprogress = true"),
            // A telegraph is never dimmed by lighting or fog.
            decal("telegraph = true\nstyle = \"paint\""),
            decal("telegraph = true\nshape = \"splat\""),
            format!(
                "duration = 1\n{}",
                "[[decals]]\ncolor = [1, 1, 1, 1]\n".repeat(MAX_VFX_DECALS + 1)
            ),
        ] {
            assert!(VfxEffect::parse(&bad).is_err(), "{bad:?}");
        }
        let four = format!(
            "duration = 1\n{}",
            "[[decals]]\ncolor = [1, 1, 1, 1]\n".repeat(MAX_VFX_DECALS)
        );
        assert_eq!(
            VfxEffect::parse(&four).expect("four decals").decals.len(),
            4
        );
        // Eight emitters plus four decals is the most an effect holds.
        let full = format!(
            "{four}{}",
            "[[emitters]]\nburst = 1\nlifetime = [1, 1]\nsize = [1, 1]\ncolor = [1, 1, 1, 1]\n"
                .repeat(MAX_VFX_EMITTERS)
        );
        let full = VfxEffect::parse(&full).expect("full effect");
        assert_eq!((full.emitters.len(), full.decals.len()), (8, 4));
    }

    /// ADR-0019: a heat-haze emitter carries strength, scale, speed and an optional
    /// fallback sprite; colour keys, other primitives, telegraphs and stray haze keys fail.
    #[test]
    fn check_vfx_dir_loads_every_effect_and_resolves_references() {
        let root = std::env::temp_dir().join(format!("moba-vfx-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // No directory: nothing to check, unless something names an effect.
        assert_eq!(
            super::check_vfx_dir(&root, &[]),
            super::VfxDirReport::default()
        );
        assert_eq!(
            super::check_vfx_dir(&root, &[("map 'ember'", "heat-vent")])
                .errors
                .len(),
            1
        );
        std::fs::create_dir_all(root.join("vfx")).expect("vfx dir");
        let haze = "duration = 1\n[[emitters]]\nburst = 1\nlifetime = [1, 1]\nsize = [1, 1]\n\
                    blend = \"distortion\"\ndistortion = 0.5\n";
        std::fs::write(root.join("vfx/heat-vent.toml"), haze).expect("write");
        // A distortion emitter with a colour is invalid content.
        std::fs::write(
            root.join("vfx/bad.toml"),
            format!("{haze}color = [1, 1, 1, 1]\n"),
        )
        .expect("write");
        std::fs::write(
            root.join("vfx/bindings.toml"),
            "[defaults]\nhit = \"heat-vent\"\nzone = \"gone\"\n",
        )
        .expect("write");
        let report =
            super::check_vfx_dir(&root, &[("map 'ember'", "heat-vent"), ("map 'x'", "bad")]);
        assert_eq!(report.effects, 1);
        assert_eq!(report.errors.len(), 3, "{:?}", report.errors);
        assert!(report.errors[0].starts_with("vfx/bad.toml: "));
        assert!(report.errors[1].contains("'gone'"));
        assert!(report.errors[2].starts_with("map 'x' names effect 'bad'"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_distortion_emitters_and_rejects_invalid_ones() {
        let haze = "duration = 1\n[[emitters]]\nblend = \"distortion\"\nrate = 8\n\
                    lifetime = [1, 1.5]\nsize = [1.5, 2]\ndistortion = 0.6\n";
        let effect = VfxEffect::parse(haze).expect("haze");
        let emitter = &effect.emitters[0];
        assert_eq!(emitter.blend, VfxBlend::Distortion);
        assert_eq!(emitter.color, [0.0; 4]);
        let distortion = emitter.distortion.expect("distortion parameters");
        assert!((distortion.strength - 0.6).abs() < 1e-6);
        assert_eq!(distortion.strength_end, 0.0);
        assert_eq!((distortion.scale, distortion.speed), (2.0, 1.0));
        assert_eq!(distortion.fallback_color, None);
        assert_eq!(
            emitter.sprite.name, "soft",
            "the fallback sprite defaults to soft"
        );
        let full = VfxEffect::parse(&format!(
            "{haze}distortion_end = 0.2\ndistortion_scale = 4\ndistortion_speed = 3\n\
             fallback_color = [1, 0.4, 0.1, 0.1]\nsprite = \"dot\"\n"
        ))
        .expect("haze with every key");
        let distortion = full.emitters[0].distortion.expect("parameters");
        assert_eq!((distortion.scale, distortion.speed), (4.0, 3.0));
        assert_eq!(distortion.fallback_color, Some([1.0, 0.4, 0.1, 0.1]));
        assert_eq!(full.emitters[0].sprite.name, "dot");
        // Other emitters carry no haze parameters.
        let plain = VfxEffect::parse(BOLT).expect("bolt");
        assert!(
            plain
                .emitters
                .iter()
                .all(|emitter| emitter.distortion.is_none())
        );
        for bad in [
            // Strength is required and range-checked.
            haze.replace("distortion = 0.6\n", ""),
            haze.replace("distortion = 0.6", "distortion = 0.01"),
            haze.replace("distortion = 0.6", "distortion = 1.5"),
            haze.replace("distortion = 0.6", "distortion = nan"),
            format!("{haze}distortion_end = 2"),
            format!("{haze}distortion_scale = 0.1"),
            format!("{haze}distortion_scale = 9"),
            format!("{haze}distortion_speed = -1"),
            format!("{haze}fallback_color = [1, 1, 1, 0.9]"),
            format!("{haze}fallback_color = [9, 1, 1, 0.1]"),
            // Offsets, not colour; billboards only; never a telegraph.
            format!("{haze}color = [1, 1, 1, 1]"),
            format!("{haze}color_end = [1, 1, 1, 0]"),
            format!("{haze}team_tint = 0.5"),
            format!("{haze}width = 0.5"),
            format!("{haze}fps = 12"),
            format!("{haze}telegraph = true"),
            format!("{haze}primitive = \"ring\""),
            format!("{haze}primitive = \"trail\""),
            format!("{haze}primitive = \"mesh\""),
            format!("{haze}primitive = \"flipbook\"\nsprite = \"smoke\""),
            format!("{haze}distortion_amount = 1"),
            // Haze keys need the distortion blend; colour is required elsewhere.
            String::from(
                "duration = 1\n[[emitters]]\nburst = 1\nlifetime = [1, 1]\nsize = [1, 1]\n\
                 color = [1, 1, 1, 1]\ndistortion = 0.5",
            ),
            String::from(
                "duration = 1\n[[emitters]]\nburst = 1\nlifetime = [1, 1]\nsize = [1, 1]\n\
                 color = [1, 1, 1, 1]\nfallback_color = [1, 1, 1, 0.1]",
            ),
            String::from("duration = 1\n[[emitters]]\nburst = 1\nlifetime = [1, 1]\nsize = [1, 1]"),
        ] {
            assert!(VfxEffect::parse(&bad).is_err(), "{bad:?}");
        }
    }
}
