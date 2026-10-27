//! Client startup, settings and capture/exit plumbing.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moba_core::FixVec2;
use moba_net::NetClient;
use moba_platform::{WindowConfig, run as run_window};
use moba_render::{CapturedFrame, Renderer, Scene, UiDrawList};
use moba_sim::SimConfig;

use crate::app::ClientApp;

pub(crate) fn frames_is_capture(
    frames: u64,
    capture_frame: u64,
    capture_path: Option<&Path>,
) -> bool {
    capture_path.is_some() && frames >= capture_frame
}

pub(crate) fn capture_and_exit(
    renderer: &mut Renderer,
    scene: &Scene<'_>,
    ui: &UiDrawList,
    capture_path: Option<&Path>,
    failure: &mut Option<String>,
) {
    let Some(path) = capture_path else {
        return;
    };
    match renderer.capture_frame_with_ui(scene, ui) {
        Ok(frame) => {
            if let Err(error) = write_ppm(path, &frame) {
                *failure = Some(format!("capture write failed: {error}"));
            }
        }
        Err(error) => *failure = Some(format!("capture failed: {error}")),
    }
}

pub(crate) fn parse_demo_move() -> Option<FixVec2> {
    let value = std::env::var("MOBA_DEMO_MOVE").ok()?;
    let (x, y) = value.split_once(',')?;
    Some(FixVec2::from_ints(
        x.trim().parse().ok()?,
        y.trim().parse().ok()?,
    ))
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
        let config = moba_net::ServerConfig {
            bind: "127.0.0.1:0".parse().expect("loopback address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 90,
            results_ticks: 450,
            client_timeout: Duration::from_secs(5),
            reconnect_grace: Duration::from_secs(60),
            pre_shared_key: secret,
            record: None,
            max_spectators: 8,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
        };
        let mut server = moba_net::NetServer::bind_with_content(config, Arc::new(content))
            .map_err(|error| format!("local server failed: {error}"))?;
        let addr = server
            .local_addr()
            .map_err(|error| format!("local server address: {error}"))?;
        std::thread::spawn(move || {
            let _ = server.run_for(Duration::from_secs(3_600));
        });
        let auth = moba_net::client::ClientAuth {
            pre_shared_key: secret,
            expected_server: None,
            identity_seed,
            spectator,
        };
        NetClient::connect_with_auth(addr, Duration::from_secs(5), auth)
            .map_err(|error| format!("local connection failed: {error}"))
    } else {
        let addr = spec
            .parse()
            .map_err(|error| format!("invalid MOBA_SERVER {spec}: {error}"))?;
        let auth = moba_net::client::ClientAuth {
            pre_shared_key: secret,
            expected_server: server_public,
            identity_seed,
            spectator,
        };
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
    let config = WindowConfig::new("MOBA", 1280, 720);
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
        let report = format!(
            "frames={} mean_ms={mean:.3} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3}\n",
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

#[cfg(test)]
mod tests {
    use super::parse_hex32;

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
