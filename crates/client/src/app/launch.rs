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
/// Frame-log samples kept (`MOBA_FRAME_LOG_FRAMES`, default 10,000, clamped to
/// `100..=1,000,000`; R8: a 10-minute replay at 144 Hz is ≈ 86,000). Allocated once at start.
pub(crate) fn frame_log_frames() -> usize {
    frame_log_frames_from(std::env::var("MOBA_FRAME_LOG_FRAMES").ok().as_deref())
}

fn frame_log_frames_from(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map_or(10_000, |frames| frames.clamp(100, 1_000_000))
}

/// The culling mode from `MOBA_GPU_CULL` (`off` draws everything; `cpu`/`on`, the default,
/// culls per view on the CPU; R8, ADR-0018 §1).
pub(crate) fn cull_mode() -> moba_render::CullMode {
    cull_mode_from(std::env::var("MOBA_GPU_CULL").ok().as_deref())
}

fn cull_mode_from(value: Option<&str>) -> moba_render::CullMode {
    value.map_or(moba_render::CullMode::Cpu, |name| {
        moba_render::CullMode::from_name(name).unwrap_or_else(|| {
            eprintln!("MOBA_GPU_CULL={name:?} is not off/cpu/on; culling on the CPU");
            moba_render::CullMode::Cpu
        })
    })
}

/// The mesh LOD selection from `MOBA_MESH_LOD`: `off` draws LOD0, a number is the pixel
/// tolerance; unset, the tier's (R8, ADR-0018 §3).
pub(crate) fn mesh_lod() -> Option<moba_render::LodConfig> {
    mesh_lod_from(std::env::var("MOBA_MESH_LOD").ok().as_deref())
}

fn mesh_lod_from(value: Option<&str>) -> Option<moba_render::LodConfig> {
    let value = value?.trim();
    if value.eq_ignore_ascii_case("off") || value == "0" {
        return Some(moba_render::LodConfig::OFF);
    }
    match value.parse::<f32>() {
        Ok(pixels) if pixels.is_finite() && pixels > 0.0 => {
            Some(moba_render::LodConfig::with_tolerance(pixels))
        }
        _ => {
            eprintln!("MOBA_MESH_LOD={value:?} is not off or a pixel tolerance; using the tier's");
            None
        }
    }
}

/// One `[vram]` line: device memory by class against the tier's budgets, and the upload
/// counters (R8, ADR-0018 §4).
pub(crate) fn memory_summary(stats: &moba_render::MemoryStats) -> String {
    let mib = |bytes: u64| bytes as f64 / f64::from(1u32 << 20);
    format!(
        "vram_mb={:.1} vram_budget_mb={:.0} vram_peak_mb={:.1} vram_over_budget={} \
         vram_reserved_mb={:.1} texture_mb={:.1} texture_budget_mb={:.0} targets_mb={:.1} \
         buffers_mb={:.1} swapchain_mb={:.1} degraded_textures={} refused_textures={} \
         uploads={} locked_uploads={} locked_pipelines={}",
        mib(stats.total_bytes()),
        mib(stats.total_budget),
        mib(stats.peak_bytes),
        u8::from(stats.over_budget()),
        mib(stats.reserved_bytes),
        mib(stats.texture_bytes),
        mib(stats.texture_budget),
        mib(stats.target_bytes),
        mib(stats.buffer_bytes),
        mib(stats.swapchain_bytes),
        stats.degraded_textures,
        stats.refused_textures,
        stats.uploads,
        stats.locked_uploads,
        stats.locked_pipelines,
    )
}

/// Whether the mid-match upload lock should be armed (R8, ADR-0018 §4): only while a match
/// is playing and frames are measured. A networked client loads in the lobby and loading
/// phases, so it locks only in [`MatchPhase::Playing`]. The local preview and replays load
/// everything before their first frame and have no phase (`None`).
pub(crate) fn upload_lock_wanted(
    measuring: bool,
    phase: Option<moba_net::protocol::MatchPhase>,
) -> bool {
    measuring && phase.is_none_or(|phase| phase == moba_net::protocol::MatchPhase::Playing)
}

pub(crate) fn compressed_textures() -> bool {
    std::env::var("MOBA_COMPRESSED_TEXTURES").map_or(true, |value| value.trim() != "0")
}

/// The lighting features for `tier` with the `MOBA_SHADOWS` (`off|hard|pcf|pcss`),
/// `MOBA_SHADOW_SIZE`, `MOBA_IBL` and `MOBA_SPECULAR_IBL` (`0` turns either off) overrides,
/// and the R2b switches (ADR-0016): `MOBA_LIGHTS=off|<global cap>`,
/// `MOBA_OUTLINES=off|hull|screen`, `MOBA_BAKED_AO=off` and
/// `MOBA_CONTACT_SHADOWS=off|<ray-march steps>`.
pub(crate) fn lighting_config(tier: moba_render::QualityTier) -> moba_render::LightingConfig {
    lighting_config_from(tier, |name| std::env::var(name).ok())
}

