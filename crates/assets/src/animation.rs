//! Content-defined animation state machine, spring bones and sockets (ADR-0012).
//!
//! An animation set is a small, client-local TOML descriptor at
//! `<asset root>/animations/<model>.toml` that names a skinned model's clips, how they blend
//! and which presentation events trigger them. Like [`MapVisuals`](crate::MapVisuals) it is
//! untrusted and cosmetic: it never reaches the simulation, the wire, replays or the content
//! hash. See `docs/content-authoring.md` for the format.
//!
//! Three types split the work:
//!
//! - [`AnimationSet`]: the parsed, range-checked descriptor (names only). Parsing is bounded:
//!   at most [`MAX_ANIMATION_SET_BYTES`], unknown keys rejected, every number range-checked.
//! - [`AnimationGraph`]: an animation set bound to one [`Skeleton`]. Clip, joint and state
//!   names are resolved to indices once, at load time, so evaluation never compares strings
//!   except to match a cast's ability name.
//! - [`Animator`]: per-unit runtime state in fixed-size arrays. [`Animator::trigger`] queues
//!   an action stamped with the time its *release* frame (the hit or the cast) happens, and
//!   [`Animator::evaluate`] produces the frame's skinning matrices from an absolute clock.
//!   Neither allocates.
//!
//! **Time.** The animator is driven by an absolute clock (for a network client, the render
//! tick in seconds), never by accumulated frame deltas. Locomotion phase, action progress and
//! fades are pure functions of that clock and the trigger stamps, so a pinned render tick
//! gives the same pose however the frames before it were paced. Spring bones are the one
//! history-dependent part; they step at a fixed [`SPRING_STEP`] and are deterministic given
//! the sequence of `evaluate` calls, and [`Animator::reset_springs`] removes the history.

use std::io::Read;
use std::path::Path;

use moba_core::math::{Mat4, Quat, Vec3};
use serde::Deserialize;

use crate::pose::{BlendSpace1d, BoneMask, ClipWrap, Pose};
use crate::skin::{JointMatrix, MAX_JOINTS, Skeleton, valid_animation_name};

/// Largest animation set file accepted, in bytes.
pub const MAX_ANIMATION_SET_BYTES: u64 = 16 * 1024;
/// Most action states in one animation set.
pub const MAX_ANIMATION_STATES: usize = 16;
/// Most ability-name bindings in one animation set.
pub const MAX_ABILITY_BINDINGS: usize = 16;
/// Most spring chains in one animation set.
pub const MAX_SPRING_CHAINS: usize = 4;
/// Most spring joints across all chains.
pub const MAX_SPRING_JOINTS: usize = 16;
/// Most sockets in one animation set.
pub const MAX_SOCKETS: usize = 8;
/// Fixed spring-bone time step, seconds.
pub const SPRING_STEP: f32 = 1.0 / 60.0;
/// Most spring steps one [`Animator::evaluate`] runs; a longer gap drops the excess.
pub const MAX_SPRING_STEPS: u32 = 8;
/// Queued actions waiting for their start time.
const MAX_PENDING: usize = 4;
/// Two triggers of the same state this close (seconds) are the same action.
const DEDUPE_SECONDS: f32 = 0.1;
/// A gap (seconds) after which springs restart from the animated pose.
const SPRING_RESET_SECONDS: f32 = 0.5;

const FADE_RANGE: (f32, f32) = (0.0, 2.0);
const SPEED_RANGE: (f32, f32) = (0.1, 4.0);
const RELEASE_RANGE: (f32, f32) = (0.0, 10.0);
const LOCOMOTION_SPEED_RANGE: (f32, f32) = (0.0, 100.0);
const MOVE_SPEED_RANGE: (f32, f32) = (0.05, 100.0);
const LINGER_RANGE: (f32, f32) = (0.0, 30.0);
const UNIT_RANGE: (f32, f32) = (0.0, 1.0);
const GRAVITY_RANGE: (f32, f32) = (0.0, 50.0);
const OFFSET_RANGE: (f32, f32) = (-100.0, 100.0);
const DEGREES_RANGE: (f32, f32) = (-360.0, 360.0);

/// Why an animation set could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnimationSetError {
    /// The file does not exist or cannot be read; the text is the I/O error.
    Io(String),
    /// The path is not a regular file.
    NotAFile,
    /// The file is larger than [`MAX_ANIMATION_SET_BYTES`].
    TooLarge,
    /// TOML syntax or schema error (unknown key, wrong type, missing field).
    Toml(String),
    /// A value is out of range or a cap is exceeded; the text names it.
    Invalid(&'static str),
    /// A clip, joint or state name does not exist; the text names it.
    Unresolved(String),
}

impl core::fmt::Display for AnimationSetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "missing or unreadable ({error})"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::TooLarge => write!(f, "larger than {MAX_ANIMATION_SET_BYTES} bytes"),
            Self::Toml(error) => write!(f, "animation set TOML: {}", error.trim_end()),
            Self::Invalid(what) => write!(f, "animation set: {what}"),
            Self::Unresolved(what) => write!(f, "animation set: unknown {what}"),
        }
    }
}

impl std::error::Error for AnimationSetError {}

/// How an action state plays its clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PlayMode {
    /// Play once, hold the last frame while fading out.
    Once,
    /// Loop until replaced or interrupted.
    Loop,
    /// Play once and hold the last frame until replaced (death).
    Hold,
}

