//! HUD status.

use super::{Camera, FixVec2, UiDrawList, Vec2, Vec3, Vec4, project_to_screen};
use std::sync::OnceLock;

use crate::hud_fmt;

static CAPTURE_ACTIVE: OnceLock<bool> = OnceLock::new();

fn capture_active() -> bool {
    *CAPTURE_ACTIVE.get_or_init(|| std::env::var_os("MOBA_CAPTURE").is_some())
}

pub(crate) fn draw_world_circle(
    ui: &mut UiDrawList,
    camera: &Camera,
    center: FixVec2,
    radius: f32,
    width: u32,
    height: u32,
    color: Vec4,
) {
    let segments = 64;
    let mut previous: Option<Vec2> = None;
    for index in 0..=segments {
        let angle = std::f32::consts::TAU * index as f32 / segments as f32;
        let world = Vec3::new(
            center.x.to_f32() + angle.cos() * radius,
            center.y.to_f32() + angle.sin() * radius,
            0.08,
        );
        let current = project_to_screen(camera, world, width, height);
        if let (Some(from), Some(to)) = (previous, current) {
            ui.line(from.x, from.y, to.x, to.y, 1.5, color);
        }
        previous = current;
    }
}

pub(crate) fn draw_perf(ui: &mut UiDrawList, width: f32, frame_ms: f32, fps: f32) {
    // Capture/golden images must not depend on wall-clock frame timing.
    if capture_active() {
        return;
    }
    let truncated = moba_render::ui_vertices_truncated();
    let label = if truncated == 0 {
        hud_fmt!("{fps:.0} FPS   {frame_ms:.1} MS")
    } else {
        hud_fmt!("{fps:.0} FPS   {frame_ms:.1} MS   UI TRUNC {truncated}")
    };
    ui.text(
        width - 24.0 - UiDrawList::text_width(&label, 1.8),
        54.0,
        1.8,
        Vec4::new(0.55, 0.62, 0.72, 1.0),
        &label,
    );
}
