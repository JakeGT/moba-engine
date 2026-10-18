//! Deterministic input replay format.
//!
//! A replay stores the simulation configuration plus the exact per-tick intents that were
//! applied on the server. Because the simulation is deterministic, replaying those inputs
//! must reproduce the recorded state hash. Replays are intentionally small (the server
//! records post-bot-resolution inputs; snapshots are not stored).
//!
//! Format (little-endian, version 5):
//! `"MOBARPL1" | version u8 | sim_logic u8 | seed u64 | hero_picks[10] | bot_mask u16 | spawn_waves u8 |
//!  content_hash u64 | name_len u8+name | version_len u8+version | has_hash u8 |
//!  expected_hash u64 | frame_count u32 | frames...`
//!
//! Each frame is a `present u16` command mask, one command per present slot, a `purchases
//! u16` mask and one item id per present purchase.

use std::sync::Arc;

use moba_core::FixVec2;
use moba_ecs::Entity;

use crate::{MAX_PLAYERS, PlayerCommand, SimConfig, Simulation, TickInputs};
use moba_game::GameContent;

/// Replay magic/version tag.
const MAGIC: &[u8; 8] = b"MOBARPL1";
/// Current replay format version.
const VERSION: u8 = 5;
/// Simulation logic contract. Bump when tick behaviour changes enough that old inputs no
/// longer reproduce the recorded state.
pub const SIM_LOGIC_VERSION: u8 = 1;
/// Minimum bytes per frame on the wire (`present` bitmask, no commands).
const MIN_FRAME_BYTES: usize = 4;
/// A checkpoint is `tick u32 + state hash u64`.
const MIN_CHECKPOINT_BYTES: usize = 12;

/// Replay encoding/decoding or verification errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// File is too short or a field is truncated.
    Truncated,
    /// Wrong magic/version.
    BadHeader,
    /// Replay layout version is not supported by this build.
    UnsupportedVersion {
        /// Version found in the file.
        found: u8,
        /// Version supported by this build.
        supported: u8,
    },
    /// Replay was recorded with incompatible simulation logic.
    UnsupportedLogicVersion {
        /// Logic version found in the file.
        found: u8,
        /// Logic version supported by this build.
        supported: u8,
    },
    /// Frame or command data is invalid.
    BadData,
    /// The content pack hash does not match the recording.
    ContentMismatch {
        /// Hash recorded when the replay was written.
        expected: u64,
        /// Hash of the supplied content pack.
        actual: u64,
    },
    /// A checkpoint state hash does not match.
    CheckpointMismatch {
        /// Completed tick count of the checkpoint.
        tick: u32,
        /// Hash recorded in the replay.
        expected: u64,
        /// Hash produced during replay.
        actual: u64,
    },
    /// The replayed state hash does not match the recorded hash.
    HashMismatch {
        /// Hash recorded when the replay was written.
        expected: u64,
        /// Hash produced by replaying the inputs.
        actual: u64,
    },
}

impl core::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated replay"),
            Self::BadHeader => f.write_str("bad replay header/version"),
            Self::UnsupportedVersion { found, supported } => {
                write!(
                    f,
                    "unsupported replay version {found} (supported {supported})"
                )
            }
            Self::UnsupportedLogicVersion { found, supported } => write!(
                f,
                "unsupported simulation logic version {found} (supported {supported})"
            ),
            Self::BadData => f.write_str("invalid replay data"),
            Self::ContentMismatch { expected, actual } => {
                write!(
                    f,
                    "replay content mismatch: expected {expected:#018x}, got {actual:#018x}"
                )
            }
            Self::CheckpointMismatch {
                tick,
                expected,
                actual,
            } => write!(
                f,
                "replay diverged at tick {tick}: expected {expected:#018x}, got {actual:#018x}"
            ),
            Self::HashMismatch { expected, actual } => {
                write!(
                    f,
                    "replay hash mismatch: expected {expected:#018x}, got {actual:#018x}"
                )
            }
        }
    }
}

impl std::error::Error for ReplayError {}