/// Which joints an action state drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Layer {
    /// The whole body.
    Full,
    /// The upper-body mask; the lower body joins in only while the unit stands still, so a
    /// cast plays over the run cycle while moving.
    Upper,
    /// Added on top of everything else as the difference from the clip's first frame (hit
    /// reactions).
    Additive,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSet {
    #[serde(default)]
    locomotion: Vec<RawLocomotion>,
    #[serde(default)]
    move_speed: Option<f32>,
    #[serde(default)]
    upper_body: Option<String>,
    #[serde(default)]
    states: Vec<RawState>,
    #[serde(default)]
    triggers: RawTriggers,
    #[serde(default)]
    abilities: Vec<RawAbility>,
    #[serde(default)]
    springs: Vec<RawSpring>,
    #[serde(default)]
    sockets: Vec<RawSocket>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLocomotion {
    clip: String,
    speed: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawState {
    name: String,
    clip: String,
    #[serde(default)]
    mode: Option<PlayMode>,
    #[serde(default)]
    layer: Option<Layer>,
    #[serde(default)]
    fade_in: Option<f32>,
    #[serde(default)]
    fade_out: Option<f32>,
    #[serde(default)]
    speed: Option<f32>,
    #[serde(default)]
    release: Option<f32>,
    #[serde(default)]
    interrupt_on_move: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTriggers {
    #[serde(default)]
    attack: Option<String>,
    #[serde(default)]
    casts: Vec<String>,
    #[serde(default)]
    death: Option<String>,
    #[serde(default)]
    death_linger: Option<f32>,
    #[serde(default)]
    recall: Option<String>,
    #[serde(default)]
    hit: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAbility {
    name: String,
    state: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpring {
    joints: Vec<String>,
    #[serde(default)]
    stiffness: Option<f32>,
    #[serde(default)]
    damping: Option<f32>,
    #[serde(default)]
    gravity: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSocket {
    name: String,
    joint: String,
    #[serde(default)]
    offset: Option<[f32; 3]>,
    #[serde(default)]
    rotation: Option<[f32; 3]>,
}

/// One action state of an [`AnimationSet`].
#[derive(Clone, Debug, PartialEq)]
struct StateDef {
    name: String,
    clip: String,
    mode: PlayMode,
    layer: Layer,
    fade_in: f32,
    fade_out: f32,
    speed: f32,
    release: f32,
    interrupt_on_move: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct SpringDef {
    joints: Vec<String>,
    stiffness: f32,
    damping: f32,
    gravity: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct SocketDef {
    name: String,
    joint: String,
    offset: [f32; 3],
    rotation: [f32; 3],
}

/// A parsed, range-checked animation set descriptor. Bind it to a skeleton with
/// [`AnimationGraph::bind`].
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSet {
    locomotion: Vec<(String, f32)>,
    move_speed: f32,
    upper_body: Option<String>,
    states: Vec<StateDef>,
    attack: Option<String>,
    casts: Vec<String>,
    abilities: Vec<(String, String)>,
    death: Option<String>,
    death_linger: f32,
    recall: Option<String>,
    hit: Option<String>,
    springs: Vec<SpringDef>,
    sockets: Vec<SocketDef>,
}

fn check_range(
    value: f32,
    (min, max): (f32, f32),
    what: &'static str,
) -> Result<f32, AnimationSetError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(AnimationSetError::Invalid(what))
    }
}

fn check_name(name: String) -> Result<String, AnimationSetError> {
    if valid_animation_name(&name) {
        Ok(name)
    } else {
        Err(AnimationSetError::Invalid(
            "names must be 1-32 of [A-Za-z0-9_-]",
        ))
    }
}

fn check_optional(name: Option<String>) -> Result<Option<String>, AnimationSetError> {
    name.map(check_name).transpose()
}

impl AnimationSet {
    /// Parses and validates a descriptor.
    ///
    /// # Errors
    /// [`AnimationSetError::TooLarge`], a TOML error, or the first invalid value or cap.
    #[allow(clippy::too_many_lines)] // One linear validation pass over the schema.
    pub fn parse(text: &str) -> Result<Self, AnimationSetError> {
        if text.len() as u64 > MAX_ANIMATION_SET_BYTES {
            return Err(AnimationSetError::TooLarge);
        }
        let raw: RawSet =
            toml::from_str(text).map_err(|error| AnimationSetError::Toml(error.to_string()))?;
        if raw.locomotion.len() > crate::pose::MAX_BLEND_POINTS {
            return Err(AnimationSetError::Invalid("at most 8 locomotion points"));
        }
        if raw.states.len() > MAX_ANIMATION_STATES {
            return Err(AnimationSetError::Invalid("at most 16 states"));
        }
        if raw.abilities.len() > MAX_ABILITY_BINDINGS {
            return Err(AnimationSetError::Invalid("at most 16 ability bindings"));
        }
        if raw.triggers.casts.len() > 3 {
            return Err(AnimationSetError::Invalid("at most 3 cast slots (Q, W, E)"));
        }
        if raw.springs.len() > MAX_SPRING_CHAINS {
            return Err(AnimationSetError::Invalid("at most 4 spring chains"));
        }
        if raw
            .springs
            .iter()
            .map(|chain| chain.joints.len())
            .sum::<usize>()
            > MAX_SPRING_JOINTS
        {
            return Err(AnimationSetError::Invalid("at most 16 spring joints"));
        }
        if raw.sockets.len() > MAX_SOCKETS {
            return Err(AnimationSetError::Invalid("at most 8 sockets"));
        }
        let mut locomotion = Vec::with_capacity(raw.locomotion.len());
        for point in raw.locomotion {
            let speed = check_range(
                point.speed,
                LOCOMOTION_SPEED_RANGE,
                "locomotion speed must be 0..=100",
            )?;
            if locomotion.last().is_some_and(|(_, last)| speed <= *last) {
                return Err(AnimationSetError::Invalid(
                    "locomotion speeds must increase",
                ));
            }
            locomotion.push((check_name(point.clip)?, speed));
        }
        let mut states: Vec<StateDef> = Vec::with_capacity(raw.states.len());
        for state in raw.states {
            let name = check_name(state.name)?;
            if states
                .iter()
                .any(|other| other.name.eq_ignore_ascii_case(&name))
            {
                return Err(AnimationSetError::Invalid("state names must be unique"));
            }
            states.push(StateDef {
                name,
                clip: check_name(state.clip)?,
                mode: state.mode.unwrap_or(PlayMode::Once),
                layer: state.layer.unwrap_or(Layer::Full),
                fade_in: check_range(
                    state.fade_in.unwrap_or(0.1),
                    FADE_RANGE,
                    "fade_in must be 0..=2 seconds",
                )?,
                fade_out: check_range(
                    state.fade_out.unwrap_or(0.15),
                    FADE_RANGE,
                    "fade_out must be 0..=2 seconds",
                )?,
                speed: check_range(
                    state.speed.unwrap_or(1.0),
                    SPEED_RANGE,
                    "speed must be 0.1..=4",
                )?,
                release: check_range(
                    state.release.unwrap_or(0.0),
                    RELEASE_RANGE,
                    "release must be 0..=10 seconds",
                )?,
                interrupt_on_move: state.interrupt_on_move.unwrap_or(false),
            });
        }
        let mut abilities = Vec::with_capacity(raw.abilities.len());
        for binding in raw.abilities {
            if binding.name.is_empty() || binding.name.len() > 32 || !binding.name.is_ascii() {
                return Err(AnimationSetError::Invalid(
                    "ability names must be 1-32 ASCII bytes",
                ));
            }
            abilities.push((binding.name, check_name(binding.state)?));
        }
        let mut springs = Vec::with_capacity(raw.springs.len());
        for chain in raw.springs {
            if chain.joints.is_empty() || chain.joints.len() > 8 {
                return Err(AnimationSetError::Invalid(
                    "spring chains need 1 to 8 joints",
                ));
            }
            springs.push(SpringDef {
                joints: chain
                    .joints
                    .into_iter()
                    .map(check_name)
                    .collect::<Result<_, _>>()?,
                stiffness: check_range(
                    chain.stiffness.unwrap_or(0.2),
                    UNIT_RANGE,
                    "spring stiffness must be 0..=1",
                )?,
                damping: check_range(
                    chain.damping.unwrap_or(0.15),
                    UNIT_RANGE,
                    "spring damping must be 0..=1",
                )?,
                gravity: check_range(
                    chain.gravity.unwrap_or(9.81),
                    GRAVITY_RANGE,
                    "spring gravity must be 0..=50",
                )?,
            });
        }
        let mut sockets: Vec<SocketDef> = Vec::with_capacity(raw.sockets.len());
        for socket in raw.sockets {
            let name = check_name(socket.name)?;
            if sockets
                .iter()
                .any(|other| other.name.eq_ignore_ascii_case(&name))
            {
                return Err(AnimationSetError::Invalid("socket names must be unique"));
            }
            let offset = socket.offset.unwrap_or([0.0; 3]);
            let rotation = socket.rotation.unwrap_or([0.0; 3]);
            for value in offset {
                check_range(value, OFFSET_RANGE, "socket offset must be -100..=100")?;
            }
            for value in rotation {
                check_range(
                    value,
                    DEGREES_RANGE,
                    "socket rotation must be -360..=360 degrees",
                )?;
            }
            sockets.push(SocketDef {
                name,
                joint: check_name(socket.joint)?,
                offset,
                rotation,
            });
        }
        let triggers = raw.triggers;
        Ok(Self {
            locomotion,
            move_speed: check_range(
                raw.move_speed.unwrap_or(1.5),
                MOVE_SPEED_RANGE,
                "move_speed must be 0.05..=100",
            )?,
            upper_body: check_optional(raw.upper_body)?,
            states,
            attack: check_optional(triggers.attack)?,
            casts: triggers
                .casts
                .into_iter()
                .map(check_name)
                .collect::<Result<_, _>>()?,
            abilities,
            death: check_optional(triggers.death)?,
            death_linger: check_range(
                triggers.death_linger.unwrap_or(1.5),
                LINGER_RANGE,
                "death_linger must be 0..=30 seconds",
            )?,
            recall: check_optional(triggers.recall)?,
            hit: check_optional(triggers.hit)?,
            springs,
            sockets,
        })
    }

    /// Reads and parses a descriptor file, reading at most [`MAX_ANIMATION_SET_BYTES`] + 1
    /// bytes whatever the file claims or grows to.
    ///
    /// # Errors
    /// An I/O, size, TOML or range error.
    pub fn load_file(path: &Path) -> Result<Self, AnimationSetError> {
        let file =
            std::fs::File::open(path).map_err(|error| AnimationSetError::Io(error.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|error| AnimationSetError::Io(error.to_string()))?;
        if !metadata.is_file() {
            return Err(AnimationSetError::NotAFile);
        }
        if metadata.len() > MAX_ANIMATION_SET_BYTES {
            return Err(AnimationSetError::TooLarge);
        }
        let mut text = String::new();
        file.take(MAX_ANIMATION_SET_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|error| AnimationSetError::Io(error.to_string()))?;
        Self::parse(&text)
    }

    /// Number of action states.
    #[must_use]
    pub fn state_count(&self) -> usize {
        self.states.len()
    }

    /// Number of spring joints across all chains.
    #[must_use]
    pub fn spring_joint_count(&self) -> usize {
        self.springs.iter().map(|chain| chain.joints.len()).sum()
    }

    /// Number of sockets.
    #[must_use]
    pub fn socket_count(&self) -> usize {
        self.sockets.len()
    }
}

/// A resolved action state.
#[derive(Clone, Debug, PartialEq)]
struct State {
    name: Box<str>,
    clip: u8,
    duration: f32,
    mode: PlayMode,
    layer: Layer,
    fade_in: f32,
    fade_out: f32,
    speed: f32,
    release: f32,
    interrupt_on_move: bool,
}

/// One resolved spring joint.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SpringJoint {
    joint: u8,
    /// Bone tip in the joint's local frame.
    tip: Vec3,
    stiffness: f32,
    damping: f32,
    gravity: f32,
}

/// A resolved socket.
#[derive(Clone, Debug, PartialEq)]
struct Socket {
    name: Box<str>,
    joint: u8,
    offset: Mat4,
}

/// A socket of an [`AnimationGraph`], for [`Animator::socket_transform`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SocketId(u8);

/// An action state of an [`AnimationGraph`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StateId(u8);

impl StateId {
    /// The state's index in its graph.
    #[must_use]
    pub fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// An [`AnimationSet`] bound to one skeleton: every name resolved to an index.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationGraph {
    joint_count: usize,
    locomotion: Option<BlendSpace1d>,
    move_speed: f32,
    upper: BoneMask,
    states: Vec<State>,
    attack: Option<u8>,
    casts: [Option<u8>; 3],
    abilities: Vec<(Box<str>, u8)>,
    death: Option<u8>,
    death_linger: f32,
    recall: Option<u8>,
    hit: Option<u8>,
    springs: Vec<SpringJoint>,
    /// Spring slot of each joint, or `u8::MAX`.
    spring_slot: [u8; MAX_JOINTS],
    sockets: Vec<Socket>,
}

impl AnimationGraph {
    /// Resolves `set` against `skeleton`'s clip and joint names.
    ///
    /// # Errors
    /// [`AnimationSetError::Unresolved`] for a clip, joint or state that does not exist, or
    /// [`AnimationSetError::Invalid`] for a spring chain whose joints are not parent → child.
    #[allow(clippy::too_many_lines)] // One linear resolution pass over the set.
    pub fn bind(set: &AnimationSet, skeleton: &Skeleton) -> Result<Self, AnimationSetError> {
        let clip = |name: &str| {
            skeleton
                .clip_index(name)
                .and_then(|index| u8::try_from(index).ok())
                .ok_or_else(|| AnimationSetError::Unresolved(format!("clip '{name}'")))
        };
        let joint = |name: &str| {
            skeleton
                .joint_index(name)
                .and_then(|index| u8::try_from(index).ok())
                .ok_or_else(|| AnimationSetError::Unresolved(format!("joint '{name}'")))
        };
        let locomotion = if set.locomotion.is_empty() {
            None
        } else {
            let mut points = Vec::with_capacity(set.locomotion.len());
            for (name, speed) in &set.locomotion {
                points.push((*speed, usize::from(clip(name)?)));
            }
            Some(
                BlendSpace1d::new(&points)
                    .map_err(|_| AnimationSetError::Invalid("locomotion blend space"))?,
            )
        };
        let upper = match &set.upper_body {
            Some(name) => BoneMask::subtree(skeleton, usize::from(joint(name)?)),
            None => BoneMask::ALL,
        };
        let mut states = Vec::with_capacity(set.states.len());
        for state in &set.states {
            let index = clip(&state.clip)?;
            states.push(State {
                name: state.name.as_str().into(),
                clip: index,
                duration: skeleton.clips()[usize::from(index)].duration(),
                mode: state.mode,
                layer: state.layer,
                fade_in: state.fade_in,
                fade_out: state.fade_out,
                speed: state.speed,
                release: state.release,
                interrupt_on_move: state.interrupt_on_move,
            });
        }
        let state = |name: &str| {
            states
                .iter()
                .position(|state| state.name.eq_ignore_ascii_case(name))
                .and_then(|index| u8::try_from(index).ok())
                .ok_or_else(|| AnimationSetError::Unresolved(format!("state '{name}'")))
        };
        let optional = |name: &Option<String>| name.as_deref().map(state).transpose();
        let mut casts = [None; 3];
        for (slot, name) in set.casts.iter().enumerate() {
            casts[slot] = Some(state(name)?);
        }
        let mut abilities = Vec::with_capacity(set.abilities.len());
        for (ability, name) in &set.abilities {
            abilities.push((ability.as_str().into(), state(name)?));
        }
        let mut springs = Vec::with_capacity(set.spring_joint_count());
        let mut spring_slot = [u8::MAX; MAX_JOINTS];
        for chain in &set.springs {
            let mut previous: Option<u8> = None;
            let indices = chain
                .joints
                .iter()
                .map(|name| joint(name))
                .collect::<Result<Vec<_>, _>>()?;
            for (position, &index) in indices.iter().enumerate() {
                if let Some(parent) = previous
                    && skeleton.parents()[usize::from(index)] != Some(parent)
                {
                    return Err(AnimationSetError::Invalid(
                        "spring chain joints must each be the child of the previous one",
                    ));
                }
                if spring_slot[usize::from(index)] != u8::MAX {
                    return Err(AnimationSetError::Invalid(
                        "a joint may be in one spring chain once",
                    ));
                }
                // The bone points at the next chain joint; the last bone continues its
                // own direction from the parent.
                let tip = indices.get(position + 1).map_or_else(
                    || skeleton.rest()[usize::from(index)].translation,
                    |&next| skeleton.rest()[usize::from(next)].translation,
                );
                let tip = if tip.length_squared() > 1e-8 {
                    tip
                } else {
                    Vec3::NEG_Y * 0.25
                };
                spring_slot[usize::from(index)] = u8::try_from(springs.len())
                    .map_err(|_| AnimationSetError::Invalid("springs"))?;
                springs.push(SpringJoint {
                    joint: index,
                    tip,
                    stiffness: chain.stiffness,
                    damping: chain.damping,
                    gravity: chain.gravity,
                });
                previous = Some(index);
            }
        }
        let mut sockets = Vec::with_capacity(set.sockets.len());
        for socket in &set.sockets {
            let [x, y, z] = socket.rotation.map(f32::to_radians);
            sockets.push(Socket {
                name: socket.name.as_str().into(),
                joint: joint(&socket.joint)?,
                offset: Mat4::from_rotation_translation(
                    Quat::from_rotation_x(x) * Quat::from_rotation_y(y) * Quat::from_rotation_z(z),
                    Vec3::from_array(socket.offset),
                ),
            });
        }
        Ok(Self {
            joint_count: skeleton.joint_count(),
            locomotion,
            move_speed: set.move_speed,
            upper,
            attack: optional(&set.attack)?,
            casts,
            abilities,
            death: optional(&set.death)?,
            death_linger: set.death_linger,
            recall: optional(&set.recall)?,
            hit: optional(&set.hit)?,
            states,
            springs,
            spring_slot,
            sockets,
        })
    }

    /// Joint count of the skeleton this graph was bound to.
    #[must_use]
    pub fn joint_count(&self) -> usize {
        self.joint_count
    }

    /// The state called `name` (ASCII case-insensitive).
    #[must_use]
    pub fn state(&self, name: &str) -> Option<StateId> {
        self.states
            .iter()
            .position(|state| state.name.eq_ignore_ascii_case(name))
            .and_then(|index| u8::try_from(index).ok())
            .map(StateId)
    }

    /// The name of a state.
    #[must_use]
    pub fn state_name(&self, state: StateId) -> &str {
        self.states
            .get(state.index())
            .map_or("", |state| &state.name)
    }

    /// The socket called `name` (ASCII case-insensitive).
    #[must_use]
    pub fn socket(&self, name: &str) -> Option<SocketId> {
        self.sockets
            .iter()
            .position(|socket| socket.name.eq_ignore_ascii_case(name))
            .and_then(|index| u8::try_from(index).ok())
            .map(SocketId)
    }

    /// Number of spring joints.
    #[must_use]
    pub fn spring_joint_count(&self) -> usize {
        self.springs.len()
    }

    /// The state an event plays, if the set binds one.
    #[must_use]
    pub fn state_for(&self, event: AnimationEvent<'_>) -> Option<StateId> {
        let index = match event {
            AnimationEvent::Attack { .. } => self.attack,
            AnimationEvent::Cast { slot, ability } => ability
                .and_then(|ability| {
                    self.abilities
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case(ability))
                        .map(|(_, state)| *state)
                })
                .or_else(|| self.casts.get(usize::from(slot)).copied().flatten()),
            AnimationEvent::Death => self.death,
            AnimationEvent::Recall => self.recall,
            AnimationEvent::Hit => self.hit,
            AnimationEvent::Respawn => None,
        };
        index.map(StateId)
    }
}

/// A presentation event that can start an action (see [`Animator::trigger`]).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum AnimationEvent<'a> {
    /// A basic attack whose hit lands at the trigger time; `interval` (seconds, 0 if
    /// unknown) speeds the clip up so it fits the attack cadence.
    Attack {
        /// Seconds between attacks.
        interval: f32,
    },
    /// An ability cast (Q/W/E slot) released at the trigger time; the ability's name, when
    /// known, picks a state bound to it before the slot binding.
    Cast {
        /// Ability slot, 0..=2.
        slot: u8,
        /// Ability name from the catalogue.
        ability: Option<&'a str>,
    },
    /// The unit died: hold the death state until [`AnimationEvent::Respawn`].
    Death,
    /// The unit respawned: clear every action.
    Respawn,
    /// The unit started recalling (looping until it moves).
    Recall,
    /// The unit was hit (additive reaction).
    Hit,
}

/// One playing or queued action.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Play {
    state: u8,
    /// Clock time at clip time 0.
    start: f32,
    /// Clip seconds per clock second.
    rate: f32,
    /// Clock time the action was told to stop (fades out from here).
    stopped: Option<f32>,
}

/// Spring particle in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Particle {
    position: Vec3,
    previous: Vec3,
}

/// What an animator is playing at a time, for tests, tools and debug overlays.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct AnimatorReport {
    /// The current action.
    pub action: Option<StateId>,
    /// Its blend weight.
    pub action_weight: f32,
    /// Its clip time in seconds (unclamped: past the clip's end while fading out).
    pub action_time: f32,
    /// The action being crossfaded out.
    pub previous: Option<StateId>,
    /// Its blend weight (before the current action is layered on top).
    pub previous_weight: f32,
    /// The additive reaction's weight.
    pub additive_weight: f32,
    /// The locomotion blend: `(lower clip, upper clip, weight of upper)`.
    pub locomotion: (usize, usize, f32),
    /// Whether the unit is dead.
    pub dead: bool,
}

/// Preallocated scratch poses and transforms shared by every [`Animator`] of a client.
#[derive(Clone, Debug, Default)]
pub struct AnimationScratch {
    base: Pose,
    layer: Pose,
    extra: Pose,
}

/// Per-unit animation runtime (fixed size, no heap; about 4 KiB).
#[derive(Clone, Debug)]
pub struct Animator {
    phase: f32,
    current: Option<Play>,
    previous: Option<Play>,
    additive: Option<Play>,
    pending: [Option<Play>; MAX_PENDING],
    dead: bool,
    died_at: f32,
    particles: [Particle; MAX_SPRING_JOINTS],
    springs_live: bool,
    spring_clock: f32,
    spring_accumulator: f32,
    model: [Mat4; MAX_JOINTS],
    matrices: [JointMatrix; MAX_JOINTS],
    sockets: [Mat4; MAX_SOCKETS],
    joint_count: usize,
}

impl Default for Animator {
    fn default() -> Self {
        Self::new(0.0)
    }
}

/// Linear ramp `0..=1` over `span` seconds (an instant step when `span` is 0).
fn ramp(elapsed: f32, span: f32) -> f32 {
    if span <= 0.0 {
        if elapsed >= 0.0 { 1.0 } else { 0.0 }
    } else {
        (elapsed / span).clamp(0.0, 1.0)
    }
}

impl Animator {
    /// A fresh animator. `phase` (seconds) offsets the locomotion cycle so units do not
    /// animate in lockstep.
    #[must_use]
    pub fn new(phase: f32) -> Self {
        Self {
            phase: if phase.is_finite() { phase } else { 0.0 },
            current: None,
            previous: None,
            additive: None,
            pending: [None; MAX_PENDING],
            dead: false,
            died_at: 0.0,
            particles: [Particle {
                position: Vec3::ZERO,
                previous: Vec3::ZERO,
            }; MAX_SPRING_JOINTS],
            springs_live: false,
            spring_clock: 0.0,
            spring_accumulator: 0.0,
            model: [Mat4::IDENTITY; MAX_JOINTS],
            matrices: [[[0.0; 4]; 3]; MAX_JOINTS],
            sockets: [Mat4::IDENTITY; MAX_SOCKETS],
            joint_count: 0,
        }
    }

    /// Clears every action, the dead flag and the spring history (a new life or unit).
    pub fn reset(&mut self) {
        let phase = self.phase;
        *self = Self::new(phase);
    }

    /// Restarts spring bones from the animated pose on the next evaluation.
    pub fn reset_springs(&mut self) {
        self.springs_live = false;
    }

    /// Whether the unit is dead (a [`AnimationEvent::Death`] without a later respawn).
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// Starts the action `event` binds in `graph`, stamped with the clock time `at` (in
    /// seconds) of its release frame: the state's `release` time lines up with `at`, so a
    /// wind-up starts before it. A start in the future waits in a small queue; one in the
    /// past starts part-way through. Returns whether the event did anything.
    ///
    /// A repeat of the playing or queued state within 0.1 s is ignored, so a predicted
    /// attack is not restarted when the real one is observed. Never allocates.
    pub fn trigger(&mut self, graph: &AnimationGraph, event: AnimationEvent<'_>, at: f32) -> bool {
        if !at.is_finite() {
            return false;
        }
        match event {
            AnimationEvent::Respawn => {
                let was = self.dead || self.current.is_some();
                self.reset();
                return was;
            }
            AnimationEvent::Death => {
                if self.dead {
                    return false;
                }
                self.dead = true;
                self.died_at = at;
                self.pending = [None; MAX_PENDING];
                self.additive = None;
            }
            _ if self.dead => return false,
            _ => {}
        }
        let Some(state_id) = graph.state_for(event) else {
            return matches!(event, AnimationEvent::Death);
        };
        let Some(state) = graph.states.get(state_id.index()) else {
            return false;
        };
        let mut rate = state.speed;
        if let AnimationEvent::Attack { interval } = event
            && interval.is_finite()
            && interval > 0.05
            && state.duration / rate > interval
        {
            // Speed the swing up so one fits the attack cadence.
            rate = (state.duration / interval).min(SPEED_RANGE.1);
        }
        let play = Play {
            state: state_id.0,
            start: at - state.release / rate,
            rate,
            stopped: None,
        };
        if state.layer == Layer::Additive {
            self.additive = Some(play);
            return true;
        }
        let duplicate = |other: &Option<Play>| {
            other.is_some_and(|other| {
                other.state == play.state
                    && other.stopped.is_none()
                    && (other.start - play.start).abs() < DEDUPE_SECONDS
            })
        };
        if duplicate(&self.current) || self.pending.iter().any(duplicate) {
            return false;
        }
        if matches!(event, AnimationEvent::Death) {
            // Death replaces everything at once.
            self.previous = self.current.take();
            self.current = Some(play);
            return true;
        }
        // Queue in start order; when full, the latest start is dropped.
        let slot = self.pending.iter().position(Option::is_none).or_else(|| {
            let (latest, start) = self
                .pending
                .iter()
                .enumerate()
                .filter_map(|(index, play)| play.map(|play| (index, play.start)))
                .fold((0, f32::NEG_INFINITY), |best, item| {
                    if item.1 > best.1 { item } else { best }
                });
            (play.start < start).then_some(latest)
        });
        if let Some(slot) = slot {
            self.pending[slot] = Some(play);
            true
        } else {
            false
        }
    }

    /// Clock time an action ends (after its fade-out), or infinity while it loops or holds.
    fn end_time(graph: &AnimationGraph, play: &Play) -> f32 {
        let Some(state) = graph.states.get(usize::from(play.state)) else {
            return f32::NEG_INFINITY;
        };
        if let Some(stopped) = play.stopped {
            return stopped + state.fade_out;
        }
        match state.mode {
            PlayMode::Once => play.start + state.duration / play.rate + state.fade_out,
            PlayMode::Loop | PlayMode::Hold => f32::INFINITY,
        }
    }

    /// Blend weight of an action at `now`.
    fn weight(graph: &AnimationGraph, play: &Play, now: f32) -> f32 {
        let Some(state) = graph.states.get(usize::from(play.state)) else {
            return 0.0;
        };
        let mut weight = ramp(now - play.start, state.fade_in);
        let fade_from = match (play.stopped, state.mode) {
            (Some(stopped), _) => Some(stopped),
            (None, PlayMode::Once) => Some(play.start + state.duration / play.rate),
            (None, _) => None,
        };
        if let Some(from) = fade_from {
            weight *= 1.0 - ramp(now - from, state.fade_out);
        }
        weight
    }

    /// Promotes due actions and retires finished ones.
    fn advance(&mut self, graph: &AnimationGraph, now: f32, speed: f32) {
        loop {
            let due = self
                .pending
                .iter()
                .enumerate()
                .filter_map(|(index, play)| play.map(|play| (index, play.start)))
                .filter(|&(_, start)| start <= now)
                .fold(None::<(usize, f32)>, |best, item| match best {
                    Some(best) if best.1 <= item.1 => Some(best),
                    _ => Some(item),
                });
            let Some((index, _)) = due else {
                break;
            };
            let play = self.pending[index].take();
            if self.current.is_some() {
                self.previous = self.current;
            }
            self.current = play;
        }
        let moving = speed > graph.move_speed;
        if let Some(play) = self.current.as_mut()
            && play.stopped.is_none()
            && moving
            && graph
                .states
                .get(usize::from(play.state))
                .is_some_and(|state| state.interrupt_on_move)
        {
            play.stopped = Some(now);
        }
        if self
            .current
            .is_some_and(|play| now >= Self::end_time(graph, &play))
        {
            self.current = None;
        }
        // The previous action only matters until the current one has fully faded in.
        if let Some(previous) = self.previous {
            let covered = self
                .current
                .is_some_and(|play| Self::weight(graph, &play, now) >= 1.0);
            if covered || now >= Self::end_time(graph, &previous) {
                self.previous = None;
            }
        }
        if self
            .additive
            .is_some_and(|play| now >= Self::end_time(graph, &play))
        {
            self.additive = None;
        }
    }

    /// What the animator plays at `now` for a unit moving at `speed` m/s (without advancing
    /// it; [`Animator::evaluate`] does that).
    #[must_use]
    pub fn report(&self, graph: &AnimationGraph, now: f32, speed: f32) -> AnimatorReport {
        let mut copy = self.clone();
        copy.advance(graph, now, speed);
        AnimatorReport {
            action: copy.current.map(|play| StateId(play.state)),
            action_weight: copy
                .current
                .map_or(0.0, |play| Self::weight(graph, &play, now)),
            action_time: copy
                .current
                .map_or(0.0, |play| (now - play.start) * play.rate),
            previous: copy.previous.map(|play| StateId(play.state)),
            previous_weight: copy
                .previous
                .map_or(0.0, |play| Self::weight(graph, &play, now)),
            additive_weight: copy
                .additive
                .map_or(0.0, |play| Self::weight(graph, &play, now)),
            locomotion: graph
                .locomotion
                .map_or((0, 0, 0.0), |space| space.weights(speed)),
            dead: copy.dead,
        }
    }

    /// Whether to draw the unit at `now`: alive, or dead for less than its death clip plus
    /// the set's `death_linger`. Without a death state a dead unit is hidden at once.
    #[must_use]
    pub fn visible(&self, graph: &AnimationGraph, now: f32) -> bool {
        if !self.dead {
            return true;
        }
        let Some(state) = graph
            .death
            .and_then(|death| graph.states.get(usize::from(death)))
        else {
            return false;
        };
        now < self.died_at + state.duration / state.speed + graph.death_linger
    }

    /// Samples one action's clip into `out` at `now`.
    fn sample_action(
        graph: &AnimationGraph,
        skeleton: &Skeleton,
        play: &Play,
        now: f32,
        out: &mut Pose,
    ) -> Option<Layer> {
        let state = graph.states.get(usize::from(play.state))?;
        let time = (now - play.start) * play.rate;
        let wrap = match state.mode {
            PlayMode::Loop => ClipWrap::Loop,
            PlayMode::Once | PlayMode::Hold => ClipWrap::Clamp,
        };
        skeleton.sample_clip(usize::from(state.clip), time, wrap, out);
        Some(state.layer)
    }

    /// Advances to clock time `now` (seconds) and computes the frame's pose for a unit
    /// moving at `speed` m/s whose model is placed by `model_to_world` (springs react to
    /// its motion). Returns the skinning matrices, one per joint.
    ///
    /// `graph` must have been bound to `skeleton`; otherwise the rest pose is returned.
    /// Never allocates.
    pub fn evaluate(
        &mut self,
        graph: &AnimationGraph,
        skeleton: &Skeleton,
        now: f32,
        speed: f32,
        model_to_world: Mat4,
        scratch: &mut AnimationScratch,
    ) -> &[JointMatrix] {
        let count = skeleton.joint_count();
        self.joint_count = count;
        let now = if now.is_finite() { now } else { 0.0 };
        let speed = if speed.is_finite() && !self.dead {
            speed.max(0.0)
        } else {
            0.0
        };
        let AnimationScratch { base, layer, extra } = scratch;
        if graph.joint_count != count {
            base.set_rest(skeleton);
            self.finish(graph, skeleton, base, now, model_to_world);
            return &self.matrices[..count];
        }
        self.advance(graph, now, speed);
        match graph.locomotion {
            Some(space) => space.sample(skeleton, speed, now + self.phase, base, layer),
            None => base.set_rest(skeleton),
        }
        let lower = graph.upper.complement();
        let moving = (speed / graph.move_speed).clamp(0.0, 1.0);
        for play in [self.previous, self.current].into_iter().flatten() {
            let weight = Self::weight(graph, &play, now);
            if weight <= 0.0 {
                continue;
            }
            match Self::sample_action(graph, skeleton, &play, now, layer) {
                Some(Layer::Full) => base.blend(layer, weight, BoneMask::ALL),
                Some(Layer::Upper) => {
                    base.blend(layer, weight, graph.upper);
                    base.blend(layer, weight * (1.0 - moving), lower);
                }
                Some(Layer::Additive) => {
                    skeleton.sample_clip(
                        usize::from(graph.states[usize::from(play.state)].clip),
                        0.0,
                        ClipWrap::Clamp,
                        extra,
                    );
                    base.add(layer, extra, weight, BoneMask::ALL);
                }
                None => {}
            }
        }
        if let Some(play) = self.additive {
            let weight = Self::weight(graph, &play, now);
            if weight > 0.0 && Self::sample_action(graph, skeleton, &play, now, layer).is_some() {
                let clip = usize::from(graph.states[usize::from(play.state)].clip);
                skeleton.sample_clip(clip, 0.0, ClipWrap::Clamp, extra);
                base.add(layer, extra, weight, BoneMask::ALL);
            }
        }
        self.finish(graph, skeleton, base, now, model_to_world);
        &self.matrices[..count]
    }

    /// Model transforms with spring bones applied, then sockets and skinning matrices.
    fn finish(
        &mut self,
        graph: &AnimationGraph,
        skeleton: &Skeleton,
        pose: &Pose,
        now: f32,
        model_to_world: Mat4,
    ) {
        let count = skeleton.joint_count();
        let steps = self.spring_steps(graph, now);
        let world_rotation = model_to_world.to_scale_rotation_translation().1;
        let model_rotation = world_rotation.inverse();
        let local = pose.joints();
        for &joint in skeleton.evaluation_order() {
            let joint = usize::from(joint);
            let transform = local
                .get(joint)
                .map_or(Mat4::IDENTITY, crate::skin::JointTransform::matrix);
            let mut global = match skeleton.parents()[joint] {
                Some(parent) => self.model[usize::from(parent)] * transform,
                None => skeleton.base(joint) * transform,
            };
            let slot = graph.spring_slot.get(joint).copied().unwrap_or(u8::MAX);
            if graph.joint_count == count
                && let Some(spring) = graph.springs.get(usize::from(slot))
            {
                global = self.spring(
                    usize::from(slot),
                    spring,
                    global,
                    steps,
                    model_to_world,
                    model_rotation,
                    world_rotation,
                );
            }
            self.model[joint] = global;
        }
        self.springs_live = true;
        for (target, socket) in self.sockets.iter_mut().zip(&graph.sockets) {
            *target = self.model[usize::from(socket.joint).min(count - 1)] * socket.offset;
        }
        skeleton.skinning_matrices(&self.model[..count], &mut self.matrices[..count]);
    }

    /// Fixed spring steps to run at `now`, resetting the springs after a gap.
    fn spring_steps(&mut self, graph: &AnimationGraph, now: f32) -> u32 {
        if graph.springs.is_empty() {
            return 0;
        }
        let elapsed = now - self.spring_clock;
        self.spring_clock = now;
        if !self.springs_live || !(0.0..=SPRING_RESET_SECONDS).contains(&elapsed) {
            self.springs_live = false;
            self.spring_accumulator = 0.0;
            return 0;
        }
        self.spring_accumulator += elapsed;
        // `accumulator <= SPRING_RESET_SECONDS + SPRING_STEP`, a few dozen steps at most.
        let steps = (self.spring_accumulator / SPRING_STEP) as u32;
        self.spring_accumulator -= steps as f32 * SPRING_STEP;
        steps.min(MAX_SPRING_STEPS)
    }

    /// Steps one spring particle and bends `global` towards it.
    #[allow(clippy::too_many_arguments)] // One joint's full spring inputs.
    fn spring(
        &mut self,
        slot: usize,
        spring: &SpringJoint,
        global: Mat4,
        steps: u32,
        model_to_world: Mat4,
        model_rotation: Quat,
        world_rotation: Quat,
    ) -> Mat4 {
        let to_world = model_to_world * global;
        let origin = to_world.transform_point3(Vec3::ZERO);
        let target = to_world.transform_point3(spring.tip);
        let length = origin.distance(target);
        let particle = &mut self.particles[slot];
        if !self.springs_live
            || !particle.position.is_finite()
            || particle.position.distance(target) > length * 10.0 + 1.0
        {
            *particle = Particle {
                position: target,
                previous: target,
            };
            return global;
        }
        let gravity = Vec3::NEG_Y * spring.gravity * SPRING_STEP * SPRING_STEP;
        for _ in 0..steps {
            let velocity = (particle.position - particle.previous) * (1.0 - spring.damping);
            particle.previous = particle.position;
            let mut position = particle.position + velocity + gravity;
            position += (target - position) * spring.stiffness;
            let offset = position - origin;
            let distance = offset.length();
            particle.position = if distance > 1e-6 {
                origin + offset * (length / distance)
            } else {
                target
            };
        }
        let animated = (target - origin).normalize_or_zero();
        let simulated = (particle.position - origin).normalize_or_zero();
        if animated == Vec3::ZERO || simulated == Vec3::ZERO {
            return global;
        }
        let delta = model_rotation * Quat::from_rotation_arc(animated, simulated) * world_rotation;
        let pivot = global.w_axis.truncate();
        Mat4::from_translation(pivot)
            * Mat4::from_quat(delta.normalize())
            * Mat4::from_translation(-pivot)
            * global
    }

    /// The last evaluated skinning matrices (for a frame that skips evaluation).
    #[must_use]
    pub fn matrices(&self) -> &[JointMatrix] {
        &self.matrices[..self.joint_count]
    }

    /// A socket's model-space transform from the last evaluation; multiply by the
    /// instance's model-to-world transform for a world position.
    #[must_use]
    pub fn socket_transform(&self, socket: SocketId) -> Option<Mat4> {
        (self.joint_count > 0)
            .then(|| self.sockets.get(usize::from(socket.0)).copied())
            .flatten()
    }

    /// A joint's model-space transform from the last evaluation.
    #[must_use]
    pub fn joint_transform(&self, joint: usize) -> Option<Mat4> {
        (joint < self.joint_count).then(|| self.model[joint])
    }
}

/// The skinning matrices for `pose` without springs or sockets (a convenience for tools).
///
/// Returns `false` if `out` is shorter than the joint count.
pub fn pose_matrices(skeleton: &Skeleton, pose: &Pose, out: &mut [JointMatrix]) -> bool {
    let mut model = [Mat4::IDENTITY; MAX_JOINTS];
    skeleton.model_transforms(pose, &mut model)
        && skeleton.skinning_matrices(&model[..skeleton.joint_count()], out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::{AnimationClip, Interpolation, JointTransform, Track, TrackPath};

    const HIPS: usize = 0;
    const CHEST: usize = 1;
    const ARM: usize = 2;
    const LEG: usize = 3;
    const TAIL: [usize; 2] = [4, 5];

    fn rotation_clip(name: &str, joint: u8, times: &[f32], angles: &[f32]) -> AnimationClip {
        let track = Track::new(
            joint,
            TrackPath::Rotation,
            Interpolation::Linear,
            times.to_vec(),
            angles
                .iter()
                .map(|&angle| Quat::from_rotation_x(angle).to_array())
                .collect(),
        )
        .expect("track");
        AnimationClip::new(vec![track])
            .and_then(|clip| clip.with_name(name))
            .expect("clip")
    }

    /// hips → chest → arm, hips → leg, chest → tail0 → tail1; one clip per state.
    fn rig() -> Skeleton {
        let at = |x: f32, y: f32, z: f32| JointTransform {
            translation: Vec3::new(x, y, z),
            ..JointTransform::IDENTITY
        };
        let rest = vec![
            at(0.0, 1.0, 0.0),
            at(0.0, 0.5, 0.0),
            at(0.5, 0.5, 0.0),
            at(0.2, -0.1, 0.0),
            at(0.0, 0.5, -0.3),
            at(0.0, -0.5, 0.0),
        ];
        let clips = vec![
            rotation_clip("idle", 1, &[0.0, 1.0, 2.0], &[0.0, 0.05, 0.0]),
            rotation_clip("run", 3, &[0.0, 0.4, 0.8], &[0.5, -0.5, 0.5]),
            rotation_clip("attack", 2, &[0.0, 0.35, 0.7], &[0.0, 1.2, 0.0]),
            rotation_clip("cast", 2, &[0.0, 0.3, 0.6], &[0.0, -1.0, 0.0]),
            rotation_clip("death", 0, &[0.0, 1.0], &[0.0, -1.5]),
            rotation_clip("recall", 1, &[0.0, 1.0, 2.0], &[0.3, 0.4, 0.3]),
            rotation_clip("hit", 1, &[0.0, 0.1, 0.3], &[0.0, -0.3, 0.0]),
        ];
        Skeleton::new(
            vec![None, Some(0), Some(1), Some(0), Some(1), Some(4)],
            rest,
            vec![Mat4::IDENTITY; 6],
            vec![Mat4::IDENTITY; 6],
            None,
        )
        .and_then(|skeleton| skeleton.with_clips(clips))
        .and_then(|skeleton| {
            skeleton.with_joint_names(
                ["hips", "chest", "arm", "leg", "tail0", "tail1"]
                    .map(|name| Some(String::from(name)))
                    .to_vec(),
            )
        })
        .expect("rig")
    }

    const SET: &str = r#"
move_speed = 1.0
upper_body = "chest"

[[locomotion]]
clip = "idle"
speed = 0.0

[[locomotion]]
clip = "run"
speed = 4.0

[[states]]
name = "attack"
clip = "attack"
layer = "upper"
fade_in = 0.1
fade_out = 0.1
release = 0.35
interrupt_on_move = true

[[states]]
name = "cast"
clip = "cast"
layer = "upper"
fade_in = 0.2
fade_out = 0.1
release = 0.3

[[states]]
name = "nova"
clip = "cast"
layer = "full"
fade_in = 0.0
fade_out = 0.0

[[states]]
name = "death"
clip = "death"
mode = "hold"
fade_in = 0.1

[[states]]
name = "recall"
clip = "recall"
mode = "loop"
fade_in = 0.3
fade_out = 0.2
interrupt_on_move = true

[[states]]
name = "hit"
clip = "hit"
layer = "additive"
fade_in = 0.0
fade_out = 0.1

[triggers]
attack = "attack"
casts = ["cast", "cast", "nova"]
death = "death"
death_linger = 1.0
recall = "recall"
hit = "hit"

[[abilities]]
name = "Frost Nova"
state = "nova"

[[springs]]
joints = ["tail0", "tail1"]
stiffness = 0.1
damping = 0.1
gravity = 9.81

[[sockets]]
name = "hand"
joint = "arm"
offset = [0.0, -0.5, 0.0]
rotation = [0.0, 90.0, 0.0]
"#;

    fn graph() -> (Skeleton, AnimationGraph) {
        let skeleton = rig();
        let set = AnimationSet::parse(SET).expect("set parses");
        assert_eq!(set.state_count(), 6);
        assert_eq!(set.spring_joint_count(), 2);
        assert_eq!(set.socket_count(), 1);
        let graph = AnimationGraph::bind(&set, &skeleton).expect("set binds");
        (skeleton, graph)
    }

    fn name(graph: &AnimationGraph, state: Option<StateId>) -> Option<&str> {
        state.map(|state| graph.state_name(state))
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn cast(slot: u8) -> AnimationEvent<'static> {
        AnimationEvent::Cast {
            slot,
            ability: None,
        }
    }

    const ATTACK: AnimationEvent<'static> = AnimationEvent::Attack { interval: 0.0 };

    #[test]
    fn descriptors_reject_bad_input() {
        let invalid = |text: &str| AnimationSet::parse(text).expect_err(text);
        assert_eq!(
            AnimationSet::parse(&"#".repeat(17 * 1024)),
            Err(AnimationSetError::TooLarge)
        );
        assert!(matches!(invalid("bogus = 1"), AnimationSetError::Toml(_)));
        for text in [
            "[[states]]\nname = \"a b\"\nclip = \"x\"",
            "[[states]]\nname = \"a\"\nclip = \"x\"\nfade_in = -1.0",
            "[[states]]\nname = \"a\"\nclip = \"x\"\nspeed = nan",
            "[[states]]\nname = \"a\"\nclip = \"x\"\n[[states]]\nname = \"A\"\nclip = \"y\"",
            "[triggers]\ncasts = [\"a\", \"b\", \"c\", \"d\"]",
            "[[locomotion]]\nclip = \"a\"\nspeed = 2.0\n[[locomotion]]\nclip = \"b\"\nspeed = 1.0",
            "[[springs]]\njoints = []",
            "[[springs]]\njoints = [\"a\"]\nstiffness = 2.0",
            "[[sockets]]\nname = \"a\"\njoint = \"b\"\noffset = [0.0, 1e9, 0.0]",
        ] {
            assert!(
                matches!(invalid(text), AnimationSetError::Invalid(_)),
                "{text}"
            );
        }
        assert!(matches!(
            invalid("[[states]]\nname = \"a\"\nclip = \"x\"\nmode = \"bounce\""),
            AnimationSetError::Toml(_)
        ));
        let many = "[[states]]\nname = \"s\"\nclip = \"x\"\n".repeat(17);
        assert!(matches!(invalid(&many), AnimationSetError::Invalid(_)));
        // Parses, but names something the skeleton does not have.
        let skeleton = rig();
        for (text, missing) in [
            ("[[states]]\nname = \"a\"\nclip = \"nope\"", "clip 'nope'"),
            ("upper_body = \"wing\"", "joint 'wing'"),
            ("[triggers]\nattack = \"swing\"", "state 'swing'"),
            (
                "[[sockets]]\nname = \"a\"\njoint = \"tail9\"",
                "joint 'tail9'",
            ),
        ] {
            let set = AnimationSet::parse(text).expect(text);
            assert_eq!(
                AnimationGraph::bind(&set, &skeleton),
                Err(AnimationSetError::Unresolved(missing.into()))
            );
        }
        // A spring chain must run parent → child.
        let set = AnimationSet::parse("[[springs]]\njoints = [\"tail0\", \"leg\"]").expect("set");
        assert!(matches!(
            AnimationGraph::bind(&set, &skeleton),
            Err(AnimationSetError::Invalid(_))
        ));
        // An empty set binds: rest pose, no triggers.
        let graph = AnimationGraph::bind(&AnimationSet::parse("").expect("empty"), &skeleton)
            .expect("bind");
        assert_eq!(graph.state_for(AnimationEvent::Death), None);
    }

    #[test]
    fn events_pick_states_and_release_frames_line_up() {
        let (skeleton, graph) = graph();
        let mut animator = Animator::new(0.0);
        let attack = graph.state("attack");
        // The hit lands at t = 2.0; the swing starts 0.35 s before so the release frame
        // (0.35 s into the clip) is on the hit.
        assert!(animator.trigger(&graph, ATTACK, 2.0));
        assert_eq!(
            animator.report(&graph, 1.6, 0.0).action,
            None,
            "queued until its start"
        );
        let report = animator.report(&graph, 2.0, 0.0);
        assert_eq!(report.action, attack);
        assert!(close(report.action_time, 0.35), "{report:?}");
        assert!(close(report.action_weight, 1.0));
        // Repeating the same attack (a prediction confirmed by the snapshot) is a no-op.
        assert!(!animator.trigger(&graph, AnimationEvent::Attack { interval: 0.0 }, 2.05));
        // Fading in from the start and out after the end.
        let mut scratch = AnimationScratch::default();
        animator.evaluate(&graph, &skeleton, 1.70, 0.0, Mat4::IDENTITY, &mut scratch);
        let fading = animator.report(&graph, 1.70, 0.0);
        assert!(close(fading.action_weight, 0.5), "{fading:?}");
        let ending = animator.report(&graph, 1.65 + 0.7 + 0.05, 0.0);
        assert!(close(ending.action_weight, 0.5), "{ending:?}");
        assert_eq!(animator.report(&graph, 3.0, 0.0).action, None);
        // A short attack interval speeds the swing up to fit; the release stays on the hit.
        let mut fast = Animator::new(0.0);
        fast.trigger(&graph, AnimationEvent::Attack { interval: 0.35 }, 1.0);
        assert!(close(fast.report(&graph, 1.0, 0.0).action_time, 0.35));
        assert!(close(fast.report(&graph, 0.9, 0.0).action_time, 0.15));

        // Casts: an ability name binding wins over the slot binding.
        let mut caster = Animator::new(0.0);
        caster.trigger(
            &graph,
            AnimationEvent::Cast {
                slot: 0,
                ability: Some("frost nova"),
            },
            1.0,
        );
        assert_eq!(
            name(&graph, caster.report(&graph, 1.0, 0.0).action),
            Some("nova")
        );
        let mut caster = Animator::new(0.0);
        caster.trigger(
            &graph,
            AnimationEvent::Cast {
                slot: 1,
                ability: Some("Fireball"),
            },
            1.0,
        );
        assert_eq!(
            name(&graph, caster.report(&graph, 1.0, 0.0).action),
            Some("cast")
        );
        assert!(!caster.trigger(&graph, cast(7), 1.0));
        assert!(!caster.trigger(&graph, AnimationEvent::Hit, f32::NAN));
    }

    #[test]
    fn crossfades_masks_and_additive_layers() {
        let (skeleton, graph) = graph();
        let mut scratch = AnimationScratch::default();
        let mut animator = Animator::new(0.0);
        animator.trigger(&graph, ATTACK, 1.35);
        animator.trigger(&graph, cast(0), 1.6);
        // At 1.4 the cast (start 1.3) is 0.1 s into its 0.2 s fade-in over the attack.
        let report = animator.report(&graph, 1.4, 0.0);
        assert_eq!(name(&graph, report.action), Some("cast"));
        assert_eq!(name(&graph, report.previous), Some("attack"));
        assert!(close(report.action_weight, 0.5), "{report:?}");
        assert!(report.previous_weight > 0.9);
        // Fully faded in: the attack is dropped.
        assert_eq!(animator.report(&graph, 1.55, 0.0).previous, None);

        // Upper-body casts while running: the arm follows the cast, the leg the run cycle.
        let mut running = Animator::new(0.0);
        running.trigger(&graph, cast(0), 1.3);
        let mut standing = running.clone();
        let mut plain = Animator::new(0.0);
        running.evaluate(&graph, &skeleton, 1.3, 6.0, Mat4::IDENTITY, &mut scratch);
        standing.evaluate(&graph, &skeleton, 1.3, 0.0, Mat4::IDENTITY, &mut scratch);
        plain.evaluate(&graph, &skeleton, 1.3, 6.0, Mat4::IDENTITY, &mut scratch);
        let arm = |animator: &Animator| animator.joint_transform(ARM).expect("arm");
        let leg = |animator: &Animator| animator.joint_transform(LEG).expect("leg");
        assert_eq!(leg(&running), leg(&plain), "lower body keeps running");
        assert_ne!(arm(&running), arm(&plain), "upper body casts");
        assert!(arm(&running).abs_diff_eq(arm(&standing), 1e-5));
        assert_eq!(running.report(&graph, 1.3, 6.0).locomotion, (1, 1, 0.0));
        assert_eq!(running.report(&graph, 1.3, 2.0).locomotion, (0, 1, 0.5));

        // Additive hit: layered over whatever plays, then gone.
        let mut hit = Animator::new(0.0);
        let mut reference = Animator::new(0.0);
        hit.trigger(&graph, AnimationEvent::Hit, 1.0);
        hit.evaluate(&graph, &skeleton, 1.1, 0.0, Mat4::IDENTITY, &mut scratch);
        reference.evaluate(&graph, &skeleton, 1.1, 0.0, Mat4::IDENTITY, &mut scratch);
        assert!(close(hit.report(&graph, 1.1, 0.0).additive_weight, 1.0));
        assert_ne!(hit.joint_transform(CHEST), reference.joint_transform(CHEST));
        assert_eq!(hit.joint_transform(LEG), reference.joint_transform(LEG));
        hit.evaluate(&graph, &skeleton, 1.5, 0.0, Mat4::IDENTITY, &mut scratch);
        assert!(close(hit.report(&graph, 1.5, 0.0).additive_weight, 0.0));
    }

    #[test]
    fn death_holds_until_respawn_and_recall_stops_on_move() {
        let (skeleton, graph) = graph();
        let mut scratch = AnimationScratch::default();
        let mut animator = Animator::new(0.0);
        animator.trigger(&graph, AnimationEvent::Recall, 1.0);
        assert_eq!(
            name(&graph, animator.report(&graph, 5.0, 0.0).action),
            Some("recall"),
            "loops while still"
        );
        animator.evaluate(&graph, &skeleton, 5.0, 0.0, Mat4::IDENTITY, &mut scratch);
        animator.evaluate(&graph, &skeleton, 5.1, 3.0, Mat4::IDENTITY, &mut scratch);
        let report = animator.report(&graph, 5.2, 3.0);
        assert!(close(report.action_weight, 0.5), "fading out: {report:?}");
        assert_eq!(animator.report(&graph, 5.4, 3.0).action, None);

        animator.trigger(&graph, AnimationEvent::Death, 6.0);
        assert!(animator.is_dead());
        assert!(!animator.trigger(&graph, ATTACK, 6.1));
        assert!(!animator.trigger(&graph, AnimationEvent::Death, 6.2));
        let report = animator.report(&graph, 60.0, 5.0);
        assert_eq!(name(&graph, report.action), Some("death"));
        assert!(report.dead && close(report.action_weight, 1.0));
        // Clip (1 s) plus linger (1 s), then hidden.
        assert!(animator.visible(&graph, 7.9));
        assert!(!animator.visible(&graph, 8.1));
        assert!(animator.trigger(&graph, AnimationEvent::Respawn, 9.0));
        assert!(!animator.is_dead() && animator.visible(&graph, 9.0));
        assert_eq!(animator.report(&graph, 9.0, 0.0).action, None);
    }

    #[test]
    fn springs_lag_settle_and_are_deterministic() {
        let (skeleton, graph) = graph();
        let mut scratch = AnimationScratch::default();
        let run = |animator: &mut Animator, scratch: &mut AnimationScratch| {
            let mut tips = Vec::new();
            for frame in 0..120u8 {
                let now = f32::from(frame) / 60.0;
                // The unit moves at 6 m/s along +x for a second, then stops.
                let x = now.min(1.0) * 6.0;
                animator.evaluate(
                    &graph,
                    &skeleton,
                    now,
                    0.0,
                    Mat4::from_translation(Vec3::X * x),
                    scratch,
                );
                tips.push(animator.joint_transform(TAIL[1]).expect("tail"));
            }
            tips
        };
        let mut first = Animator::new(0.0);
        let mut second = Animator::new(0.0);
        let a = run(&mut first, &mut scratch);
        let b = run(&mut second, &mut scratch);
        assert_eq!(a, b, "same inputs, same springs");
        // Without springs the tail is rigid; with them it trails while moving.
        let mut rigid = Animator::new(0.0);
        rigid.evaluate(&graph, &skeleton, 0.5, 0.0, Mat4::IDENTITY, &mut scratch);
        let rest_tail = rigid.joint_transform(TAIL[1]).expect("tail");
        assert!(
            !a[30].abs_diff_eq(rest_tail, 1e-3),
            "the tail lags while the unit moves"
        );
        // Every pose stays finite, and a reset returns to the animated pose.
        assert!(a.iter().all(Mat4::is_finite));
        first.reset_springs();
        first.evaluate(&graph, &skeleton, 2.0, 0.0, Mat4::IDENTITY, &mut scratch);
        rigid.evaluate(&graph, &skeleton, 2.0, 0.0, Mat4::IDENTITY, &mut scratch);
        let rest_tail = rigid.joint_transform(TAIL[1]).expect("tail");
        assert!(
            first
                .joint_transform(TAIL[1])
                .expect("tail")
                .abs_diff_eq(rest_tail, 1e-5)
        );
        assert_eq!(graph.spring_joint_count(), 2);
    }

    #[test]
    fn sockets_follow_their_joint() {
        let (skeleton, graph) = graph();
        let mut scratch = AnimationScratch::default();
        let hand = graph.socket("HAND").expect("socket");
        assert_eq!(graph.socket("missing"), None);
        let mut animator = Animator::new(0.0);
        assert_eq!(animator.socket_transform(hand), None, "before evaluation");
        animator.evaluate(&graph, &skeleton, 0.0, 0.0, Mat4::IDENTITY, &mut scratch);
        let socket = animator.socket_transform(hand).expect("evaluated");
        let arm = animator.joint_transform(ARM).expect("arm");
        // Arm at (0.5, 2.0, 0) in the rest pose; the socket hangs 0.5 m below it.
        assert!(arm.w_axis.truncate().distance(Vec3::new(0.5, 2.0, 0.0)) < 1e-4);
        assert!(socket.w_axis.truncate().distance(Vec3::new(0.5, 1.5, 0.0)) < 1e-4);
        // Rotated 90 degrees about Y.
        assert!(socket.x_axis.truncate().distance(Vec3::NEG_Z) < 1e-4);
        assert_eq!(animator.matrices().len(), skeleton.joint_count());
        assert!(
            animator
                .joint_transform(HIPS)
                .is_some_and(|m| m.is_finite())
        );
    }

    #[test]
    fn mismatched_skeletons_and_hostile_clocks_do_not_panic() {
        let (skeleton, graph) = graph();
        let mut scratch = AnimationScratch::default();
        let other = Skeleton::new(
            vec![None],
            vec![JointTransform::IDENTITY],
            vec![Mat4::IDENTITY],
            vec![Mat4::IDENTITY],
            None,
        )
        .expect("one joint");
        let mut animator = Animator::new(f32::NAN);
        animator.trigger(&graph, AnimationEvent::Attack { interval: 1.0 }, 0.0);
        assert_eq!(
            animator
                .evaluate(&graph, &other, 0.0, 0.0, Mat4::IDENTITY, &mut scratch)
                .len(),
            1
        );
        for now in [f32::NAN, f32::INFINITY, -1e9, 1e9] {
            for speed in [f32::NAN, -1.0, 1e9] {
                let matrices =
                    animator.evaluate(&graph, &skeleton, now, speed, Mat4::IDENTITY, &mut scratch);
                assert!(matrices.iter().flatten().flatten().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn legacy_matrices_match_pose_matrices() {
        let skeleton = rig();
        let mut legacy = [[[0.0; 4]; 3]; MAX_JOINTS];
        let mut posed = [[[0.0; 4]; 3]; MAX_JOINTS];
        assert!(skeleton.sample(0.0, 0.0, &mut legacy));
        assert!(pose_matrices(&skeleton, &Pose::rest(&skeleton), &mut posed));
        assert_eq!(legacy[..6], posed[..6]);
        assert!(!pose_matrices(
            &skeleton,
            &Pose::rest(&skeleton),
            &mut posed[..2]
        ));
    }

    #[cfg(feature = "glb")]
    #[test]
    fn shipped_animation_set_binds_to_the_shipped_model() {
        let mesh = crate::load_glb(include_bytes!(
            "../../../content/models/sentinel-skinned.glb"
        ))
        .expect("sample");
        let skeleton = mesh.skin().expect("skin").skeleton();
        let set = AnimationSet::parse(include_str!(
            "../../../content/animations/sentinel-skinned.toml"
        ))
        .expect("shipped set parses");
        let graph = AnimationGraph::bind(&set, skeleton).expect("shipped set binds");
        for state in [
            "attack", "cast1", "cast2", "cast3", "death", "recall", "hit",
        ] {
            assert!(graph.state(state).is_some(), "{state}");
        }
        assert_eq!(graph.spring_joint_count(), 3);
        assert!(graph.socket("spear_tip").is_some());
    }
}