fn lighting_config_from(
    tier: moba_render::QualityTier,
    var: impl Fn(&str) -> Option<String>,
) -> moba_render::LightingConfig {
    let mut config = moba_render::LightingConfig::for_tier(tier);
    if let Some(value) = var("MOBA_SHADOWS") {
        match moba_render::ShadowFilter::from_name(&value) {
            Some(filter) => config = config.with_shadows(filter),
            None => eprintln!("MOBA_SHADOWS={value:?} is not off/hard/pcf/pcss; using the tier's"),
        }
    }
    if let Some(value) = var("MOBA_SHADOW_SIZE") {
        match value.trim().parse::<u32>() {
            Ok(size) => config = config.with_shadow_resolution(size),
            Err(_) => eprintln!("MOBA_SHADOW_SIZE={value:?} is not a number; using the tier's"),
        }
    }
    let off = |name| var(name).is_some_and(|value| value.trim() == "0");
    if off("MOBA_IBL") {
        config = config.with_ibl(false);
    }
    if off("MOBA_SPECULAR_IBL") {
        config = config.with_specular_ibl(false);
    }
    let switched_off = |value: &str| matches!(value.trim(), "off" | "OFF" | "0");
    if let Some(value) = var("MOBA_LIGHTS") {
        match value.trim().parse::<u32>() {
            _ if switched_off(&value) => config = config.with_max_lights(0),
            Ok(cap) => config = config.with_max_lights(cap),
            Err(_) => eprintln!("MOBA_LIGHTS={value:?} is not off or a count; using the tier's"),
        }
    }
    if let Some(value) = var("MOBA_OUTLINES") {
        match moba_render::OutlineMode::from_name(&value) {
            Some(mode) => config = config.with_outlines(mode),
            None => eprintln!("MOBA_OUTLINES={value:?} is not off/hull/screen; using the tier's"),
        }
    }
    if var("MOBA_BAKED_AO").is_some_and(|value| switched_off(&value)) {
        config = config.with_baked_ao(false);
    }
    if let Some(value) = var("MOBA_CONTACT_SHADOWS") {
        match value.trim().parse::<u32>() {
            _ if switched_off(&value) => config = config.with_contact_shadow_steps(0),
            Ok(steps) => config = config.with_contact_shadow_steps(steps),
            Err(_) => eprintln!(
                "MOBA_CONTACT_SHADOWS={value:?} is not off or a step count; using the tier's"
            ),
        }
    }
    config
}

/// The post-processing passes for `tier` (ADR-0015) with the settings' `render_scale` and
/// the environment overrides: `MOBA_BLOOM=off|on`, `MOBA_AA=off|fxaa|msaa4`,
/// `MOBA_AO=off|on`, `MOBA_RENDER_SCALE=0.5..1`, `MOBA_DYNRES=off|on|<target ms>`,
/// `MOBA_GRADING=off`, `MOBA_EXPOSURE=fixed|auto|<EV>` (only `fixed`/`auto` change the
/// passes; an EV is a map-look override), `MOBA_UPSCALE=bilinear|bicubic` and
/// `MOBA_SCREEN_EFFECTS=off`. With `capture` (`MOBA_CAPTURE`), auto exposure and dynamic
/// resolution are pinned off unless set explicitly, so captures stay byte-identical.
pub(crate) fn post_config(
    tier: moba_render::QualityTier,
    render_scale: f32,
    capture: bool,
) -> moba_render::PostConfig {
    post_config_from(tier, render_scale, capture, |name| std::env::var(name).ok())
}

fn post_config_from(
    tier: moba_render::QualityTier,
    render_scale: f32,
    capture: bool,
    var: impl Fn(&str) -> Option<String>,
) -> moba_render::PostConfig {
    let mut config = moba_render::PostConfig::for_tier(tier).with_render_scale(render_scale);
    let switch = |name: &str| {
        let value = var(name)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" => Some(false),
            "on" | "1" | "true" => Some(true),
            _ => {
                eprintln!("{name}={value:?} is not off/on; using the tier's");
                None
            }
        }
    };
    if let Some(on) = switch("MOBA_BLOOM") {
        config = config.with_bloom(on);
    }
    if let Some(on) = switch("MOBA_AO") {
        config = config.with_ambient_occlusion(on);
    }
    if let Some(on) = switch("MOBA_GRADING") {
        config = config.with_grading(on);
    }
    if let Some(on) = switch("MOBA_SCREEN_EFFECTS") {
        config = config.with_screen_effects(on);
    }
    if let Some(value) = var("MOBA_AA") {
        match moba_render::AntiAliasing::from_name(&value) {
            Some(mode) => config = config.with_anti_aliasing(mode),
            None => eprintln!("MOBA_AA={value:?} is not off/fxaa/msaa4; using the tier's"),
        }
    }
    if let Some(value) = var("MOBA_UPSCALE") {
        match moba_render::UpscaleFilter::from_name(&value) {
            Some(filter) => config = config.with_upscale(filter),
            None => eprintln!("MOBA_UPSCALE={value:?} is not bilinear/bicubic; using the tier's"),
        }
    }
    if let Some(value) = var("MOBA_RENDER_SCALE") {
        match value.trim().parse::<f32>() {
            Ok(scale) if scale.is_finite() => config = config.with_render_scale(scale),
            _ => eprintln!("MOBA_RENDER_SCALE={value:?} is not a number; using the setting"),
        }
    }
    match var("MOBA_DYNRES") {
        None => {
            if capture {
                config = config.with_dynamic_resolution(false);
            }
        }
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" => config = config.with_dynamic_resolution(false),
            "on" | "1" => config = config.with_dynamic_resolution(true),
            other => match other.parse::<f32>() {
                Ok(target) if target.is_finite() => {
                    config = config
                        .with_dynamic_resolution(true)
                        .with_dynamic_target_ms(target);
                }
                _ => eprintln!("MOBA_DYNRES={value:?} is not off/on/<ms>; using the tier's"),
            },
        },
    }
    let exposure = var("MOBA_EXPOSURE");
    let parsed = exposure
        .as_deref()
        .and_then(crate::app::map_visuals::ExposureOverride::parse);
    if let (Some(value), None) = (&exposure, parsed) {
        eprintln!("MOBA_EXPOSURE={value:?} is not fixed/auto/<EV>; using the map's");
    }
    // Only an explicit `auto`/`fixed` decides; captures otherwise pin eye adaptation off.
    match parsed.and_then(|parsed| parsed.auto) {
        Some(auto) => config = config.with_auto_exposure(auto),
        None if capture => config = config.with_auto_exposure(false),
        None => {}
    }
    config.sanitized()
}

/// `MOBA_EXPOSURE` as a map-look override (an EV, or forcing eye adaptation on/off).
pub(crate) fn exposure_override() -> crate::app::map_visuals::ExposureOverride {
    std::env::var("MOBA_EXPOSURE")
        .ok()
        .as_deref()
        .and_then(crate::app::map_visuals::ExposureOverride::parse)
        .unwrap_or_default()
}