/// A recorded match.
#[derive(Clone, Debug)]
pub struct Replay {
    /// Simulation configuration used for the match.
    pub config: SimConfig,
    /// One input frame per simulation tick.
    pub frames: Vec<TickInputs>,
    /// Simulation logic contract used when recording.
    pub sim_logic_version: u8,
    /// State hash recorded at the end of the match, when known.
    pub expected_hash: Option<u64>,
    /// Content pack hash used by the match.
    pub content_hash: u64,
    /// Content pack display name.
    pub content_name: String,
    /// Content pack version.
    pub content_version: String,
    /// Periodic `(completed_tick, state_hash)` checkpoints for first-divergence debugging.
    pub checkpoints: Vec<(u32, u64)>,
}

/// Result of replaying a match.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ReplayResult {
    /// Number of replayed ticks.
    pub ticks: u32,
    /// Final simulation state hash.
    pub state_hash: u64,
    /// Match outcome (winner), if any.
    pub outcome: Option<moba_game::Team>,
}

impl Replay {
    /// Creates an empty replay for `config`.
    #[must_use]
    pub fn new(config: SimConfig) -> Self {
        Self {
            config,
            frames: Vec::new(),
            sim_logic_version: SIM_LOGIC_VERSION,
            expected_hash: None,
            content_hash: GameContent::default().hash(),
            content_name: GameContent::default().manifest.name,
            content_version: GameContent::default().manifest.version,
            checkpoints: Vec::new(),
        }
    }

    /// Records one tick of already-resolved inputs.
    pub fn push(&mut self, inputs: TickInputs) {
        self.frames.push(inputs);
    }

    /// Replays all frames into a fresh simulation and verifies the recorded hash.
    ///
    /// # Errors
    /// Returns [`ReplayError::HashMismatch`] when the result differs from the recording.
    pub fn play(&self) -> Result<ReplayResult, ReplayError> {
        self.play_with_content(&GameContent::default())
    }

    /// Replays with a specific content pack, checking the recorded content hash first.
    ///
    /// # Errors
    /// Returns [`ReplayError::ContentMismatch`] or [`ReplayError::HashMismatch`].
    pub fn play_with_content(&self, content: &GameContent) -> Result<ReplayResult, ReplayError> {
        let actual_content = content.hash();
        if actual_content != self.content_hash {
            return Err(ReplayError::ContentMismatch {
                expected: self.content_hash,
                actual: actual_content,
            });
        }
        let mut simulation = Simulation::new_with_content(self.config, Arc::new(content.clone()));
        let mut checkpoints = self.checkpoints.iter().peekable();
        for (index, frame) in self.frames.iter().enumerate() {
            simulation.tick(frame);
            simulation.clear_events();
            let completed = u32::try_from(index + 1).unwrap_or(u32::MAX);
            while let Some(&&(tick, expected)) = checkpoints.peek() {
                if tick != completed {
                    break;
                }
                let actual = simulation.state_hash();
                if expected != actual {
                    return Err(ReplayError::CheckpointMismatch {
                        tick,
                        expected,
                        actual,
                    });
                }
                checkpoints.next();
            }
        }
        if checkpoints.next().is_some() {
            return Err(ReplayError::BadData);
        }
        let state_hash = simulation.state_hash();
        if let Some(expected) = self.expected_hash {
            if expected != state_hash {
                return Err(ReplayError::HashMismatch {
                    expected,
                    actual: state_hash,
                });
            }
        }
        Ok(ReplayResult {
            ticks: simulation.current_tick().get(),
            state_hash,
            outcome: simulation.outcome(),
        })
    }

