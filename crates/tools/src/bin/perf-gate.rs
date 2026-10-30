//! Frame-log perf gate (R8, ADR-0018 §6): checks a client frame log (`MOBA_FRAME_LOG`) of
//! the teamfight replay against the roadmap's §7 GPU budget for its tier, the frame-pacing
//! rule (p99 within 1.5× p50), the load-time-only contract (no mid-match uploads or pipeline
//! builds) and a committed baseline (`tests/perf/baseline.toml`), and exits non-zero on any
//! breach. The nightly perf workflow runs it on the self-hosted GPU runner; it runs the same
//! way on a developer machine.
//!
//! ```text
//! perf-gate [--baseline FILE] [--max-regression FRACTION] [--pacing RATIO] [--warm-cache]
//!           <frame-log>...
//! ```
//!
//! Every log must also have dropped no frames (`frames_dropped=0`: the frame log had room for
//! the whole run) and kept its VRAM peak inside the tier's budget.
//!
//! Each frame log names its tier (`quality=`) and GPU (`gpu=`). The baseline names the GPU it
//! was measured on (top-level `gpu = "..."`) and holds one `[tier]` table per tier, each with
//! exactly `gpu_total_p95_ms` and `render_p95_ms`; unknown tables or keys are an error. With
//! `--baseline`, a log whose tier has no table, or that ran on another GPU, fails: a baseline
//! only means something on the GPU that recorded it, so a new runner records its own
//! (`BASELINE= scripts/perf-replay.sh`). Without `--baseline` only the budgets are checked.
//! `--warm-cache` also requires `pipeline_cache=warm` (every pipeline came from the
//! persistent cache: the run had no compile hitch at start either).
#![allow(clippy::print_stdout)] // The report is the tool's output.

use std::process::ExitCode;

/// The roadmap's §7 GPU budgets (ms, p95), per tier.
const GPU_BUDGET_MS: [(&str, f64); 4] = [
    ("low", 14.0),
    ("medium", 6.0),
    ("high", 6.0),
    ("ultra", 7.5),
];

/// Allowed growth over the baseline before the gate fails.
const DEFAULT_MAX_REGRESSION: f64 = 0.25;
/// Frame pacing: p99 must stay within this multiple of p50.
const DEFAULT_PACING: f64 = 1.5;
/// Baselines under this many milliseconds compare with this floor, so a 0.05 ms pass does
/// not fail on timer noise.
const NOISE_FLOOR_MS: f64 = 0.05;

/// One frame-log line's `key=value` fields.
struct FrameLog {
    fields: Vec<(String, String)>,
}

impl FrameLog {
    fn parse(text: &str) -> Self {
        let line = text
            .lines()
            .rev()
            .find(|line| line.contains("frames="))
            .unwrap_or_default();
        let mut fields = Vec::new();
        let mut rest = line.trim();
        while !rest.is_empty() {
            let Some((key, after)) = rest.split_once('=') else {
                break;
            };
            let (value, next) = if let Some(quoted) = after.strip_prefix('"') {
                quoted.split_once('"').unwrap_or((quoted, ""))
            } else {
                after.split_once(' ').unwrap_or((after, ""))
            };
            fields.push((key.trim().to_owned(), value.to_owned()));
            rest = next.trim_start();
        }
        Self { fields }
    }

    /// The last value of `key` (a later field overrides an earlier one).
    fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    fn number(&self, key: &str) -> Option<f64> {
        self.get(key)?
            .parse()
            .ok()
            .filter(|value: &f64| value.is_finite())
    }
}

/// `line` up to its first `#` outside a quoted string.
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (at, character) in line.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..at],
            _ => {}
        }
    }
    line
}

/// The keys every baseline tier table has.
const BASELINE_KEYS: [&str; 2] = ["gpu_total_p95_ms", "render_p95_ms"];

/// The baseline file: the GPU it was measured on, then `[tier]` tables of `key = number`
/// lines (the TOML subset it uses).
#[derive(Debug, Default, PartialEq)]
struct Baseline {
    gpu: String,
    tables: Vec<(String, Vec<(String, f64)>)>,
}