/// The terrain, fog-of-war and atmosphere features for `tier` (ADR-0014) with the
/// `MOBA_TERRAIN=flat|splat`, `MOBA_FOW=texture|legacy|off`, `MOBA_FOW_SIZE=<64..1024>`,
/// `MOBA_FOG=off` (height fog) and `MOBA_SKY=off` overrides.
pub(crate) fn world_config(tier: moba_render::QualityTier) -> moba_render::WorldConfig {
    world_config_from(tier, |name| std::env::var(name).ok())
}

fn world_config_from(
    tier: moba_render::QualityTier,
    var: impl Fn(&str) -> Option<String>,
) -> moba_render::WorldConfig {
    let mut config = moba_render::WorldConfig::for_tier(tier);
    if let Some(value) = var("MOBA_TERRAIN") {
        match moba_render::TerrainShading::from_name(&value) {
            Some(shading) => config = config.with_terrain(shading),
            None => eprintln!("MOBA_TERRAIN={value:?} is not flat/splat; using the tier's"),
        }
    }
    if let Some(value) = var("MOBA_FOW") {
        match moba_render::FogOfWarMode::from_name(&value) {
            Some(mode) => config = config.with_fog_of_war(mode),
            None => eprintln!("MOBA_FOW={value:?} is not texture/legacy/off; using the tier's"),
        }
    }
    if let Some(value) = var("MOBA_FOW_SIZE") {
        match value.trim().parse::<u32>() {
            Ok(size) => config = config.with_fog_of_war_resolution(size),
            Err(_) => eprintln!("MOBA_FOW_SIZE={value:?} is not a number; using the tier's"),
        }
    }
    let off = |name| var(name).is_some_and(|value| value.trim().eq_ignore_ascii_case("off"));
    if off("MOBA_FOG") {
        config = config.with_height_fog(false);
    }
    if off("MOBA_SKY") {
        config = config.with_sky(false);
    }
    config
}

/// `MOBA_DISTORTION` (ADR-0019) as `(enabled, drawn)`: `off` (or `0`) has no heat haze on
/// any tier; `nodraw` keeps every haze spawn and its simulation but never draws it (a
/// diagnostic: a capture then differs from the normal one only by the refraction).
pub(crate) fn distortion_switch() -> (bool, bool) {
    distortion_switch_from(std::env::var("MOBA_DISTORTION").ok().as_deref())
}

fn distortion_switch_from(value: Option<&str>) -> (bool, bool) {
    match value.map(str::trim) {
        None => (true, true),
        Some(value) if value.eq_ignore_ascii_case("off") || value == "0" => (false, true),
        Some(value) if value.eq_ignore_ascii_case("on") || value == "1" => (true, true),
        Some(value) if value.eq_ignore_ascii_case("nodraw") => (true, false),
        Some(value) => {
            eprintln!("MOBA_DISTORTION={value:?} is not on/off/nodraw; using the tier's");
            (true, true)
        }
    }
}

/// The decal, foliage and water features for `tier` (ADR-0017) with the `MOBA_DECALS=off`,
/// `MOBA_FOLIAGE=off` and `MOBA_WATER=off|simple|full` overrides.
pub(crate) fn environment_config(tier: moba_render::QualityTier) -> moba_render::EnvironmentConfig {
    environment_config_from(tier, |name| std::env::var(name).ok())
}

fn environment_config_from(
    tier: moba_render::QualityTier,
    var: impl Fn(&str) -> Option<String>,
) -> moba_render::EnvironmentConfig {
    let mut config = moba_render::EnvironmentConfig::for_tier(tier);
    let off = |name| var(name).is_some_and(|value| value.trim().eq_ignore_ascii_case("off"));
    if off("MOBA_DECALS") {
        config = config.with_decals(false);
    }
    if off("MOBA_FOLIAGE") {
        config = config.with_foliage(false);
    }
    if let Some(value) = var("MOBA_WATER") {
        match moba_render::WaterMode::from_name(&value) {
            Some(mode) => config = config.with_water(mode),
            None => eprintln!("MOBA_WATER={value:?} is not off/simple/full; using the tier's"),
        }
    }
    config
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

/// One scripted order of `MOBA_DEMO_ORDERS` (capture and evidence runs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DemoOrder {
    /// Start recalling.
    Recall,
    /// Stop (cancels a recall).
    Stop,
    /// Cast ability slot 0-2 four units from the hero, like `MOBA_DEMO_CAST`.
    Cast(u8),
    /// Walk to a map position.
    Move(FixVec2),
}

/// Most orders `MOBA_DEMO_ORDERS` takes.
const MAX_DEMO_ORDERS: usize = 32;

/// `MOBA_DEMO_ORDERS=<order>@<tick>,...` (capture and evidence runs): scripted orders sent
/// at the first Playing snapshot at or after each tick, in tick order. An order is
/// `recall`, `stop`, `q`/`w`/`e` (cast at four units from the hero) or `move:<x>:<y>`. An
/// entry that does not parse is skipped (and logged); at most 32 are kept.
pub(crate) fn parse_demo_orders() -> Vec<(u32, DemoOrder)> {
    std::env::var("MOBA_DEMO_ORDERS").map_or_else(|_| Vec::new(), |value| demo_orders(&value))
}