    /// Encodes the replay to bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.frames.len() * 16);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(self.sim_logic_version);
        out.extend_from_slice(&self.config.seed.to_le_bytes());
        out.extend_from_slice(&self.config.hero_picks);
        out.extend_from_slice(&self.config.bot_mask.to_le_bytes());
        out.push(u8::from(self.config.spawn_waves));
        out.extend_from_slice(&self.content_hash.to_le_bytes());
        for field in [&self.content_name, &self.content_version] {
            let bytes = field.as_bytes();
            let length = bytes.len().min(255);
            out.push(u8::try_from(length).unwrap_or(u8::MAX));
            out.extend_from_slice(&bytes[..length]);
        }
        out.push(u8::from(self.expected_hash.is_some()));
        out.extend_from_slice(&self.expected_hash.unwrap_or(0).to_le_bytes());
        out.extend_from_slice(
            &u32::try_from(self.frames.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        for frame in &self.frames {
            let mut present = 0u16;
            for (slot, command) in frame.players.iter().enumerate() {
                present |= u16::from(command.is_some()) << slot;
            }
            out.extend_from_slice(&present.to_le_bytes());
            for command in frame.players.iter().flatten() {
                encode_command(command, &mut out);
            }
            let mut purchases = 0u16;
            for (slot, item) in frame.purchases.iter().enumerate() {
                purchases |= u16::from(item.is_some()) << slot;
            }
            out.extend_from_slice(&purchases.to_le_bytes());
            for item in frame.purchases.iter().flatten() {
                out.push(*item);
            }
        }
        out.extend_from_slice(
            &u32::try_from(self.checkpoints.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        for (tick, hash) in &self.checkpoints {
            out.extend_from_slice(&tick.to_le_bytes());
            out.extend_from_slice(&hash.to_le_bytes());
        }
        out
    }

    /// Decodes and validates a replay.
    ///
    /// # Errors
    /// Returns [`ReplayError`] for malformed or truncated input.
    pub fn decode(data: &[u8]) -> Result<Self, ReplayError> {
        let mut reader = Reader::new(data);
        if reader.bytes(8)? != MAGIC {
            return Err(ReplayError::BadHeader);
        }
        let version = reader.u8()?;
        if version != VERSION {
            return Err(ReplayError::UnsupportedVersion {
                found: version,
                supported: VERSION,
            });
        }
        let sim_logic_version = reader.u8()?;
        if sim_logic_version != SIM_LOGIC_VERSION {
            return Err(ReplayError::UnsupportedLogicVersion {
                found: sim_logic_version,
                supported: SIM_LOGIC_VERSION,
            });
        }
        let seed = reader.u64()?;
        let mut hero_picks = [0u8; MAX_PLAYERS];
        hero_picks.copy_from_slice(reader.bytes(MAX_PLAYERS)?);
        let bot_mask = reader.u16()?;
        let spawn_waves = reader.u8()? != 0;
        let content_hash = reader.u64()?;
        let read_field = |reader: &mut Reader<'_>| -> Result<String, ReplayError> {
            let length = usize::from(reader.u8()?);
            Ok(String::from_utf8_lossy(reader.bytes(length)?).into_owned())
        };
        let content_name = read_field(&mut reader)?;
        let content_version = read_field(&mut reader)?;
        let has_hash = reader.u8()? != 0;
        let expected = reader.u64()?;
        let frame_count = usize::try_from(reader.u32()?).map_err(|_| ReplayError::BadData)?;
        if frame_count > reader.remaining() / MIN_FRAME_BYTES {
            return Err(ReplayError::BadData);
        }
        let mut frames = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            let present = reader.u16()?;
            if present & !0x03FF != 0 {
                return Err(ReplayError::BadData);
            }
            let mut inputs = TickInputs::new();
            for slot in 0..MAX_PLAYERS {
                if present & (1 << slot) != 0 {
                    inputs.players[slot] = Some(decode_command(&mut reader)?);
                }
            }
            let purchases = reader.u16()?;
            if purchases & !0x03FF != 0 {
                return Err(ReplayError::BadData);
            }
            for slot in 0..MAX_PLAYERS {
                if purchases & (1 << slot) != 0 {
                    inputs.purchases[slot] = Some(reader.u8()?);
                }
            }
            frames.push(inputs);
        }
        let checkpoint_count = usize::try_from(reader.u32()?).map_err(|_| ReplayError::BadData)?;
        if checkpoint_count > reader.remaining() / MIN_CHECKPOINT_BYTES {
            return Err(ReplayError::BadData);
        }
        let mut checkpoints = Vec::with_capacity(checkpoint_count);
        for _ in 0..checkpoint_count {
            checkpoints.push((reader.u32()?, reader.u64()?));
        }
        if checkpoints.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(ReplayError::BadData);
        }
        if reader.remaining() != 0 {
            return Err(ReplayError::BadData);
        }
        Ok(Self {
            config: SimConfig {
                seed,
                bot_mask,
                hero_picks,
                spawn_waves,
            },
            frames,
            sim_logic_version,
            expected_hash: has_hash.then_some(expected),
            content_hash,
            content_name,
            content_version,
            checkpoints,
        })
    }
}