impl Baseline {
    fn parse(text: &str) -> Result<Self, String> {
        let mut gpu = None;
        let mut tables: Vec<(String, Vec<(String, f64)>)> = Vec::new();
        for (number, raw) in text.lines().enumerate() {
            let at = |message: String| format!("line {}: {message}", number + 1);
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
            {
                let name = name.trim();
                if !GPU_BUDGET_MS.iter().any(|(tier, _)| *tier == name) {
                    return Err(at(format!("unknown tier table [{name}]")));
                }
                if tables.iter().any(|(table, _)| table == name) {
                    return Err(at(format!("[{name}] appears twice")));
                }
                tables.push((name.to_owned(), Vec::new()));
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| at("expected `key = value`".to_owned()))?;
            let (key, value) = (key.trim(), value.trim());
            let Some((table, entries)) = tables.last_mut() else {
                if key != "gpu" {
                    return Err(at(format!("unknown top-level key {key:?} (only `gpu`)")));
                }
                let name = value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| at("gpu must be a non-empty quoted string".to_owned()))?;
                gpu = Some(name.to_owned());
                continue;
            };
            if !BASELINE_KEYS.contains(&key) {
                return Err(at(format!(
                    "unknown key {key:?} in [{table}] (expected {})",
                    BASELINE_KEYS.join(", ")
                )));
            }
            if entries.iter().any(|(name, _)| name == key) {
                return Err(at(format!("{key} appears twice in [{table}]")));
            }
            let value: f64 = value
                .parse()
                .ok()
                .filter(|value: &f64| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| at(format!("{value:?} is not a non-negative number")))?;
            entries.push((key.to_owned(), value));
        }
        for (table, entries) in &tables {
            for key in BASELINE_KEYS {
                if !entries.iter().any(|(name, _)| name == key) {
                    return Err(format!("[{table}] has no {key}"));
                }
            }
        }
        let gpu = gpu.ok_or_else(|| "the baseline names no gpu (gpu = \"...\")".to_owned())?;
        Ok(Self { gpu, tables })
    }

    fn has_table(&self, table: &str) -> bool {
        self.tables.iter().any(|(name, _)| name == table)
    }

    fn get(&self, table: &str, key: &str) -> Option<f64> {
        self.tables
            .iter()
            .find(|(name, _)| name == table)?
            .1
            .iter()
            .find(|(name, _)| name == key)
            .map(|&(_, value)| value)
    }
}

/// Limits the gate applies.
#[derive(Clone, Copy, Debug)]
struct Limits {
    max_regression: f64,
    pacing: f64,
    warm_cache: bool,
}

/// Every breach of `log` (empty: it passes), plus the lines describing what was checked.
fn check(
    log: &FrameLog,
    baseline: Option<&Baseline>,
    limits: Limits,
) -> (Vec<String>, Vec<String>) {
    let mut failures = Vec::new();
    let mut notes = Vec::new();
    let tier = log
        .get("quality")
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let Some(&(_, budget)) = GPU_BUDGET_MS.iter().find(|(name, _)| *name == tier) else {
        failures.push(format!(
            "the frame log names no known tier (quality={tier:?})"
        ));
        return (failures, notes);
    };
    let need = |key: &str, failures: &mut Vec<String>| {
        let value = log.number(key);
        if value.is_none() {
            failures.push(format!("{tier}: the frame log has no {key}"));
        }
        value
    };
    // A baseline only applies on its own GPU, and must cover every tier it is asked about.
    let baseline = match baseline {
        Some(reference) => {
            let gpu = log.get("gpu").unwrap_or("not reported");
            if gpu != reference.gpu {
                failures.push(format!(
                    "{tier}: the baseline is for {:?}, this run is on {gpu:?}: record a baseline \
                     on this GPU (BASELINE= scripts/perf-replay.sh)",
                    reference.gpu
                ));
                None
            } else if !reference.has_table(&tier) {
                failures.push(format!("{tier}: the baseline has no [{tier}] table"));
                None
            } else {
                Some(reference)
            }
        }
        None => None,
    };
    // §7 GPU budget.
    if let Some(gpu_p95) = need("gpu_total_p95_ms", &mut failures) {
        notes.push(format!(
            "{tier}: gpu_total_p95 {gpu_p95:.3} ms (budget {budget} ms)"
        ));
        if gpu_p95 > budget {
            failures.push(format!(
                "{tier}: GPU p95 {gpu_p95:.3} ms is over the §7 budget of {budget} ms"
            ));
        }
        if let Some(reference) =
            baseline.and_then(|baseline| baseline.get(&tier, "gpu_total_p95_ms"))
        {
            let allowed = reference.max(NOISE_FLOOR_MS) * (1.0 + limits.max_regression);
            notes.push(format!(
                "{tier}: baseline gpu_total_p95 {reference:.3} ms, allowed {allowed:.3} ms"
            ));
            if gpu_p95 > allowed {
                failures.push(format!(
                    "{tier}: GPU p95 {gpu_p95:.3} ms regressed past {allowed:.3} ms (baseline \
                     {reference:.3} ms + {:.0} %)",
                    limits.max_regression * 100.0
                ));
            }
        }
    }
    if let (Some(render), Some(reference)) = (
        log.number("render_p95_ms"),
        baseline.and_then(|baseline| baseline.get(&tier, "render_p95_ms")),
    ) {
        let allowed = reference.max(NOISE_FLOOR_MS) * (1.0 + limits.max_regression);
        if render > allowed {
            failures.push(format!(
                "{tier}: CPU render p95 {render:.3} ms regressed past {allowed:.3} ms"
            ));
        }
    }
    // Frame pacing.
    if let (Some(p50), Some(p99)) = (need("p50_ms", &mut failures), need("p99_ms", &mut failures)) {
        let ratio = if p50 > 0.0 { p99 / p50 } else { f64::INFINITY };
        notes.push(format!(
            "{tier}: frame p50 {p50:.3} ms, p99 {p99:.3} ms ({ratio:.2}x)"
        ));
        if ratio > limits.pacing {
            failures.push(format!(
                "{tier}: frame p99 {p99:.3} ms is {ratio:.2}x p50 {p50:.3} ms (limit {:.2}x)",
                limits.pacing
            ));
        }
    }
    check_run(log, &tier, limits, &mut failures, &mut notes);
    (failures, notes)
}

