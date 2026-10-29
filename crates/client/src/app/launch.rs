//! Client startup, settings and capture/exit plumbing.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moba_core::FixVec2;
use moba_net::protocol::MatchPhase;
use moba_net::{NetClient, NetServer};
use moba_platform::{WindowConfig, run as run_window};
use moba_render::{CapturedFrame, OffscreenRender, Renderer, Scene, UiDrawList};
use moba_sim::SimConfig;

use crate::app::ClientApp;

pub(crate) fn frames_is_capture(
    frames: u64,
    capture_frame: u64,
    capture_path: Option<&Path>,
) -> bool {
    capture_path.is_some() && frames >= capture_frame
}

/// Renders `scene` (after the `offscreen` renders, R7b) and writes the frame to
/// `capture_path`. Returns whether the frame was rendered, so the offscreen renders landed.
pub(crate) fn capture_and_exit(
    renderer: &mut Renderer,
    scene: &Scene<'_>,
    ui: &UiDrawList,
    offscreen: &[OffscreenRender<'_>],
    capture_path: Option<&Path>,
    failure: &mut Option<String>,
) -> bool {
    let Some(path) = capture_path else {
        return false;
    };
    match renderer.capture_frame_with_offscreen(scene, ui, offscreen) {
        Ok(frame) => {
            if let Err(error) = write_ppm(path, &frame) {
                *failure = Some(format!("capture write failed: {error}"));
            }
            true
        }
        Err(error) => {
            *failure = Some(format!("capture failed: {error}"));
            // A capture can fail after its frame (and offscreen renders) was submitted.
            renderer.last_frame_submitted()
        }
    }
}

/// Where the renderer keeps its `VkPipelineCache`: `MOBA_PIPELINE_CACHE` (a file path, or
/// `off`), else a per-user cache file: `$XDG_CACHE_HOME` or `~/.cache` on Linux/BSD,
/// `~/Library/Caches` on macOS, `%LOCALAPPDATA%` on Windows, each under
/// `moba-engine/pipeline-cache.bin`. `None` when disabled or no such directory is known.
/// Read once at startup.
pub(crate) fn pipeline_cache_path() -> Option<PathBuf> {
    pipeline_cache_path_from(|name| std::env::var_os(name))
}

fn pipeline_cache_path_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    if let Some(value) = set("MOBA_PIPELINE_CACHE") {
        return (value != "off").then(|| PathBuf::from(value));
    }
    let base = if cfg!(windows) {
        set("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        set("HOME").map(|home| PathBuf::from(home).join("Library/Caches"))
    } else {
        set("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }?;
    Some(base.join("moba-engine").join("pipeline-cache.bin"))
}

/// `MOBA_SHADER_DIR`: SPIR-V overrides, hot-reloaded when a file changes. Debug builds only
/// (a mismatched module is invalid Vulkan usage); release builds log once and ignore it.
pub(crate) fn shader_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("MOBA_SHADER_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)?;
    if cfg!(debug_assertions) {
        Some(dir)
    } else {
        eprintln!("[render] MOBA_SHADER_DIR is a debug-build feature; ignored");
        None
    }
}

/// `MOBA_COMPRESSED_TEXTURES=0` decodes BC textures on the CPU even where the GPU samples
/// them (to exercise the fallback path); anything else keeps the default.
pub(crate) fn compressed_textures() -> bool {
    std::env::var("MOBA_COMPRESSED_TEXTURES").map_or(true, |value| value.trim() != "0")
}

/// Number of `.spv` files in a shader override directory and their newest modification time.
pub(crate) type ShaderStamp = (usize, Option<std::time::SystemTime>);

/// The [`ShaderStamp`] of `dir` (dev shader hot reload), polled a few times a second at
/// most; a missing directory is `(0, None)`. Allocates (directory listing); debug builds only.
pub(crate) fn shader_stamp(dir: &Path) -> ShaderStamp {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, None);
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "spv")
        })
        .fold((0, None), |(count, newest), entry| {
            let modified = entry.metadata().ok().and_then(|meta| meta.modified().ok());
            (count + 1, newest.max(modified))
        })
}