fn encode_command(command: &PlayerCommand, out: &mut Vec<u8>) {
    match command {
        PlayerCommand::Stop => out.push(0),
        PlayerCommand::MoveTo(position) => {
            out.push(1);
            out.extend_from_slice(&position.x.to_bits().to_le_bytes());
            out.extend_from_slice(&position.y.to_bits().to_le_bytes());
        }
        PlayerCommand::Attack(entity) => {
            out.push(2);
            out.extend_from_slice(&entity.to_bits().to_le_bytes());
        }
        PlayerCommand::CastAbility { slot, target } => {
            out.push(3);
            out.push(*slot);
            out.extend_from_slice(&target.x.to_bits().to_le_bytes());
            out.extend_from_slice(&target.y.to_bits().to_le_bytes());
        }
        PlayerCommand::BuyItem(item) => {
            out.push(4);
            out.push(*item);
        }
        PlayerCommand::Surrender => out.push(5),
    }
}

fn decode_command(reader: &mut Reader<'_>) -> Result<PlayerCommand, ReplayError> {
    Ok(match reader.u8()? {
        0 => PlayerCommand::Stop,
        1 => PlayerCommand::MoveTo(fixed_vec(reader)?),
        2 => PlayerCommand::Attack(Entity::from_bits(reader.u64()?)),
        3 => PlayerCommand::CastAbility {
            slot: reader.u8()?,
            target: fixed_vec(reader)?,
        },
        4 => PlayerCommand::BuyItem(reader.u8()?),
        5 => PlayerCommand::Surrender,
        _ => return Err(ReplayError::BadData),
    })
}