/// The run-integrity checks: a warm cache (when asked), no dropped frames, VRAM within the
/// budget, nothing loaded mid-match.
fn check_run(
    log: &FrameLog,
    tier: &str,
    limits: Limits,
    failures: &mut Vec<String>,
    notes: &mut Vec<String>,
) {
    if limits.warm_cache && log.get("pipeline_cache") != Some("warm") {
        failures.push(format!(
            "{tier}: the pipeline cache was {} (warm required)",
            log.get("pipeline_cache").unwrap_or("not reported")
        ));
    }
    // The whole run is in the log.
    match log.number("frames_dropped") {
        Some(dropped) if dropped > 0.0 => failures.push(format!(
            "{tier}: {dropped} frames were dropped from the frame log (raise \
             MOBA_FRAME_LOG_FRAMES)"
        )),
        Some(_) => {}
        None => failures.push(format!("{tier}: the frame log has no frames_dropped")),
    }
    // VRAM inside the tier's budget.
    let mut need = |key: &str| {
        let value = log.number(key);
        if value.is_none() {
            failures.push(format!("{tier}: the frame log has no {key}"));
        }
        value
    };
    if let (Some(peak), Some(budget)) = (need("vram_peak_mb"), need("vram_budget_mb")) {
        notes.push(format!(
            "{tier}: vram peak {peak:.1} MB (budget {budget:.0} MB)"
        ));
        if peak > budget {
            failures.push(format!(
                "{tier}: VRAM peak {peak:.1} MB is over the tier's {budget:.0} MB budget"
            ));
        }
    }
    // Load-time-only assets and pipelines.
    for key in ["locked_uploads", "locked_pipelines"] {
        match log.number(key) {
            Some(count) if count > 0.0 => {
                failures.push(format!(
                    "{tier}: {count} {key} (assets must load before the match)"
                ));
            }
            Some(_) => {}
            None => failures.push(format!("{tier}: the frame log has no {key}")),
        }
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: perf-gate [--baseline FILE] [--max-regression FRACTION] [--pacing RATIO] \
         [--warm-cache] <frame-log>..."
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut baseline_path = None;
    let mut limits = Limits {
        max_regression: DEFAULT_MAX_REGRESSION,
        pacing: DEFAULT_PACING,
        warm_cache: false,
    };
    let mut logs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut number = || {
            args.next()
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value > 0.0)
        };
        match arg.as_str() {
            "--baseline" => match args.next() {
                Some(path) => baseline_path = Some(path),
                None => return usage(),
            },
            "--max-regression" => match number() {
                Some(value) => limits.max_regression = value,
                None => return usage(),
            },
            "--warm-cache" => limits.warm_cache = true,
            "--pacing" => match number() {
                Some(value) => limits.pacing = value,
                None => return usage(),
            },
            _ if arg.starts_with("--") => return usage(),
            _ => logs.push(arg),
        }
    }
    if logs.is_empty() {
        return usage();
    }
    let baseline = match baseline_path.as_deref().map(std::fs::read_to_string) {
        None => None,
        Some(Ok(text)) => match Baseline::parse(&text) {
            Ok(baseline) => Some(baseline),
            Err(error) => {
                eprintln!("perf-gate: baseline: {error}");
                return ExitCode::from(2);
            }
        },
        Some(Err(error)) => {
            eprintln!("perf-gate: baseline: {error}");
            return ExitCode::from(2);
        }
    };
    let mut failed = false;
    for path in &logs {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("perf-gate: {path}: {error}");
                failed = true;
                continue;
            }
        };
        let (failures, notes) = check(&FrameLog::parse(&text), baseline.as_ref(), limits);
        for note in &notes {
            println!("{path}: {note}");
        }
        for failure in &failures {
            println!("{path}: FAIL {failure}");
        }
        if failures.is_empty() {
            println!("{path}: ok");
        }
        failed |= !failures.is_empty();
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        max_regression: 0.25,
        pacing: 1.5,
        warm_cache: false,
    };

    fn log(fields: &str) -> FrameLog {
        FrameLog::parse(&format!(
            "frames=100 frames_dropped=0 p50_ms=4.0 p95_ms=4.4 p99_ms=5.0 render_p95_ms=0.2 \
             vram_budget_mb=6144 vram_peak_mb=269.1 {fields} locked_uploads=0 \
             locked_pipelines=0 quality=ultra hdr=X gpu=\"NVIDIA GeForce RTX 5070 Ti\"\n"
        ))
    }

    #[test]
    fn frame_logs_parse_quoted_values() {
        let log = log("gpu_total_p95_ms=0.569");
        assert_eq!(log.number("gpu_total_p95_ms"), Some(0.569));
        assert_eq!(log.get("gpu"), Some("NVIDIA GeForce RTX 5070 Ti"));
        assert_eq!(log.get("quality"), Some("ultra"));
        assert_eq!(log.get("missing"), None);
    }

    #[test]
    fn a_log_within_budget_and_baseline_passes() {
        let baseline = Baseline::parse(
            "# reference\ngpu = \"NVIDIA GeForce RTX 5070 Ti\" # the runner\n[ultra]\n\
             gpu_total_p95_ms = 0.57 # RTX\nrender_p95_ms = 0.2\n",
        )
        .expect("baseline");
        assert_eq!(baseline.gpu, "NVIDIA GeForce RTX 5070 Ti");
        assert_eq!(baseline.get("ultra", "gpu_total_p95_ms"), Some(0.57));
        let (failures, notes) = check(&log("gpu_total_p95_ms=0.6"), Some(&baseline), LIMITS);
        assert!(failures.is_empty(), "{failures:?}");
        assert!(notes.iter().any(|note| note.contains("budget 7.5 ms")));
    }

    #[test]
    fn breaches_fail_with_a_reason() {
        let baseline = Baseline::parse(&baseline_for("0.5")).expect("baseline");
        let (failures, _) = check(&log("gpu_total_p95_ms=0.7"), Some(&baseline), LIMITS);
        assert!(
            failures.iter().any(|failure| failure.contains("regressed")),
            "{failures:?}"
        );
        let (failures, _) = check(&log("gpu_total_p95_ms=8.0"), None, LIMITS);
        assert!(
            failures.iter().any(|failure| failure.contains("§7 budget")),
            "{failures:?}"
        );
        let jerky = FrameLog::parse(
            "frames=9 frames_dropped=0 p50_ms=4.0 p99_ms=7.0 gpu_total_p95_ms=1 \
             vram_peak_mb=1 vram_budget_mb=2 locked_uploads=0 locked_pipelines=2 quality=high",
        );
        let (failures, _) = check(&jerky, None, LIMITS);
        assert!(
            failures.iter().any(|failure| failure.contains("1.75x")),
            "{failures:?}"
        );
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("locked_pipelines"))
        );
        let warm = Limits {
            warm_cache: true,
            ..LIMITS
        };
        let (failures, _) = check(&log("gpu_total_p95_ms=0.5 pipeline_cache=cold"), None, warm);
        assert!(
            failures.iter().any(|failure| failure.contains("cold")),
            "{failures:?}"
        );
        let (failures, _) = check(&log("gpu_total_p95_ms=0.5 pipeline_cache=warm"), None, warm);
        assert!(failures.is_empty(), "{failures:?}");
        let (failures, _) = check(&FrameLog::parse("frames=1 quality=epic"), None, LIMITS);
        assert_eq!(failures.len(), 1, "an unknown tier stops the check");
        assert!(Baseline::parse("gpu = \"x\"\n[ultra]\ngpu_total_p95_ms = fast\n").is_err());
        let (failures, _) = check(&log("gpu_total_p95_ms=0.5 frames_dropped=3"), None, LIMITS);
        assert!(
            failures.iter().any(|failure| failure.contains("dropped")),
            "{failures:?}"
        );
        let (failures, _) = check(&log("gpu_total_p95_ms=0.5 vram_peak_mb=7000"), None, LIMITS);
        assert!(
            failures.iter().any(|failure| failure.contains("VRAM peak")),
            "{failures:?}"
        );
    }

    fn baseline_for(gpu_total_p95_ms: &str) -> String {
        format!(
            "gpu = \"NVIDIA GeForce RTX 5070 Ti\"\n[ultra]\ngpu_total_p95_ms = {gpu_total_p95_ms}\n\
             render_p95_ms = 0.2\n"
        )
    }

    /// R8 review: a baseline is strict. It must name its GPU, have both keys in every table,
    /// and no unknown table or key; a log on another GPU, or of a tier the baseline has no
    /// table for, fails instead of silently skipping the regression check.
    #[test]
    fn baselines_are_strict_and_bound_to_their_gpu() {
        for (text, why) in [
            (
                "[ultra]\ngpu_total_p95_ms = 1\nrender_p95_ms = 1\n",
                "no gpu",
            ),
            (
                "gpu = \"x\"\n[ultra]\ngpu_total_p95_ms = 1\n",
                "missing render_p95_ms",
            ),
            (
                "gpu = \"x\"\n[ultra]\ngpu_total_p95_ms = 1\nrender_p95_ms = 1\ngpu_p95 = 1\n",
                "unknown key",
            ),
            (
                "gpu = \"x\"\n[epic]\ngpu_total_p95_ms = 1\nrender_p95_ms = 1\n",
                "unknown tier",
            ),
            ("gpu = \"x\"\nnotes = 1\n", "unknown top-level key"),
            (
                "gpu = \"x\"\n[low]\ngpu_total_p95_ms = 1\nrender_p95_ms = 1\n[low]\n",
                "twice",
            ),
            (
                "gpu = \"x\"\n[low]\ngpu_total_p95_ms = -1\nrender_p95_ms = 1\n",
                "negative",
            ),
        ] {
            assert!(Baseline::parse(text).is_err(), "{why}");
        }
        let baseline = Baseline::parse(&baseline_for("0.6")).expect("baseline");
        let (failures, _) = check(&log("gpu_total_p95_ms=0.5"), Some(&baseline), LIMITS);
        assert!(failures.is_empty(), "{failures:?}");
        let other_gpu = FrameLog::parse(
            "frames=100 frames_dropped=0 p50_ms=4.0 p99_ms=5.0 render_p95_ms=0.2 \
             gpu_total_p95_ms=0.5 vram_peak_mb=1 vram_budget_mb=2 locked_uploads=0 \
             locked_pipelines=0 quality=ultra gpu=\"Intel(R) UHD Graphics 620\"",
        );
        let (failures, _) = check(&other_gpu, Some(&baseline), LIMITS);
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("record a baseline")),
            "{failures:?}"
        );
        let low = FrameLog::parse(
            "frames=100 frames_dropped=0 p50_ms=4.0 p99_ms=5.0 render_p95_ms=0.2 \
             gpu_total_p95_ms=0.5 vram_peak_mb=1 vram_budget_mb=2 locked_uploads=0 \
             locked_pipelines=0 quality=low gpu=\"NVIDIA GeForce RTX 5070 Ti\"",
        );
        let (failures, _) = check(&low, Some(&baseline), LIMITS);
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("no [low] table")),
            "{failures:?}"
        );
    }

    #[test]
    fn tiny_baselines_compare_against_a_noise_floor() {
        let baseline = Baseline::parse(&baseline_for("0.01")).expect("baseline");
        let (failures, _) = check(&log("gpu_total_p95_ms=0.05"), Some(&baseline), LIMITS);
        assert!(failures.is_empty(), "{failures:?}");
    }
}
