//! Replay recording and the per-match results log.

use std::path::{Path, PathBuf};

use moba_sim::Replay;

use super::NetServer;

impl NetServer {
    /// Writes any in-progress match replay. Safe to call multiple times.
    ///
    /// A completed match is written automatically when its nexus falls; this method is for
    /// graceful server shutdown during a live match.
    pub fn finish_recording(&mut self) {
        self.write_recording();
    }

    pub(super) fn write_result_log(&mut self) {
        if self.results_logged {
            return;
        }
        let Some(path) = self.results_log.clone() else {
            return;
        };
        self.results_logged = true;
        let outcome = match self.simulation.outcome() {
            Some(moba_sim::MatchOutcome::Winner(moba_game::Team::Blue)) => "blue",
            Some(moba_sim::MatchOutcome::Winner(moba_game::Team::Red)) => "red",
            Some(moba_sim::MatchOutcome::Draw) => "draw",
            // `MatchOutcome` is `#[non_exhaustive]`; a future outcome logs as unknown.
            Some(_) => "unknown",
            // Only completed matches are logged; keep a defined value regardless.
            None => "unfinished",
        };
        let (blue, red) = self.simulation.score();
        let replay = self
            .config
            .record
            .as_ref()
            .map(|path| replay_path(path, self.match_number));
        let line = result_json(
            self.match_number.wrapping_add(1),
            self.config.sim.seed,
            self.simulation.current_tick().get(),
            outcome,
            blue,
            red,
            &self.content.manifest.name,
            &self.content.manifest.version,
            self.content.hash(),
            self.simulation.state_hash(),
            replay.as_deref(),
        );
        let write = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write as _;
                file.write_all(line.as_bytes())
                    .and_then(|()| file.write_all(b"\n"))
            });
        if let Err(error) = write {
            eprintln!("results log {}: {error}", path.display());
        }
    }

    pub(super) fn write_recording(&mut self) {
        if self.recorded || self.record_frames.is_empty() {
            return;
        }
        if !self.systems.0.is_empty() {
            eprintln!("replay not written: extension systems are not part of replay state");
            self.recorded = true;
            return;
        }
        let Some(path) = self.config.record.clone() else {
            return;
        };
        let state_hash = self.simulation.state_hash();
        let mut replay = Replay::new(self.simulation.config());
        replay.frames = std::mem::take(&mut self.record_frames);
        replay.sim_logic_version = moba_sim::SIM_LOGIC_VERSION;
        replay.expected_hash = Some(state_hash);
        replay.content_hash = self.content.hash();
        replay.content_name.clone_from(&self.content.manifest.name);
        replay
            .content_version
            .clone_from(&self.content.manifest.version);
        replay.checkpoints = std::mem::take(&mut self.record_checkpoints);
        let target = replay_path(&path, self.match_number);
        match std::fs::write(&target, replay.encode()) {
            Ok(()) => eprintln!(
                "replay: wrote {} ticks to {} ({state_hash:#018x})",
                replay.frames.len(),
                target.display()
            ),
            Err(error) => eprintln!("replay: cannot write {}: {error}", target.display()),
        }
        self.recorded = true;
    }
}

fn json_string(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if character < ' ' => {
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

#[allow(clippy::too_many_arguments)] // One flat record keeps call sites explicit.
pub(super) fn result_json(
    match_number: u32,
    seed: u64,
    ticks: u32,
    outcome: &str,
    blue_score: u32,
    red_score: u32,
    content_name: &str,
    content_version: &str,
    content_hash: u64,
    state_hash: u64,
    replay: Option<&Path>,
) -> String {
    let replay = replay.map_or_else(
        || String::from("null"),
        |path| json_string(&path.to_string_lossy()),
    );
    format!(
        "{{\"match\":{match_number},\"seed\":{seed},\"ticks\":{ticks},\"outcome\":{},\"blue_score\":{blue_score},\"red_score\":{red_score},\"content\":{},\"content_version\":{},\"content_hash\":\"{content_hash:#018x}\",\"state_hash\":\"{state_hash:#018x}\",\"replay\":{replay}}}",
        json_string(outcome),
        json_string(content_name),
        json_string(content_version)
    )
}

fn replay_path(base: &Path, match_number: u32) -> PathBuf {
    if match_number == 0 {
        return base.to_path_buf();
    }
    let stem = base
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("match");
    base.with_file_name(format!("{stem}.match{match_number}.mbr"))
}