pub(crate) fn parse_demo_move() -> Option<FixVec2> {
    let value = std::env::var("MOBA_DEMO_MOVE").ok()?;
    let (x, y) = value.split_once(',')?;
    Some(FixVec2::from_ints(
        x.trim().parse().ok()?,
        y.trim().parse().ok()?,
    ))
}

/// `MOBA_DEMO_CAST=<slot>@<tick>` (capture demos): cast ability slot 0-2 once, at the first
/// Playing snapshot at or after `tick`.
pub(crate) fn parse_demo_cast() -> Option<(u8, u32)> {
    let value = std::env::var("MOBA_DEMO_CAST").ok()?;
    let (slot, tick) = value.split_once('@')?;
    let slot = slot.trim().parse::<u8>().ok().filter(|slot| *slot < 3)?;
    Some((slot, tick.trim().parse().ok()?))
}

/// Player-camera orbit distance: `MOBA_CAMERA_DISTANCE` metres (clamped to 8–120) or 46.
///
/// Read once at startup (close-up captures of hero models), never per frame.
pub(crate) fn parse_follow_distance() -> f32 {
    std::env::var("MOBA_CAMERA_DISTANCE")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .map_or(46.0, |value| value.clamp(8.0, 120.0))
}

/// `MOBA_CAPTURE_TICK=<n>`: the network capture tick, read once at startup.
pub(crate) fn parse_capture_tick() -> Option<u32> {
    std::env::var("MOBA_CAPTURE_TICK")
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

/// `MOBA_WINDOW_SIZE=<w>x<h>` (logical pixels, 320-7680 × 240-4320), else 1280×720.
pub(crate) fn parse_window_size(value: Option<&str>) -> (u32, u32) {
    value
        .and_then(|value| {
            let (width, height) = value.trim().split_once(['x', 'X'])?;
            let width: u32 = width.trim().parse().ok()?;
            let height: u32 = height.trim().parse().ok()?;
            ((320..=7_680).contains(&width) && (240..=4_320).contains(&height))
                .then_some((width, height))
        })
        .unwrap_or((1_280, 720))
}

/// Latest snapshot tick the client has processed, published for the lock-stepped local
/// capture server (see [`run_local_server`]). Unused otherwise.
pub(crate) static LOCAL_CAPTURE_ACK: AtomicU32 = AtomicU32::new(0);

/// How long the lock-stepped server waits for the client before ticking anyway, so a lost
/// snapshot cannot stall a capture forever. Long enough that a slow software-rendered frame
/// (lavapipe with validation) never triggers it; expiry is logged because the capture may
/// then differ from run to run.
const LOCKSTEP_PATIENCE: Duration = Duration::from_secs(10);

/// Runs the in-process `MOBA_SERVER=local` server at 30 Hz.
///
/// With `stop_at = Some(n)` (a `MOBA_CAPTURE_TICK` capture) the match is lock-stepped to the
/// client and stops at tick `n`:
///
/// - Lobby and Countdown tick in real time (they do not advance the simulation);
/// - a Playing tick only runs once the client has processed the previous tick's snapshot
///   ([`LOCAL_CAPTURE_ACK`]), so the client buffers every snapshot `1..=n` however slowly it
///   renders (lavapipe in CI), and interpolation sees the same history on every run;
/// - after `n` Playing ticks the server keeps polling (the client stays connected) but sends
///   nothing newer than the tick-`n` snapshot;
/// - if the client does not acknowledge a tick within [`LOCKSTEP_PATIENCE`] (a lost
///   datagram), the server logs it and ticks anyway, including once past `n`, so the client
///   can still capture (`latest >= n`) instead of hanging;
/// - a rematch (Lobby/Countdown again) restarts the count with the simulation tick.
fn run_local_server(mut server: NetServer, stop_at: Option<u32>) {
    let Some(stop_at) = stop_at else {
        let _ = server.run_for(Duration::from_secs(3_600));
        return;
    };
    let tick = Duration::from_secs_f64(1.0 / 30.0);
    let started = Instant::now();
    let mut next_tick = started;
    let mut played = 0u32;
    let mut waiting_since = started;
    while started.elapsed() < Duration::from_secs(3_600) {
        if server.poll().is_err() {
            return;
        }
        let now = Instant::now();
        if now >= next_tick {
            // A Playing tick advances the simulation by one; Lobby/Countdown ticks do not.
            let phase = server.phase().0;
            let playing = phase == MatchPhase::Playing;
            if matches!(phase, MatchPhase::Lobby | MatchPhase::Countdown) {
                played = 0;
            }
            let acked = LOCAL_CAPTURE_ACK.load(Ordering::Relaxed);
            let run = if !playing || (acked >= played && played < stop_at) {
                true
            } else if acked < played && now.duration_since(waiting_since) >= LOCKSTEP_PATIENCE {
                eprintln!(
                    "[capture] client has not processed tick {played} (latest {acked}) after \
                     {LOCKSTEP_PATIENCE:?}; ticking anyway, so this capture may not repeat"
                );
                true
            } else {
                false
            };
            if run {
                if server.tick().is_err() {
                    return;
                }
                played += u32::from(playing);
                waiting_since = now;
            }
            next_tick = (next_tick + tick).max(now);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Parses an optional 32-byte hex value from an environment variable.
///
/// Fails closed: a variable that is set but malformed is an error, never silently ignored,
/// so a typo cannot downgrade a pinned or authenticated connection to an unprotected one.
fn hex32_env(name: &str) -> Result<Option<[u8; 32]>, String> {
    let Ok(value) = std::env::var(name) else {
        return Ok(None);
    };
    parse_hex32(&value)
        .map(Some)
        .ok_or_else(|| format!("{name} must be exactly 64 hex characters"))
}

fn parse_hex32(value: &str) -> Option<[u8; 32]> {
    let bytes = value.trim().as_bytes();
    if bytes.len() != 64 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut key = [0u8; 32];
    for (index, chunk) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        key[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(key)
}

pub(crate) fn parse_secret() -> Result<Option<[u8; 32]>, String> {
    hex32_env("MOBA_SECRET")
}

pub(crate) fn parse_server_public() -> Result<Option<[u8; 32]>, String> {
    let key = hex32_env("MOBA_SERVER_PUBKEY")?;
    if key.is_some_and(|key| !moba_net::crypto::is_valid_identity_key(&key)) {
        return Err(String::from(
            "MOBA_SERVER_PUBKEY is not a valid Ed25519 public key (malformed or weak)",
        ));
    }
    Ok(key)
}

pub(crate) fn parse_identity_seed() -> Result<Option<[u8; 32]>, String> {
    hex32_env("MOBA_CLIENT_IDENTITY")
}

pub(crate) fn connect_network(spec: &str) -> Result<NetClient, String> {
    let secret = parse_secret()?;
    let server_public = parse_server_public()?;
    let identity_seed = parse_identity_seed()?;
    let spectator = std::env::var("MOBA_SPECTATE").is_ok_and(|value| value == "1");
    if spec == "local" {
        // The loopback server plays `MOBA_CONTENT` when set, like the local preview, so a
        // pack's catalogue (hero models included) reaches the client over the real wire path.
        let content = match std::env::var_os("MOBA_CONTENT") {
            Some(path) => moba_game::load_content_file(Path::new(&path))
                .map_err(|error| format!("MOBA_CONTENT load failed: {error}"))?,
            None => moba_game::GameContent::default(),
        };
        let config = moba_net::ServerConfig::default()
            .with_bind("127.0.0.1:0".parse().expect("loopback address"))
            .with_sim(SimConfig::default())
            .with_snapshot_interval(1)
            .with_max_clients(10)
            .with_countdown_ticks(90)
            .with_results_ticks(450)
            .with_client_timeout(Duration::from_secs(5))
            .with_reconnect_grace(Duration::from_secs(60))
            .with_pre_shared_key(secret)
            .with_record(None)
            .with_max_spectators(8)
            .with_max_packets_per_second(120)
            .with_max_total_packets_per_second(2_000)
            .with_require_ready(false);
        let server = moba_net::NetServer::bind_with_content(config, Arc::new(content))
            .map_err(|error| format!("local server failed: {error}"))?;
        let addr = server
            .local_addr()
            .map_err(|error| format!("local server address: {error}"))?;
        let stop_at = parse_capture_tick().filter(|_| std::env::var_os("MOBA_CAPTURE").is_some());
        std::thread::spawn(move || run_local_server(server, stop_at));
        let auth = moba_net::client::ClientAuth::default()
            .with_pre_shared_key(secret)
            .with_expected_server(None)
            .with_identity_seed(identity_seed)
            .with_spectator(spectator);
        NetClient::connect_with_auth(addr, Duration::from_secs(5), auth)
            .map_err(|error| format!("local connection failed: {error}"))
    } else {
        let addr = spec
            .parse()
            .map_err(|error| format!("invalid MOBA_SERVER {spec}: {error}"))?;
        let auth = moba_net::client::ClientAuth::default()
            .with_pre_shared_key(secret)
            .with_expected_server(server_public)
            .with_identity_seed(identity_seed)
            .with_spectator(spectator);
        NetClient::connect_with_auth(addr, Duration::from_secs(5), auth)
            .map_err(|error| format!("connection to {spec} failed: {error}"))
    }
}

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let quit_after_frames = std::env::var("MOBA_SMOKE_FRAMES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(u64::MAX);
    let capture_path = std::env::var("MOBA_CAPTURE").ok().map(PathBuf::from);
    let capture_frame = std::env::var("MOBA_CAPTURE_FRAME")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let error = Arc::new(Mutex::new(None::<String>));
    let app = ClientApp::new(
        quit_after_frames,
        capture_path,
        capture_frame,
        Arc::clone(&error),
    );
    let (width, height) = parse_window_size(std::env::var("MOBA_WINDOW_SIZE").ok().as_deref());
    let config = WindowConfig::new("MOBA", width, height);
    run_window(config, app)?;
    if let Some(message) = error.lock().ok().and_then(|mut slot| slot.take()) {
        return Err(message.into());
    }
    Ok(())
}

pub(crate) fn write_ppm(path: &Path, frame: &CapturedFrame) -> std::io::Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    write!(file, "P6\n{} {}\n255\n", frame.width, frame.height)?;
    let mut rgb = Vec::with_capacity(frame.rgba.len() / 4 * 3);
    for pixel in frame.rgba.as_chunks::<4>().0 {
        rgb.extend_from_slice(&pixel[..3]);
    }
    file.write_all(&rgb)?;
    file.flush()
}

impl Drop for ClientApp {
    fn drop(&mut self) {
        let (Some(mut samples), Some(path)) = (self.frame_log.take(), self.frame_log_path.take())
        else {
            return;
        };
        let warmup = samples.len().min(60);
        let mut samples = samples.split_off(warmup);
        if samples.is_empty() {
            return;
        }
        samples.sort_by(f32::total_cmp);
        let percentile = |fraction: f32| {
            let index = ((samples.len() - 1) as f32 * fraction).round() as usize;
            samples[index]
        };
        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        // The physical extent and GPU make each report self-describing.
        let (extent, gpu) = self.renderer.as_ref().map_or_else(
            || (String::from("?"), String::from("?")),
            |renderer| {
                let [width, height] = renderer.extent();
                (
                    format!("{width}x{height}"),
                    renderer.device_name().to_owned(),
                )
            },
        );
        // CPU work before the render call vs time inside it, same warm-up skipped.
        let work = self.work_log.take().unwrap_or_default();
        let work = work.get(warmup.min(work.len())..).unwrap_or_default();
        let split = |lane: usize| {
            let mut values: Vec<f32> = work.iter().map(|sample| sample[lane]).collect();
            values.sort_by(f32::total_cmp);
            let at = |fraction: f32| {
                values
                    .get(((values.len().max(1) - 1) as f32 * fraction).round() as usize)
                    .copied()
                    .unwrap_or(0.0)
            };
            (at(0.50), at(0.95))
        };
        let (cpu_p50, cpu_p95) = split(0);
        let (render_p50, render_p95) = split(1);
        // GPU pass times from timestamp queries, same warm-up skipped.
        let gpu_samples = self.gpu_log.take().unwrap_or_default();
        let gpu_columns = if gpu_samples.len() > warmup {
            gpu_report(&gpu_samples[warmup..])
        } else {
            String::from("gpu_timing=unavailable ")
        };
        let quality = self.renderer.as_ref().map_or_else(
            || String::from("?"),
            |renderer| {
                let pipelines = renderer.pipeline_stats();
                format!(
                    "{} hdr={} pipelines={} pipeline_build_ms={:.3} pipeline_cache={} \
                     startup_ms={:.1}",
                    renderer.quality_tier(),
                    renderer.hdr_format_name(),
                    pipelines.pipelines,
                    pipelines.build_ms,
                    pipelines.cache,
                    pipelines.startup_ms,
                )
            },
        );
        let report = format!(
            "frames={} mean_ms={mean:.3} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3} \
             cpu_p50_ms={cpu_p50:.3} cpu_p95_ms={cpu_p95:.3} \
             render_p50_ms={render_p50:.3} render_p95_ms={render_p95:.3} \
             {gpu_columns}quality={quality} extent={extent} gpu=\"{gpu}\"\n",
            samples.len(),
            percentile(0.50),
            percentile(0.95),
            percentile(0.99),
            samples[samples.len() - 1],
        );
        let _ = std::fs::write(&path, &report);
        eprintln!("frame log: {}", report.trim_end());
    }
}

/// `gpu_<lane>_p50_ms`/`_p95_ms` columns for the frame log, from `[scene, composite, ui,
/// total, span, portraits, minimap]` samples. `total` sums the passes; `span` runs from the
/// first pass's start to the last pass's end, so `span - total` is the time spent in
/// barriers between passes. The offscreen passes (R7b) run only in some frames, so their
/// percentiles cover just those frames, and `gpu_<pass>_runs` counts them; with no run the
/// columns are left out.
fn gpu_report(samples: &[[f32; 7]]) -> String {
    use core::fmt::Write as _;
    let mut columns = String::new();
    for (lane, name) in [
        "scene",
        "composite",
        "ui",
        "total",
        "span",
        "portraits",
        "minimap",
    ]
    .into_iter()
    .enumerate()
    {
        let offscreen = lane >= 5;
        let mut values: Vec<f32> = samples
            .iter()
            .map(|sample| sample[lane])
            .filter(|&value| !offscreen || value > 0.0)
            .collect();
        if offscreen {
            if values.is_empty() {
                continue;
            }
            let _ = write!(columns, "gpu_{name}_runs={} ", values.len());
        }
        values.sort_by(f32::total_cmp);
        let at = |fraction: f32| {
            values
                .get(((values.len().max(1) - 1) as f32 * fraction).round() as usize)
                .copied()
                .unwrap_or(0.0)
        };
        let _ = write!(
            columns,
            "gpu_{name}_p50_ms={:.3} gpu_{name}_p95_ms={:.3} ",
            at(0.50),
            at(0.95)
        );
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::{gpu_report, parse_hex32, parse_window_size, pipeline_cache_path_from};

    #[test]
    fn pipeline_cache_path_follows_the_override_then_the_platform_cache_dir() {
        use std::ffi::OsString;
        use std::path::PathBuf;
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        assert_eq!(
            pipeline_cache_path_from(env(&[("MOBA_PIPELINE_CACHE", "off")])),
            None
        );
        assert_eq!(
            pipeline_cache_path_from(env(&[
                ("MOBA_PIPELINE_CACHE", "/tmp/x.bin"),
                ("HOME", "/h")
            ])),
            Some(PathBuf::from("/tmp/x.bin"))
        );
        assert_eq!(pipeline_cache_path_from(env(&[])), None);
        if cfg!(all(unix, not(target_os = "macos"))) {
            assert_eq!(
                pipeline_cache_path_from(env(&[("HOME", "/h")])),
                Some(PathBuf::from("/h/.cache/moba-engine/pipeline-cache.bin"))
            );
            assert_eq!(
                pipeline_cache_path_from(env(&[("XDG_CACHE_HOME", "/c"), ("HOME", "/h")])),
                Some(PathBuf::from("/c/moba-engine/pipeline-cache.bin"))
            );
        }
    }

    #[test]
    fn gpu_report_lists_each_pass_percentile() {
        let samples: Vec<[f32; 7]> = (0..=100)
            .map(|index| {
                let ms = index as f32 / 100.0;
                // The portrait pass ran in every tenth frame; the minimap never did.
                let portraits = if index % 10 == 0 { 0.02 } else { 0.0 };
                [ms, ms * 2.0, 0.5, ms * 3.0, ms * 4.0, portraits, 0.0]
            })
            .collect();
        let report = gpu_report(&samples);
        assert!(report.starts_with("gpu_scene_p50_ms=0.500 gpu_scene_p95_ms=0.950 "));
        assert!(report.contains("gpu_composite_p95_ms=1.900 "));
        assert!(report.contains("gpu_ui_p50_ms=0.500 gpu_ui_p95_ms=0.500 "));
        assert!(report.contains("gpu_total_p50_ms=1.500 gpu_total_p95_ms=2.850 "));
        assert!(report.contains("gpu_span_p50_ms=2.000 gpu_span_p95_ms=3.800 "));
        // Offscreen passes: percentiles over the frames they ran in, plus a run count.
        assert!(report.ends_with(
            "gpu_portraits_runs=11 gpu_portraits_p50_ms=0.020 gpu_portraits_p95_ms=0.020 "
        ));
        assert!(!report.contains("gpu_minimap"));
    }

    #[test]
    fn window_size_parses_or_falls_back() {
        assert_eq!(parse_window_size(None), (1_280, 720));
        assert_eq!(parse_window_size(Some("1920x1080")), (1_920, 1_080));
        assert_eq!(parse_window_size(Some(" 2560 X 1440 ")), (2_560, 1_440));
        for bad in ["", "1920", "0x0", "99999x1080", "axb", "1920x1080x2"] {
            assert_eq!(parse_window_size(Some(bad)), (1_280, 720), "{bad}");
        }
    }

    #[test]
    fn hex32_accepts_only_exact_hex_keys() {
        let key = "ab".repeat(32);
        assert_eq!(parse_hex32(&key), Some([0xAB; 32]));
        assert_eq!(parse_hex32(&format!(" {key}\n")), Some([0xAB; 32]));
        // Typos must fail closed rather than silently disabling pinning/encryption.
        assert_eq!(parse_hex32(&key[..63]), None);
        assert_eq!(parse_hex32(&format!("{key}0")), None);
        assert_eq!(parse_hex32(&format!("g{}", &key[1..])), None);
        assert_eq!(parse_hex32(&format!("é{}", &key[2..])), None);
    }
}