pub(crate) fn demo_orders(value: &str) -> Vec<(u32, DemoOrder)> {
    let mut orders: Vec<(u32, DemoOrder)> = value
        .split(',')
        .filter(|entry| !entry.trim().is_empty())
        .filter_map(|entry| {
            let parsed = entry.trim().split_once('@').and_then(|(order, tick)| {
                let tick = tick.trim().parse::<u32>().ok()?;
                let order = match order.trim().to_ascii_lowercase().as_str() {
                    "recall" => DemoOrder::Recall,
                    "stop" => DemoOrder::Stop,
                    "q" => DemoOrder::Cast(0),
                    "w" => DemoOrder::Cast(1),
                    "e" => DemoOrder::Cast(2),
                    other => {
                        let (x, y) = other.strip_prefix("move:")?.split_once(':')?;
                        DemoOrder::Move(FixVec2::from_ints(
                            x.trim().parse().ok()?,
                            y.trim().parse().ok()?,
                        ))
                    }
                };
                Some((tick, order))
            });
            if parsed.is_none() {
                eprintln!("[demo] MOBA_DEMO_ORDERS: skipping {entry:?}");
            }
            parsed
        })
        .take(MAX_DEMO_ORDERS)
        .collect();
    orders.sort_by_key(|(tick, _)| *tick);
    orders
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

/// Largest replay file `MOBA_REPLAY` reads, in bytes.
const MAX_REPLAY_BYTES: u64 = 64 * 1024 * 1024;

/// `MOBA_REPLAY=<file.mbr>`: the recording the local view plays, checked against `content`
/// (the replay must have been recorded with the same pack). `Ok(None)` when unset.
pub(crate) fn load_replay(
    content: &moba_game::GameContent,
) -> Result<Option<moba_sim::Replay>, String> {
    let Some(path) = std::env::var_os("MOBA_REPLAY").filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    let describe =
        |error: &dyn core::fmt::Display| format!("MOBA_REPLAY {}: {error}", path.display());
    let file = File::open(&path).map_err(|error| describe(&error))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, MAX_REPLAY_BYTES + 1),
        &mut bytes,
    )
    .map_err(|error| describe(&error))?;
    if bytes.len() as u64 > MAX_REPLAY_BYTES {
        return Err(describe(&"larger than 64 MiB"));
    }
    let replay = moba_sim::Replay::decode(&bytes).map_err(|error| describe(&error))?;
    if replay.content_hash != content.hash() {
        return Err(describe(&format!(
            "recorded with content {:#018x} ({} {}), loaded {:#018x}; set MOBA_CONTENT to the \
             recording's pack",
            replay.content_hash,
            replay.content_name,
            replay.content_version,
            content.hash()
        )));
    }
    eprintln!(
        "[replay] {}: {} ticks of {} {}",
        path.display(),
        replay.frames.len(),
        replay.content_name,
        replay.content_version
    );
    Ok(Some(replay))
}

/// The replay camera: the live heroes' centroid (world space) and the follow distance.
pub(crate) fn replay_focus(
    simulation: &moba_sim::Simulation,
    center: f32,
) -> (moba_core::math::Vec3, f32) {
    let mut sum = moba_core::math::Vec3::ZERO;
    let mut count = 0.0f32;
    for slot in 0..moba_sim::MAX_PLAYERS {
        let alive = simulation
            .hero_health(slot)
            .is_some_and(|health| health.current > 0);
        if let (true, Some(position)) = (alive, simulation.hero_position(slot)) {
            sum += crate::app::vfx::world(position, center);
            count += 1.0;
        }
    }
    let focus = if count > 0.0 {
        sum / count
    } else {
        moba_core::math::Vec3::ZERO
    };
    (focus, parse_follow_distance())
}

/// Local-preview camera orbit distance: `MOBA_LOCAL_CAMERA_DISTANCE` metres (clamped to 8–240)
/// or the default overview of 235 m (close-ups of the R4b stress scene).
pub(crate) fn local_camera_distance() -> f32 {
    std::env::var("MOBA_LOCAL_CAMERA_DISTANCE")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .map_or(235.0, |value| value.clamp(8.0, 240.0))
}

/// `MOBA_CAPTURE_TICK=<n>`: the network capture tick, read once at startup.
pub(crate) fn parse_capture_tick() -> Option<u32> {
    std::env::var("MOBA_CAPTURE_TICK")
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

/// `MOBA_WINDOW_SIZE=<w>x<h>` (logical pixels, 320-7680 × 240-4320), else 1280×720.
/// `MOBA_PREVIEW_CAMERA=<yaw>,<pitch>,<distance>[,<x>,<z>]` (radians, radians, metres, and
/// an optional focus in metres from the map centre): a fixed orbit for the local preview (R3a
/// and R3b goldens). Pitch is clamped by the orbit camera, the distance to 8–600 m and the
/// focus to ±200 m. Returns `[yaw, pitch, distance, x, z]`.
pub(crate) fn parse_preview_camera(value: Option<&str>) -> Option<[f32; 5]> {
    let parts: Vec<f32> = value?
        .split(',')
        .map(|part| part.trim().parse::<f32>().ok())
        .collect::<Option<_>>()?;
    let [yaw, pitch, distance, x, z] = match parts[..] {
        [yaw, pitch, distance] => [yaw, pitch, distance, 0.0, 0.0],
        [yaw, pitch, distance, x, z] => [yaw, pitch, distance, x, z],
        _ => return None,
    };
    [yaw, pitch, distance, x, z]
        .iter()
        .all(|value| value.is_finite())
        .then(|| {
            [
                yaw,
                pitch,
                distance.clamp(8.0, 600.0),
                x.clamp(-200.0, 200.0),
                z.clamp(-200.0, 200.0),
            ]
        })
}

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
    #[allow(clippy::too_many_lines)] // One linear frame-log report.
    fn drop(&mut self) {
        if let Some(stress) = &self.stress {
            stress.report();
        }
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
        let quality = self
            .renderer
            .as_ref()
            .map_or_else(|| String::from("?"), quality_summary);
        // R8: memory, mid-match uploads and the main view's culling in the last frame.
        let memory = self.renderer.as_ref().map_or_else(String::new, |renderer| {
            let cull = renderer.cull_stats();
            let lods = cull
                .lod_instances
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("/");
            format!(
                "{} cull={} cull_candidates={} cull_drawn={} cull_casters={} cull_dropped={} \
                 terrain_chunks={}/{} lod_instances={lods} ",
                memory_summary(&renderer.memory_stats()),
                cull.mode,
                cull.candidates,
                cull.drawn,
                cull.casters,
                cull.dropped,
                cull.terrain_chunks_drawn,
                cull.terrain_chunks,
            )
        });
        let stress = self
            .stress
            .as_ref()
            .and_then(|stress| {
                stress.summary().map(|(mean, p95, _)| {
                    format!(
                        "stress_minions={} stress_cpu_mean_us={mean:.1} stress_cpu_p95_us={p95:.1} ",
                        stress.count()
                    )
                })
            })
            .unwrap_or_default();
        let report = format!(
            "frames={} frames_dropped={} mean_ms={mean:.3} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3} \
             cpu_p50_ms={cpu_p50:.3} cpu_p95_ms={cpu_p95:.3} \
             render_p50_ms={render_p50:.3} render_p95_ms={render_p95:.3} \
             {gpu_columns}{stress}{memory}quality={quality} extent={extent} gpu=\"{gpu}\"\n",
            samples.len(),
            self.frames_dropped,
            percentile(0.50),
            percentile(0.95),
            percentile(0.99),
            samples[samples.len() - 1],
        );
        let _ = std::fs::write(&path, &report);
        eprintln!("frame log: {}", report.trim_end());
    }
}

