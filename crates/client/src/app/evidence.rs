//! Opt-in evidence helpers for animation runs (R4b exit criterion). Dev-only switches; none
//! is read per frame and none allocates per frame (a line is written only on a change).
//!
//! - `MOBA_ANIM_LOG=<file>`: every animated hero's action state, dominant locomotion clip and
//!   dead flag, one line each time one of them changes, with the render tick.
//! - `MOBA_CAPTURE_SERIES=<prefix>` with `MOBA_CAPTURE_SERIES_TICKS=<t1,t2,...>` (network
//!   mode): capture `<prefix>-<tick>.ppm` once the render tick reaches each listed tick,
//!   without pinning or exiting.
//! - `MOBA_SPECTATE_FOLLOW=<team>:<slot>` (with `MOBA_SPECTATE=1`): the spectator camera
//!   follows that hero at `MOBA_CAMERA_DISTANCE` (default 46 m).

use std::io::Write as _;
use std::path::PathBuf;

/// Environment variable naming the animation log file.
pub(crate) const ANIM_LOG_ENV: &str = "MOBA_ANIM_LOG";
/// Environment variables of the capture series.
pub(crate) const CAPTURE_SERIES_ENV: &str = "MOBA_CAPTURE_SERIES";
pub(crate) const CAPTURE_SERIES_TICKS_ENV: &str = "MOBA_CAPTURE_SERIES_TICKS";
/// Environment variable of the spectator follow target.
pub(crate) const SPECTATE_FOLLOW_ENV: &str = "MOBA_SPECTATE_FOLLOW";
/// Heroes the log tracks (the driver's animator slots).
const LOGGED: usize = crate::app::animation::MAX_ANIMATED;

/// Captures at listed render ticks.
#[derive(Debug)]
pub(crate) struct CaptureSeries {
    prefix: PathBuf,
    ticks: Vec<u32>,
    next: usize,
}

impl CaptureSeries {
    /// From [`CAPTURE_SERIES_ENV`] and [`CAPTURE_SERIES_TICKS_ENV`]; `None` unless both are
    /// set.
    pub(crate) fn from_env() -> Option<Self> {
        let prefix = std::env::var_os(CAPTURE_SERIES_ENV).filter(|value| !value.is_empty())?;
        let mut ticks: Vec<u32> = std::env::var(CAPTURE_SERIES_TICKS_ENV)
            .ok()?
            .split(',')
            .filter_map(|tick| tick.trim().parse().ok())
            .collect();
        ticks.sort_unstable();
        ticks.dedup();
        (!ticks.is_empty()).then(|| Self {
            prefix: PathBuf::from(prefix),
            ticks,
            next: 0,
        })
    }

    /// The path to capture this frame to, once per listed tick the render tick has reached.
    /// Allocates only when it returns a path.
    pub(crate) fn due(&mut self, render_tick: f32) -> Option<PathBuf> {
        let tick = *self.ticks.get(self.next)?;
        if render_tick < tick as f32 {
            return None;
        }
        // Skip ticks the view jumped past (a stall), capturing the latest one.
        while self
            .ticks
            .get(self.next + 1)
            .is_some_and(|&later| render_tick >= later as f32)
        {
            self.next += 1;
        }
        let tick = self.ticks[self.next];
        self.next += 1;
        let mut name = self.prefix.clone().into_os_string();
        name.push(format!("-{tick:05}.ppm"));
        Some(PathBuf::from(name))
    }
}

/// `<team>:<slot>` from [`SPECTATE_FOLLOW_ENV`].
pub(crate) fn spectate_follow_from_env() -> Option<(u8, u8)> {
    let value = std::env::var(SPECTATE_FOLLOW_ENV).ok()?;
    let (team, slot) = value.split_once(':')?;
    Some((team.trim().parse().ok()?, slot.trim().parse().ok()?))
}

/// One logged hero's last line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Logged {
    net_id: u16,
    action: Option<u8>,
    locomotion: usize,
    dead: bool,
}

/// The animation transition log.
pub(crate) struct AnimLog {
    out: std::io::BufWriter<std::fs::File>,
    last: [Option<Logged>; LOGGED],
}

impl std::fmt::Debug for AnimLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnimLog").finish_non_exhaustive()
    }
}

impl AnimLog {
    /// Opens [`ANIM_LOG_ENV`]'s file; `None` (logged) if unset or it cannot be created.
    pub(crate) fn from_env() -> Option<Self> {
        let path = std::env::var_os(ANIM_LOG_ENV).filter(|value| !value.is_empty())?;
        match std::fs::File::create(&path) {
            Ok(file) => {
                let mut out = std::io::BufWriter::new(file);
                let _ = writeln!(out, "# tick net_id action weight clip_time locomotion dead");
                Some(Self {
                    out,
                    last: [None; LOGGED],
                })
            }
            Err(error) => {
                eprintln!("[anim] {ANIM_LOG_ENV}: cannot create the log: {error}");
                None
            }
        }
    }

    /// Records hero `net_id`'s state at render time `now` if it changed since its last line.
    pub(crate) fn record(
        &mut self,
        net_id: u16,
        now: f32,
        tick_rate: f32,
        action: Option<(u8, &str, f32, f32)>,
        locomotion: (usize, &str),
        dead: bool,
    ) {
        let index = self
            .last
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.net_id == net_id))
            .or_else(|| self.last.iter().position(Option::is_none));
        let Some(last) = index.and_then(|index| self.last.get_mut(index)) else {
            return;
        };
        let entry = Logged {
            net_id,
            action: action.map(|(id, ..)| id),
            locomotion: locomotion.0,
            dead,
        };
        if *last == Some(entry) {
            return;
        }
        *last = Some(entry);
        let (name, weight, time) = action.map_or(("-", 0.0, 0.0), |(_, name, weight, time)| {
            (name, weight, time)
        });
        let _ = writeln!(
            self.out,
            "{:.1} {net_id} {name} {weight:.2} {time:.2} {} {}",
            now * tick_rate,
            locomotion.1,
            u8::from(dead)
        );
    }
}

impl Drop for AnimLog {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::CaptureSeries;
    use std::path::PathBuf;

    #[test]
    fn a_series_captures_each_tick_once_and_skips_ticks_jumped_past() {
        let mut series = CaptureSeries {
            prefix: PathBuf::from("shot"),
            ticks: vec![10, 20, 30, 40],
            next: 0,
        };
        assert_eq!(series.due(9.5), None);
        assert_eq!(series.due(10.0), Some(PathBuf::from("shot-00010.ppm")));
        assert_eq!(series.due(11.0), None);
        // A stall jumps past 20 and 30: one capture, at the latest.
        assert_eq!(series.due(33.0), Some(PathBuf::from("shot-00030.ppm")));
        assert_eq!(series.due(41.0), Some(PathBuf::from("shot-00040.ppm")));
        assert_eq!(series.due(99.0), None);
    }
}