fn fixed_vec(reader: &mut Reader<'_>) -> Result<FixVec2, ReplayError> {
    Ok(FixVec2::new(
        moba_core::Fix::from_bits(reader.i64()?),
        moba_core::Fix::from_bits(reader.i64()?),
    ))
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

    fn bytes(&mut self, length: usize) -> Result<&'a [u8], ReplayError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(ReplayError::Truncated)?;
        let slice = self
            .data
            .get(self.position..end)
            .ok_or(ReplayError::Truncated)?;
        self.position = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ReplayError> {
        Ok(*self.bytes(1)?.first().ok_or(ReplayError::Truncated)?)
    }

    fn u16(&mut self) -> Result<u16, ReplayError> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, ReplayError> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, ReplayError> {
        let bytes = self.bytes(8)?;
        Ok(u64::from_le_bytes(
            bytes.try_into().map_err(|_| ReplayError::Truncated)?,
        ))
    }

    fn i64(&mut self) -> Result<i64, ReplayError> {
        Ok(i64::from_le_bytes(
            self.bytes(8)?
                .try_into()
                .map_err(|_| ReplayError::Truncated)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_mismatch_reports_first_divergence() {
        let config = SimConfig::default();
        let mut simulation = Simulation::new(config);
        let mut replay = Replay::new(config);
        for _ in 0..20 {
            replay.push(TickInputs::default());
            simulation.tick(&TickInputs::default());
        }
        let checkpoint = simulation.state_hash();
        replay.checkpoints.push((20, checkpoint));
        replay.expected_hash = Some(simulation.state_hash());
        assert!(
            Replay::decode(&replay.encode())
                .expect("decode")
                .play()
                .is_ok()
        );
        replay.checkpoints[0].1 ^= 1;
        assert!(matches!(
            Replay::decode(&replay.encode()).expect("decode").play(),
            Err(ReplayError::CheckpointMismatch { tick: 20, .. })
        ));
    }

    #[test]
    fn replay_records_purchases_alongside_movement() {
        let config = SimConfig {
            bot_mask: 0,
            spawn_waves: false,
            ..SimConfig::default()
        };
        let mut sim = Simulation::new(config);
        let mut replay = Replay::new(config);
        let mut frame = TickInputs::new();
        frame.set(0, PlayerCommand::MoveTo(FixVec2::from_ints(30, 128)));
        for _ in 0..240 {
            replay.push(frame);
            sim.tick(&frame);
            sim.clear_events();
        }
        let mut buy = frame;
        buy.set_purchase(0, 0);
        replay.push(buy);
        sim.tick(&buy);
        sim.clear_events();
        for _ in 0..5 {
            replay.push(TickInputs::new());
            sim.tick(&TickInputs::new());
            sim.clear_events();
        }
        assert!(sim.hero_position(0).is_some(), "human hero survives setup");
        replay.expected_hash = Some(sim.state_hash());
        let encoded = replay.encode();
        let decoded = Replay::decode(&encoded).expect("decode");
        assert!(
            decoded
                .frames
                .iter()
                .any(|frame| frame.purchases[0] == Some(0))
        );
        let result = decoded.play().expect("replay verifies");
        assert_eq!(result.state_hash, sim.state_hash());
    }

    #[test]
    fn replay_rejects_unknown_layout_or_logic_versions() {
        let mut encoded = Replay::new(SimConfig::default()).encode();
        encoded[8] = VERSION + 1;
        assert!(matches!(
            Replay::decode(&encoded),
            Err(ReplayError::UnsupportedVersion { .. })
        ));
        let mut encoded = Replay::new(SimConfig::default()).encode();
        encoded[9] = SIM_LOGIC_VERSION + 1;
        assert!(matches!(
            Replay::decode(&encoded),
            Err(ReplayError::UnsupportedLogicVersion { .. })
        ));
    }

    #[test]
    fn replay_enforces_content_hash() {
        let mut content = GameContent::default();
        content.items[0].cost += 1;
        content.validate().expect("custom content");
        let config = SimConfig::default();
        let mut sim = Simulation::new_with_content(config, Arc::new(content.clone()));
        let mut replay = Replay::new(config);
        replay.content_hash = content.hash();
        replay.push(TickInputs::default());
        sim.tick(&TickInputs::default());
        replay.expected_hash = Some(sim.state_hash());
        let encoded = replay.encode();
        let decoded = Replay::decode(&encoded).expect("decode");
        assert!(decoded.play_with_content(&content).is_ok());
        assert!(matches!(
            decoded.play(),
            Err(ReplayError::ContentMismatch { .. })
        ));
    }

    #[test]
    fn decoder_rejects_noise_without_panicking() {
        let mut rng = moba_core::Rng::from_seed(0x5EED_BEEF);
        for length in 0..512usize {
            let mut data = vec![0u8; length];
            rng.fill_bytes(&mut data);
            let _ = Replay::decode(&data);
        }
    }

    #[test]
    fn replay_round_trip_and_hash_verification() {
        let config = SimConfig {
            seed: 0x1234,
            bot_mask: 0,
            spawn_waves: false,
            hero_picks: [0, 1, 2, 3, 4, 4, 3, 2, 1, 0],
        };
        let mut simulation = Simulation::new(config);
        let mut replay = Replay::new(config);
        for tick in 0..300u32 {
            let mut inputs = TickInputs::new();
            if tick < 200 {
                inputs.set(0, PlayerCommand::MoveTo(FixVec2::from_ints(100, 128)));
            }
            if tick == 299 {
                inputs.set(1, PlayerCommand::Surrender);
            } else {
                inputs.set(1, PlayerCommand::Stop);
            }
            replay.push(inputs);
            simulation.tick(&inputs);
        }
        replay.expected_hash = Some(simulation.state_hash());
        let encoded = replay.encode();
        let decoded = Replay::decode(&encoded).expect("decode");
        assert_eq!(decoded.frames.len(), 300);
        let result = decoded.play().expect("replay verifies");
        assert_eq!(result.ticks, 300);
        assert_eq!(result.state_hash, simulation.state_hash());
        assert_eq!(result.outcome, Some(moba_game::Team::Red));

        let mut corrupt = decoded.clone();
        corrupt.expected_hash = Some(corrupt.expected_hash.unwrap_or(0) ^ 0x40);
        let corrupt = Replay::decode(&corrupt.encode()).expect("round trip");
        assert!(matches!(
            corrupt.play(),
            Err(ReplayError::HashMismatch { .. })
        ));
    }
}

#[cfg(test)]
mod allocation_tests {
    use super::*;

    #[test]
    fn oversized_count_fields_are_rejected_without_large_allocations() {
        let mut encoded = Replay::new(SimConfig::default()).encode();
        let frame_offset = encoded.len() - 8;
        let checkpoint_offset = encoded.len() - 4;
        encoded[frame_offset..frame_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            Replay::decode(&encoded),
            Err(ReplayError::BadData)
        ));

        let mut encoded = Replay::new(SimConfig::default()).encode();
        encoded[checkpoint_offset..checkpoint_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            Replay::decode(&encoded),
            Err(ReplayError::BadData)
        ));
    }
}