/// The renderer's quality, post and pipeline fields of the frame-log summary line.
fn quality_summary(renderer: &moba_render::Renderer) -> String {
    let pipelines = renderer.pipeline_stats();
    let lighting = renderer.lighting_config();
    let post = renderer.post_config();
    let world = renderer.world_config();
    let environment = renderer.environment_config();
    let [render_width, render_height] = renderer.render_extent();
    format!(
        "{} hdr={} shadows={} shadow_size={} aa={} bloom={} ao={} \
             auto_exposure={} grading={} render={render_width}x{render_height} \
             dynres={} ibl={} specular_ibl={} vat_meshes={} terrain={} fow={} fow_size={} \
             fog={} sky={} decals={} foliage={} water={} pipelines={} pipeline_build_ms={:.3} \
             pipeline_cache={} startup_ms={:.1}",
        renderer.quality_tier(),
        renderer.hdr_format_name(),
        lighting.shadows,
        lighting.shadow_resolution,
        post.anti_aliasing,
        on_off(post.bloom),
        on_off(post.ambient_occlusion),
        on_off(post.auto_exposure && renderer.post_look().auto_exposure),
        on_off(post.grading),
        on_off(post.dynamic_resolution),
        if lighting.ibl && renderer.has_environment() {
            "on"
        } else {
            "off"
        },
        if lighting.specular_ibl { "on" } else { "off" },
        renderer.vat_mesh_count(),
        if renderer.has_terrain() {
            world.terrain.name()
        } else {
            "flat"
        },
        world.fog_of_war.name(),
        world.fog_of_war_resolution,
        on_off(world.height_fog),
        on_off(world.sky),
        on_off(environment.decals),
        renderer.foliage_instances(),
        if renderer.has_water() {
            environment.water.name()
        } else {
            "none"
        },
        pipelines.pipelines,
        pipelines.build_ms,
        pipelines.cache,
        pipelines.startup_ms,
    )
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// GPU lanes of the frame log, in sample order (see [`gpu_sample`]).
pub(crate) const GPU_LANES: [&str; 29] = [
    "shadow",
    "scene",
    "composite",
    "ui",
    "total",
    "span",
    "prepass",
    "ao",
    "bloom",
    "exposure",
    "fxaa",
    "upscale",
    "contact_shadows",
    "outline",
    "portraits",
    "minimap",
    "vfx_simulate",
    "vfx_draw",
    "vfx_resolve",
    "vfx_alive",
    "vfx_overdraw",
    "vfx_distortion",
    "lights",
    "fow_raster",
    "fow_resolve",
    "water_copy",
    "water",
    "distortion_copy",
    "distortion",
];
/// First lane of a pass that only some configurations have (R6 post, ADR-0015; R2b's contact
/// shadows and outlines, ADR-0016).
const OPTIONAL_LANES: usize = 6;
/// First lane of a pass that runs only in some frames (R7b offscreen).
const OFFSCREEN_LANES: usize = 14;
/// First lane after the offscreen passes (the VFX passes, ADR-0013).
const VFX_LANES: usize = 16;
/// First lane that is a statistic rather than a pass time (particles, then R2b's lights).
const STAT_LANES: usize = 19;
/// First fog-of-war pass lane (R3a; runs only with the fog-of-war texture). The two full-water
/// lanes after them (R3b, ADR-0017) run only with `MOBA_WATER=full` and a map with water, the
/// two heat-haze lanes (ADR-0019) only on High and up.
const FOG_LANES: usize = 23;

/// One frame-log sample from `timings`, the same frame's particle `stats` and its binned
/// dynamic `lights` (ADR-0016), in [`GPU_LANES`] order (`0` for a pass that did not run).
/// `ao` sums GTAO and its blur, `bloom` every bloom down- and up-sample.
pub(crate) fn gpu_sample(
    timings: &moba_render::GpuTimings,
    stats: Option<moba_render::VfxStats>,
    lights: f32,
) -> [f32; GPU_LANES.len()] {
    let pass = |name| timings.pass_ms(name).unwrap_or(0.0);
    [
        pass("shadow"),
        pass("scene"),
        pass("composite"),
        pass("ui"),
        timings.total_ms,
        timings.span_ms,
        pass("prepass"),
        timings.group_ms("ao"),
        timings.group_ms("bloom"),
        pass("exposure"),
        pass("fxaa"),
        pass("upscale"),
        pass("contact_shadows"),
        pass("outline"),
        pass("portraits"),
        pass("minimap"),
        pass("vfx_simulate"),
        pass("vfx_draw"),
        pass("vfx_resolve"),
        stats.map_or(0.0, |stats| stats.alive as f32),
        stats.map_or(0.0, |stats| stats.overdraw),
        stats.map_or(0.0, |stats| stats.distortion as f32),
        lights,
        pass("fow-raster"),
        pass("fow-resolve"),
        pass("water_copy"),
        pass("water"),
        pass("distortion_copy"),
        pass("distortion"),
    ]
}

/// `gpu_<lane>_p50_ms`/`_p95_ms` columns for the frame log, from [`gpu_sample`] samples
/// (`shadow` is 0 when shadows are off) plus the particle stats `vfx_alive` (live particles),
/// `vfx_overdraw` (projected particle pixels per screen pixel, ADR-0013) and `vfx_distortion`
/// (live heat-haze sprites, ADR-0019) and `lights`
/// (dynamic lights binned, ADR-0016), which get
/// `_p50`/`_p95`/`_max` columns without the unit. `total` sums the passes; `span` runs from
/// the first pass's start to the last pass's end, so `span - total` is the time spent in
/// barriers between passes. The R6 post lanes (`prepass` … `upscale`) and R2b's
/// `contact_shadows` and `outline` are printed only when the configuration has the pass. The offscreen passes (R7b) run only in some frames and the
/// fog-of-war passes (R3a, `fow_raster`, `fow_resolve`) only with the texture, so their
/// percentiles cover just those frames, and `gpu_<pass>_runs` counts them; with no run the
/// columns are left out.
fn gpu_report(samples: &[[f32; GPU_LANES.len()]]) -> String {
    use core::fmt::Write as _;
    let mut columns = String::new();
    for (lane, name) in GPU_LANES.into_iter().enumerate() {
        // The offscreen passes and the fog-of-war passes (R3a, 0 without the texture) are
        // reported over the frames that ran them.
        let offscreen = (OFFSCREEN_LANES..VFX_LANES).contains(&lane) || lane >= FOG_LANES;
        let mut values: Vec<f32> = samples
            .iter()
            .map(|sample| sample[lane])
            .filter(|&value| !offscreen || value > 0.0)
            .collect();
        let optional = (OPTIONAL_LANES..VFX_LANES).contains(&lane) || lane >= FOG_LANES;
        if optional && !values.iter().any(|&value| value > 0.0) {
            continue;
        }
        if offscreen {
            let _ = write!(columns, "gpu_{name}_runs={} ", values.len());
        }
        values.sort_by(f32::total_cmp);
        let at = |fraction: f32| {
            values
                .get(((values.len().max(1) - 1) as f32 * fraction).round() as usize)
                .copied()
                .unwrap_or(0.0)
        };
        if (STAT_LANES..FOG_LANES).contains(&lane) {
            let _ = write!(
                columns,
                "{name}_p50={:.3} {name}_p95={:.3} {name}_max={:.3} ",
                at(0.50),
                at(0.95),
                at(1.0)
            );
        } else {
            let _ = write!(
                columns,
                "gpu_{name}_p50_ms={:.3} gpu_{name}_p95_ms={:.3} ",
                at(0.50),
                at(0.95)
            );
            // R8: the frame-pacing evidence needs the GPU frame's tail too.
            if name == "total" {
                let _ = write!(columns, "gpu_total_p99_ms={:.3} ", at(0.99));
            }
        }
    }
    columns
}

#[cfg(test)]
mod tests {

    #[test]
    fn demo_orders_parse_in_tick_order() {
        use super::{DemoOrder, demo_orders};
        assert_eq!(
            demo_orders("recall@450, stop@360,recall@300,move:118:127@30,e@9,bogus@1,q@x"),
            vec![
                (9, DemoOrder::Cast(2)),
                (30, DemoOrder::Move(moba_core::FixVec2::from_ints(118, 127))),
                (300, DemoOrder::Recall),
                (360, DemoOrder::Stop),
                (450, DemoOrder::Recall),
            ]
        );
        assert_eq!(demo_orders(""), []);
    }
    use super::{
        cull_mode_from, environment_config_from, frame_log_frames_from, gpu_report,
        lighting_config_from, mesh_lod_from, parse_hex32, parse_preview_camera, parse_window_size,
        pipeline_cache_path_from, post_config_from, upload_lock_wanted, world_config_from,
    };

    #[test]
    fn the_upload_lock_arms_only_while_a_match_plays() {
        use moba_net::protocol::MatchPhase;
        assert!(upload_lock_wanted(true, None), "preview and replays");
        assert!(upload_lock_wanted(true, Some(MatchPhase::Playing)));
        assert!(
            !upload_lock_wanted(false, Some(MatchPhase::Playing)),
            "warm-up"
        );
        assert!(!upload_lock_wanted(false, None));
        for phase in [
            MatchPhase::Lobby,
            MatchPhase::Countdown,
            MatchPhase::Results,
        ] {
            assert!(!upload_lock_wanted(true, Some(phase)), "{phase:?}");
        }
    }

    #[test]
    fn r8_switches_parse_with_safe_fallbacks() {
        use moba_render::{CullMode, LodConfig};
        assert_eq!(frame_log_frames_from(None), 10_000);
        assert_eq!(frame_log_frames_from(Some("90000")), 90_000);
        assert_eq!(frame_log_frames_from(Some("5")), 100);
        assert_eq!(frame_log_frames_from(Some("999999999")), 1_000_000);
        assert_eq!(frame_log_frames_from(Some("lots")), 10_000);
        assert_eq!(cull_mode_from(None), CullMode::Cpu);
        assert_eq!(cull_mode_from(Some("off")), CullMode::Off);
        assert_eq!(cull_mode_from(Some("on")), CullMode::Cpu);
        assert_eq!(cull_mode_from(Some("sideways")), CullMode::Cpu);
        assert_eq!(mesh_lod_from(None), None);
        assert_eq!(mesh_lod_from(Some("off")), Some(LodConfig::OFF));
        assert_eq!(
            mesh_lod_from(Some("2")),
            Some(LodConfig::with_tolerance(2.0))
        );
        assert_eq!(mesh_lod_from(Some("-1")), None);
    }

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
    fn lighting_overrides_turn_tier_features_off() {
        use moba_render::{LightingConfig, QualityTier, ShadowFilter};
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let high = LightingConfig::for_tier(QualityTier::High);
        assert_eq!(lighting_config_from(QualityTier::High, env(&[])), high);
        let off = lighting_config_from(
            QualityTier::High,
            env(&[
                ("MOBA_SHADOWS", "OFF"),
                ("MOBA_IBL", "0"),
                ("MOBA_SHADOW_SIZE", "512"),
            ]),
        );
        assert_eq!(off.shadows, ShadowFilter::Off);
        assert!(!off.ibl);
        assert_eq!(off.shadow_resolution, 512);
        let bad = lighting_config_from(
            QualityTier::Low,
            env(&[("MOBA_SHADOWS", "vsm"), ("MOBA_SHADOW_SIZE", "big")]),
        );
        assert_eq!(bad, LightingConfig::for_tier(QualityTier::Low));
        let r2b = lighting_config_from(
            QualityTier::High,
            env(&[
                ("MOBA_LIGHTS", "off"),
                ("MOBA_OUTLINES", "hull"),
                ("MOBA_BAKED_AO", "off"),
                ("MOBA_CONTACT_SHADOWS", "0"),
            ]),
        );
        assert_eq!(r2b.max_lights, 0);
        assert_eq!(r2b.outlines, moba_render::OutlineMode::Hull);
        assert!(!r2b.baked_ao);
        assert_eq!(r2b.contact_shadow_steps, 0);
        let capped = lighting_config_from(
            QualityTier::Low,
            env(&[("MOBA_LIGHTS", "8"), ("MOBA_CONTACT_SHADOWS", "12")]),
        );
        assert_eq!(capped.max_lights, 8);
        assert_eq!(capped.contact_shadow_steps, 12);
        let ignored = lighting_config_from(
            QualityTier::Ultra,
            env(&[
                ("MOBA_LIGHTS", "lots"),
                ("MOBA_OUTLINES", "glow"),
                ("MOBA_BAKED_AO", "1"),
                ("MOBA_CONTACT_SHADOWS", "many"),
            ]),
        );
        assert_eq!(ignored, LightingConfig::for_tier(QualityTier::Ultra));
    }

    #[test]
    fn post_overrides_switch_passes_and_captures_pin_feedback_loops() {
        use moba_render::{AntiAliasing, PostConfig, QualityTier, UpscaleFilter};
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let high = PostConfig::for_tier(QualityTier::High);
        assert_eq!(
            post_config_from(QualityTier::High, 1.0, false, env(&[])),
            high
        );
        let off = post_config_from(
            QualityTier::High,
            1.0,
            false,
            env(&[
                ("MOBA_BLOOM", "off"),
                ("MOBA_AA", "OFF"),
                ("MOBA_AO", "0"),
                ("MOBA_GRADING", "off"),
                ("MOBA_SCREEN_EFFECTS", "off"),
                ("MOBA_EXPOSURE", "fixed"),
            ]),
        );
        assert!(!off.bloom && !off.ambient_occlusion && !off.grading && !off.screen_effects);
        assert!(!off.auto_exposure);
        assert_eq!(off.anti_aliasing, AntiAliasing::Off);
        let scaled = post_config_from(
            QualityTier::Medium,
            0.9,
            false,
            env(&[
                ("MOBA_RENDER_SCALE", "0.6"),
                ("MOBA_DYNRES", "8.5"),
                ("MOBA_UPSCALE", "bilinear"),
                ("MOBA_AA", "msaa4"),
            ]),
        );
        assert_eq!(scaled.render_scale, 0.6);
        assert!(scaled.dynamic_resolution);
        assert_eq!(scaled.dynamic_target_ms, 8.5);
        assert_eq!(scaled.upscale, UpscaleFilter::Bilinear);
        assert_eq!(scaled.anti_aliasing, AntiAliasing::Msaa4);
        // The settings' scale applies without an override, clamped.
        assert_eq!(
            post_config_from(QualityTier::High, 0.1, false, env(&[])).render_scale,
            0.5
        );
        // Captures: Low's dynamic resolution and Medium's auto exposure are pinned off...
        let capture = post_config_from(QualityTier::Low, 1.0, true, env(&[]));
        assert!(!capture.dynamic_resolution && !capture.auto_exposure);
        assert!(
            !post_config_from(
                QualityTier::High,
                1.0,
                true,
                env(&[("MOBA_EXPOSURE", "1.5")])
            )
            .auto_exposure
        );
        // ...unless asked for explicitly.
        let explicit = post_config_from(
            QualityTier::Low,
            1.0,
            true,
            env(&[("MOBA_DYNRES", "on"), ("MOBA_EXPOSURE", "auto")]),
        );
        assert!(explicit.dynamic_resolution && explicit.auto_exposure);
        // Garbage keeps the tier's values.
        let bad = post_config_from(
            QualityTier::Ultra,
            1.0,
            false,
            env(&[
                ("MOBA_AA", "taa"),
                ("MOBA_BLOOM", "maybe"),
                ("MOBA_RENDER_SCALE", "big"),
                ("MOBA_DYNRES", "fast"),
                ("MOBA_EXPOSURE", "bright"),
            ]),
        );
        assert_eq!(bad, PostConfig::for_tier(QualityTier::Ultra));
    }

    #[test]
    fn world_overrides_turn_terrain_fog_and_sky_off() {
        use moba_render::{FogOfWarMode, QualityTier, TerrainShading, WorldConfig};
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let medium = WorldConfig::for_tier(QualityTier::Medium);
        assert_eq!(world_config_from(QualityTier::Medium, env(&[])), medium);
        let off = world_config_from(
            QualityTier::High,
            env(&[
                ("MOBA_TERRAIN", "flat"),
                ("MOBA_FOW", "Legacy"),
                ("MOBA_FOW_SIZE", "128"),
                ("MOBA_FOG", "off"),
                ("MOBA_SKY", "OFF"),
            ]),
        );
        assert_eq!(off.terrain, TerrainShading::Flat);
        assert_eq!(off.fog_of_war, FogOfWarMode::Legacy);
        assert_eq!(off.fog_of_war_resolution, 128);
        assert!(!off.height_fog && !off.sky);
        let bad = world_config_from(
            QualityTier::Low,
            env(&[("MOBA_FOW", "circles"), ("MOBA_TERRAIN", "3d")]),
        );
        assert_eq!(bad, WorldConfig::for_tier(QualityTier::Low));
    }

    #[test]
    fn environment_overrides_turn_decals_foliage_and_water_off() {
        use moba_render::{EnvironmentConfig, QualityTier, WaterMode};
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert_eq!(
            environment_config_from(QualityTier::Low, env(&[])),
            EnvironmentConfig::for_tier(QualityTier::Low)
        );
        let off = environment_config_from(
            QualityTier::High,
            env(&[
                ("MOBA_DECALS", "off"),
                ("MOBA_FOLIAGE", "OFF"),
                ("MOBA_WATER", "simple"),
            ]),
        );
        assert!(!off.decals && !off.foliage);
        assert_eq!(off.water, WaterMode::Simple);
        assert_eq!(super::distortion_switch_from(None), (true, true));
        assert_eq!(super::distortion_switch_from(Some(" OFF ")), (false, true));
        assert_eq!(super::distortion_switch_from(Some("0")), (false, true));
        assert_eq!(super::distortion_switch_from(Some("on")), (true, true));
        assert_eq!(super::distortion_switch_from(Some("NoDraw")), (true, false));
        assert_eq!(super::distortion_switch_from(Some("shimmer")), (true, true));
        let none = environment_config_from(QualityTier::Ultra, env(&[("MOBA_WATER", "off")]));
        assert_eq!(none.water, WaterMode::Off);
        let bad = environment_config_from(QualityTier::Medium, env(&[("MOBA_WATER", "lava")]));
        assert_eq!(bad, EnvironmentConfig::for_tier(QualityTier::Medium));
    }

    #[test]
    fn gpu_report_lists_each_pass_percentile() {
        let samples: Vec<[f32; super::GPU_LANES.len()]> = (0..=100)
            .map(|index| {
                let ms = index as f32 / 100.0;
                // The portrait pass ran in every tenth frame; the minimap never did.
                let portraits = if index % 10 == 0 { 0.02 } else { 0.0 };
                // R6: this configuration has bloom and FXAA but no AO or upscale.
                [
                    ms * 0.5,
                    ms,
                    ms * 2.0,
                    0.5,
                    ms * 3.0,
                    ms * 4.0,
                    0.0,
                    0.0,
                    0.1,
                    0.0,
                    0.03,
                    0.0,
                    // R2b: contact shadows off, outlines on.
                    0.0,
                    0.5,
                    portraits,
                    0.0,
                    ms,
                    ms,
                    0.0,
                    index as f32,
                    ms,
                    if index % 4 == 0 { 3.0 } else { 0.0 },
                    2.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    // ADR-0019: the heat haze ran in every other frame.
                    if index % 2 == 0 { 0.01 } else { 0.0 },
                    if index % 2 == 0 { 0.04 } else { 0.0 },
                ]
            })
            .collect();
        let report = gpu_report(&samples);
        assert!(report.contains("gpu_distortion_copy_runs=51 gpu_distortion_copy_p50_ms=0.010 "));
        assert!(report.ends_with(
            "gpu_distortion_runs=51 gpu_distortion_p50_ms=0.040 gpu_distortion_p95_ms=0.040 "
        ));
        assert!(report.starts_with("gpu_shadow_p50_ms=0.250 gpu_shadow_p95_ms=0.475 "));
        assert!(report.contains("gpu_scene_p50_ms=0.500 gpu_scene_p95_ms=0.950 "));
        assert!(report.contains("gpu_composite_p95_ms=1.900 "));
        assert!(report.contains("gpu_ui_p50_ms=0.500 gpu_ui_p95_ms=0.500 "));
        assert!(report.contains("gpu_total_p50_ms=1.500 gpu_total_p95_ms=2.850 "));
        assert!(report.contains("gpu_span_p50_ms=2.000 gpu_span_p95_ms=3.800 "));
        assert!(report.contains("gpu_bloom_p50_ms=0.100 "));
        assert!(report.contains("gpu_fxaa_p95_ms=0.030 "));
        assert!(!report.contains("gpu_ao") && !report.contains("gpu_upscale"));
        assert!(!report.contains("gpu_contact_shadows"));
        assert!(report.contains("gpu_outline_p50_ms=0.500 gpu_outline_p95_ms=0.500 "));
        // Offscreen passes: percentiles over the frames they ran in, plus a run count.
        assert!(report.contains(
            "gpu_portraits_runs=11 gpu_portraits_p50_ms=0.020 gpu_portraits_p95_ms=0.020 "
        ));
        assert!(!report.contains("gpu_minimap"));
        assert!(!report.contains("gpu_fow"));
        assert!(!report.contains("gpu_water"));
        assert!(report.contains("gpu_vfx_draw_p95_ms=0.950 "));
        assert!(report.contains(
            "vfx_alive_p50=50.000 vfx_alive_p95=95.000 vfx_alive_max=100.000 \
             vfx_overdraw_p50=0.500 vfx_overdraw_p95=0.950 vfx_overdraw_max=1.000 \
             vfx_distortion_p50=0.000 vfx_distortion_p95=3.000 vfx_distortion_max=3.000 \
             lights_p50=2.000 lights_p95=2.000 lights_max=2.000 "
        ));
    }

    #[test]
    fn preview_camera_parses_three_numbers() {
        assert_eq!(
            parse_preview_camera(Some("-0.7, 0.2, 260")),
            Some([-0.7, 0.2, 260.0, 0.0, 0.0])
        );
        assert_eq!(
            parse_preview_camera(Some("1,2,9000")),
            Some([1.0, 2.0, 600.0, 0.0, 0.0])
        );
        // R3b: an optional focus.
        assert_eq!(
            parse_preview_camera(Some("0.5,0.6,40,-3,-30")),
            Some([0.5, 0.6, 40.0, -3.0, -30.0])
        );
        assert_eq!(
            parse_preview_camera(Some("0,0.6,40,900,0")),
            Some([0.0, 0.6, 40.0, 200.0, 0.0])
        );
        for bad in [
            "",
            "1,2",
            "1,2,3,4",
            "1,2,3,4,5,6",
            "a,b,c",
            "nan,1,1",
            "1,1,1,1,inf",
        ] {
            assert_eq!(parse_preview_camera(Some(bad)), None, "{bad:?}");
        }
        assert_eq!(parse_preview_camera(None), None);
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
