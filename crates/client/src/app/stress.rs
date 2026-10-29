//! `MOBA_STRESS_MINIONS=<n>`: the R4 minion stress scene (R4b, perf/0018).
//!
//! The local preview draws `n` extra client-only VAT minions (at most 512) on a grid around
//! the map centre: half run in circles, half stand and attack every 1.2 s, each with its own
//! [`VatPlayer`], through exactly the per-minion path a match uses (player → pose →
//! instance). It measures the CPU time of that loop per frame and prints the mean and p95 at
//! exit, so the animation CPU budget is checked in the real client. Nothing here touches the
//! simulation. Without a VAT asset (or with `MOBA_VAT=off`) the minions are cubes.

use std::time::Instant;

use moba_assets::VatPlayer;
use moba_core::math::{Vec3, Vec4};
use moba_render::UnitInstance;
use moba_sim::UnitKind;

use crate::app::minions::MinionLibrary;

/// Environment variable naming the stress minion count.
pub(crate) const STRESS_ENV: &str = "MOBA_STRESS_MINIONS";
/// Most stress minions.
pub(crate) const MAX_STRESS_MINIONS: usize = 512;
/// Seconds between a standing minion's attacks.
const ATTACK_PERIOD: f32 = 1.2;
/// Grid spacing, metres.
const SPACING: f32 = 3.0;
/// Radius of a running minion's circle, metres, and its speed, m/s.
const CIRCLE: f32 = 1.2;
const RUN_SPEED: f32 = 4.0;
/// Frame timings kept for the report.
const MAX_SAMPLES: usize = 20_000;

/// The stress scene's minions and timings.
#[derive(Debug)]
pub(crate) struct StressScene {
    players: Vec<VatPlayer>,
    count: usize,
    samples: Vec<f32>,
}

impl StressScene {
    /// From [`STRESS_ENV`], or `None` when unset or zero.
    pub(crate) fn from_env() -> Option<Self> {
        let count: usize = std::env::var(STRESS_ENV).ok()?.trim().parse().ok()?;
        (count > 0).then(|| Self::new(count.min(MAX_STRESS_MINIONS)))
    }

    pub(crate) fn new(count: usize) -> Self {
        Self {
            players: Vec::with_capacity(count),
            count,
            samples: Vec::with_capacity(MAX_SAMPLES),
        }
    }

    /// Minions drawn per frame.
    pub(crate) fn count(&self) -> usize {
        self.count
    }

    /// Pushes every stress minion for render time `now` (seconds) and records the CPU time.
    /// Never allocates after the first frame (the players are created once).
    pub(crate) fn push(
        &mut self,
        units: &mut Vec<UnitInstance>,
        library: &MinionLibrary,
        now: f32,
    ) {
        let started = Instant::now();
        let visual = library.visual(UnitKind::MeleeMinion).copied();
        let quality = library.quality();
        if self.players.is_empty()
            && let Some(visual) = visual.as_ref()
        {
            for index in 0..self.count {
                self.players
                    .push(VatPlayer::new(&visual.animation, index as f32 * 0.173));
            }
        }
        let side = (self.count as f32).sqrt().ceil().max(1.0) as usize;
        let offset = (side as f32 - 1.0) * SPACING * 0.5;
        for index in 0..self.count {
            if units.len() >= units.capacity() {
                break;
            }
            let (row, column) = (index / side, index % side);
            let centre = Vec3::new(
                column as f32 * SPACING - offset,
                0.0,
                row as f32 * SPACING - offset,
            );
            let team = if index % 2 == 0 {
                Vec4::new(0.16, 0.36, 0.78, 1.0)
            } else {
                Vec4::new(0.78, 0.2, 0.16, 1.0)
            };
            let running = index % 2 == 0;
            let phase = index as f32 * 0.37;
            let (ground, facing, speed) = if running {
                let angle = (now * RUN_SPEED / CIRCLE + phase) % core::f32::consts::TAU;
                let ground = centre + Vec3::new(angle.cos(), 0.0, angle.sin()) * CIRCLE;
                // Moving along the circle: the tangent, in the map's x/z plane.
                (ground, angle + core::f32::consts::FRAC_PI_2, RUN_SPEED)
            } else {
                (centre, phase, 0.0)
            };
            match (visual.as_ref(), self.players.get_mut(index)) {
                (Some(visual), Some(player)) => {
                    if !running {
                        // The current swing until 0.4 s after its hit, then the next one.
                        let local = now + phase;
                        let hit = ((local - 0.4) / ATTACK_PERIOD).floor() * ATTACK_PERIOD
                            + ATTACK_PERIOD
                            - phase;
                        player.attack(&visual.animation, hit, ATTACK_PERIOD);
                    }
                    let pose = player.pose(&visual.animation, now, speed, quality);
                    units.push(
                        UnitInstance::new(visual.transform(ground, facing), team)
                            .with_mesh(visual.mesh)
                            .with_vat(pose),
                    );
                }
                _ => units
                    .push(UnitInstance::at(ground, 2.2, team).with_mesh(moba_render::MeshId::CUBE)),
            }
        }
        if self.samples.len() < MAX_SAMPLES {
            self.samples
                .push(started.elapsed().as_secs_f32() * 1_000_000.0);
        }
    }

    /// The mean and p95 of the recorded per-frame CPU times, microseconds.
    pub(crate) fn summary(&self) -> Option<(f32, f32, usize)> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(f32::total_cmp);
        let mean = sorted.iter().sum::<f32>() / sorted.len() as f32;
        let p95 = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
        Some((mean, p95, sorted.len()))
    }

    /// Prints [`Self::summary`] (at exit).
    pub(crate) fn report(&self) {
        if let Some((mean, p95, frames)) = self.summary() {
            eprintln!(
                "[stress] {} stress minions: animation + instance CPU {mean:.1} us/frame mean, \
                 {p95:.1} us p95 over {frames} frames",
                self.count
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StressScene;
    use crate::app::minions::{MinionLibrary, MinionVisual};
    use moba_assets::{VatAnimation, VatDescriptor, VatQuality};
    use moba_core::math::Mat4;
    use moba_render::MeshId;

    #[test]
    fn two_hundred_stress_minions_animate_without_allocating() {
        let descriptor = VatDescriptor::parse(include_str!("../../../../content/vat/minion.toml"))
            .expect("descriptor");
        let visual = MinionVisual {
            mesh: MeshId::CUBE,
            normalize: Mat4::IDENTITY,
            animation: VatAnimation::bind(&descriptor),
        };
        let mut library = MinionLibrary::new(std::path::PathBuf::new(), true, VatQuality::FULL);
        library.set_visuals(Some(visual), Some(visual));
        let mut stress = StressScene::new(200);
        let mut units = Vec::with_capacity(256);
        stress.push(&mut units, &library, 0.0);
        let ((), allocations) = crate::app::allocation_probe::probe(|| {
            for frame in 1..300 {
                units.clear();
                stress.push(&mut units, &library, frame as f32 / 60.0);
            }
        });
        assert_eq!(allocations, 0);
        assert_eq!(units.len(), 200);
        // Standing minions (odd indices) blend their attack clip in at some point.
        let mut swung = false;
        for frame in 300..400 {
            units.clear();
            stress.push(&mut units, &library, frame as f32 / 60.0);
            swung |= units
                .iter()
                .skip(1)
                .step_by(2)
                .any(|unit| unit.vat.blend > 0.5);
        }
        assert!(swung, "standing minions swing");
        assert!(stress.summary().is_some());
    }
}
