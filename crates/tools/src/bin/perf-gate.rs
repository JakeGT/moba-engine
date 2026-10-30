//! Frame-log perf gate (R8, ADR-0018 §6): checks a client frame log (`MOBA_FRAME_LOG`) of
//! the teamfight replay against the roadmap's §7 GPU budget for its tier, the frame-pacing
//! rule (p99 within 1.5× p50), the load-time-only contract (no mid-match uploads or pipeline
//! builds) and a committed baseline (`tests/perf/baseline.toml`), and exits non-zero on any
//! breach. The nightly perf workflow runs it on the self-hosted GPU runner; it runs the same
//! way on a developer machine.
//!
//! ```text
//! perf-gate [--baseline FILE] [--max-regression FRACTION] [--pacing RATIO] <frame-log>...
//! ```
//!
//! Each frame log names its tier (`quality=`); the baseline holds one `[tier]` table per tier
//! with the reference `gpu_total_p95_ms` (and optionally `render_p95_ms`) measured on the
//! runner's GPU. A tier without a baseline is checked against the budgets only.
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

    fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
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

/// `[tier]` tables of `key = number` lines (the TOML subset the baseline uses).
#[derive(Debug, Default, PartialEq)]
struct Baseline {
    tables: Vec<(String, Vec<(String, f64)>)>,
}

impl Baseline {
    fn parse(text: &str) -> Result<Self, String> {
        let mut baseline = Self::default();
        for (number, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
            {
                baseline.tables.push((name.trim().to_owned(), Vec::new()));
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected `key = value`", number + 1))?;
            let value = value.trim();
            let Some((_, entries)) = baseline.tables.last_mut() else {
                // Top-level keys (metadata) are allowed and ignored.
                continue;
            };
            // Strings (metadata inside a table) are skipped; numbers are kept.
            if value.starts_with('"') {
                continue;
            }
            let value: f64 = value
                .parse()
                .map_err(|_| format!("line {}: {value:?} is not a number", number + 1))?;
            entries.push((key.trim().to_owned(), value));
        }
        Ok(baseline)
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
    (failures, notes)
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: perf-gate [--baseline FILE] [--max-regression FRACTION] [--pacing RATIO] \
         <frame-log>..."
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut baseline_path = None;
    let mut limits = Limits {
        max_regression: DEFAULT_MAX_REGRESSION,
        pacing: DEFAULT_PACING,
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
    };

    fn log(fields: &str) -> FrameLog {
        FrameLog::parse(&format!(
            "frames=100 p50_ms=4.0 p95_ms=4.4 p99_ms=5.0 render_p95_ms=0.2 {fields} \
             locked_uploads=0 locked_pipelines=0 quality=ultra hdr=X gpu=\"NVIDIA GeForce RTX 5070 Ti\"\n"
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
            "# reference\ngpu = \"x\"\n[ultra]\ngpu_total_p95_ms = 0.57 # RTX\nrender_p95_ms = 0.2\n",
        )
        .expect("baseline");
        assert_eq!(baseline.get("ultra", "gpu_total_p95_ms"), Some(0.57));
        let (failures, notes) = check(&log("gpu_total_p95_ms=0.6"), Some(&baseline), LIMITS);
        assert!(failures.is_empty(), "{failures:?}");
        assert!(notes.iter().any(|note| note.contains("budget 7.5 ms")));
    }

    #[test]
    fn breaches_fail_with_a_reason() {
        let baseline = Baseline::parse("[ultra]\ngpu_total_p95_ms = 0.5\n").expect("baseline");
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
            "frames=9 p50_ms=4.0 p99_ms=7.0 gpu_total_p95_ms=1 locked_uploads=0 \
             locked_pipelines=2 quality=high",
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
        let (failures, _) = check(&FrameLog::parse("frames=1 quality=epic"), None, LIMITS);
        assert_eq!(failures.len(), 1, "an unknown tier stops the check");
        assert!(Baseline::parse("[ultra]\ngpu_total_p95_ms = fast\n").is_err());
    }

    #[test]
    fn tiny_baselines_compare_against_a_noise_floor() {
        let baseline = Baseline::parse("[ultra]\ngpu_total_p95_ms = 0.01\n").expect("baseline");
        let (failures, _) = check(&log("gpu_total_p95_ms=0.05"), Some(&baseline), LIMITS);
        assert!(failures.is_empty(), "{failures:?}");
    }
}
