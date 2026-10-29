//! Floating combat text, the loading screen and screen transitions (R7a).

use moba_core::math::Vec4;

use super::fx::{CombatKind, HudFx};
use super::theme;
use super::{
    Camera, NetworkView, UiDrawList, Vec3, map_center, project_to_screen, unit_hud_height,
};
use crate::hud_fmt;

/// Draws the visible combat numbers over the scene.
pub(crate) fn draw_combat_text(
    ui: &mut UiDrawList,
    network: &NetworkView,
    camera: &Camera,
    width: u32,
    height: u32,
    fx: &HudFx,
) {
    let render_tick = network.render_tick();
    let center = map_center(network);
    // The newest `MAX_DRAWN_TEXTS`, oldest first so the newest draw on top.
    for (text, frame) in fx.texts(render_tick) {
        let world = Vec3::new(
            text.world[0] - center,
            unit_hud_height(text.unit_kind),
            text.world[1] - center,
        );
        let Some(screen) = project_to_screen(camera, world, width, height) else {
            continue;
        };
        // Off-screen numbers cost vertices for nothing.
        if !(-64.0..=width as f32 + 64.0).contains(&screen.x)
            || !(-64.0..=height as f32 + 64.0).contains(&screen.y)
        {
            continue;
        }
        let (label, color, base) = match text.kind {
            CombatKind::Damage if text.crit => {
                (hud_fmt!("{}!", text.amount), theme::CRIT_TEXT, 2.3)
            }
            CombatKind::Damage if text.friendly => {
                (hud_fmt!("{}", text.amount), theme::DAMAGE_TEXT, 1.8)
            }
            CombatKind::Damage => (hud_fmt!("{}", text.amount), theme::TEXT, 1.8),
            CombatKind::Heal => (hud_fmt!("+{}", text.amount), theme::HEAL_TEXT, 1.7),
            CombatKind::Gold => (hud_fmt!("+{}G", text.amount), theme::GOLD_TEXT, 1.9),
        };
        let scale = base * frame.scale;
        let label_width = UiDrawList::text_width(&label, scale);
        let x = screen.x - label_width * 0.5 + f32::from(text.lane) * 14.0;
        // Spawn above the health bar.
        let y = screen.y - 30.0 - frame.rise - 7.0 * scale * 0.5;
        let fill = Vec4::new(color.x, color.y, color.z, color.w * frame.alpha);
        let style = moba_render::TextStyle::new(fill)
            .with_outline(Vec4::new(0.02, 0.01, 0.0, 0.92 * frame.alpha), 0.07)
            .with_shadow(
                Vec4::new(0.0, 0.0, 0.0, 0.45 * frame.alpha),
                [0.04, 0.08],
                0.08,
            );
        ui.text_styled(x, y, scale, &style, &label);
    }
}

/// Full-screen stylised loading screen at `alpha` (procedural: gradient, light bands,
/// title, spinner and tip). `clock` animates the spinner.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // One procedural screen.
pub(crate) fn draw_loading_screen(
    ui: &mut UiDrawList,
    width: f32,
    height: f32,
    alpha: f32,
    clock: f32,
    subtitle: &str,
    status: &str,
) {
    if alpha <= 0.0 {
        return;
    }
    let fade = |color: Vec4| Vec4::new(color.x, color.y, color.z, color.w * alpha);
    ui.rect_gradient(
        0.0,
        0.0,
        width,
        height,
        fade(theme::LOADING_TOP),
        fade(theme::LOADING_BOTTOM),
    );
    // Diagonal light bands.
    for band in 0..7 {
        let offset = band as f32 * width * 0.19 - width * 0.1;
        let band_alpha = 0.035 + 0.015 * (band % 3) as f32;
        ui.line(
            offset,
            height,
            offset + height * 0.6,
            0.0,
            26.0 + 14.0 * (band % 2) as f32,
            fade(Vec4::new(0.35, 0.55, 1.0, band_alpha)),
        );
    }
    // Top and bottom vignette.
    let edge = height * 0.22;
    ui.rect_gradient(
        0.0,
        0.0,
        width,
        edge,
        fade(Vec4::new(0.0, 0.0, 0.0, 0.55)),
        Vec4::ZERO,
    );
    ui.rect_gradient(
        0.0,
        height - edge,
        width,
        edge,
        Vec4::ZERO,
        fade(Vec4::new(0.0, 0.0, 0.0, 0.6)),
    );

    let title = "MOBA ENGINE";
    let title_scale = (width / 1280.0 * 9.0).clamp(5.0, 12.0);
    let title_width = UiDrawList::text_width(title, title_scale);
    let title_y = height * 0.36;
    // Glow, then the outlined title.
    let glow = moba_render::TextStyle::new(Vec4::ZERO).with_shadow(
        fade(Vec4::new(0.95, 0.7, 0.25, 0.35)),
        [0.0, 0.0],
        0.09,
    );
    ui.text_styled(
        (width - title_width) * 0.5,
        title_y,
        title_scale,
        &glow,
        title,
    );
    let style = moba_render::TextStyle::new(fade(theme::HERO_NAME))
        .with_outline(fade(Vec4::new(0.12, 0.06, 0.0, 0.95)), 0.05)
        .with_shadow(fade(Vec4::new(0.0, 0.0, 0.0, 0.6)), [0.03, 0.06], 0.05);
    ui.text_styled(
        (width - title_width) * 0.5,
        title_y,
        title_scale,
        &style,
        title,
    );
    let rule_width = title_width * 0.8;
    let rule_y = title_y + 7.0 * title_scale + 22.0;
    ui.rect(
        (width - rule_width) * 0.5,
        rule_y,
        rule_width,
        2.0,
        fade(Vec4::new(0.95, 0.78, 0.3, 0.7)),
    );
    let subtitle_scale = 2.6;
    ui.text_shadowed(
        (width - UiDrawList::text_width(subtitle, subtitle_scale)) * 0.5,
        rule_y + 14.0,
        subtitle_scale,
        fade(theme::GOLD),
        subtitle,
    );

    // Spinner: a faint track and a filling arc.
    let cx = width * 0.5;
    let cy = rule_y + 100.0;
    ui.ring(cx, cy, 22.0, 4.0, 1.0, fade(Vec4::new(1.0, 1.0, 1.0, 0.12)));
    let fill = (clock * 0.8).fract();
    ui.ring(cx, cy, 22.0, 4.0, 0.1 + 0.9 * fill, fade(theme::GOLD));
    let status_scale = 2.2;
    ui.text_shadowed(
        (width - UiDrawList::text_width(status, status_scale)) * 0.5,
        cy + 40.0,
        status_scale,
        fade(theme::TEXT),
        status,
    );
    let tip = "TIP: RIGHT-CLICK TO MOVE, LEFT-CLICK TO ATTACK, Q/W/E TO CAST";
    ui.text(
        (width - UiDrawList::text_width(tip, 1.8)) * 0.5,
        height - 48.0,
        1.8,
        fade(theme::MUTED),
        tip,
    );
}

/// The dip-to-dark overlay between screens.
pub(crate) fn draw_transition(ui: &mut UiDrawList, width: f32, height: f32, fx: &HudFx) {
    let dip = fx.screen.dip();
    if dip > 0.0 {
        let color = theme::TRANSITION;
        ui.rect(
            0.0,
            0.0,
            width,
            height,
            Vec4::new(color.x, color.y, color.z, dip),
        );
    }
}

/// Multiplies a colour's alpha (screen fade-ins).
pub(crate) fn faded(color: Vec4, alpha: f32) -> Vec4 {
    Vec4::new(color.x, color.y, color.z, color.w * alpha)
}

#[cfg(test)]
mod tests {
    use super::{Camera, HudFx, UiDrawList, Vec3, draw_combat_text, map_center};
    use crate::app::NetworkView;
    use crate::hud::fx::{CombatText, MAX_DRAWN_TEXTS, TEXT_CAPACITY};
    use crate::hud::theme::TeamPalette;

    #[test]
    fn teamfight_combat_text_stays_within_a_vertex_budget_without_allocating() {
        let network = NetworkView::test_stub();
        let camera = Camera::perspective(
            Vec3::new(0.0, 30.0, 30.0),
            Vec3::ZERO,
            Vec3::new(0.0, 1.0, 0.0),
            0.8,
            16.0 / 9.0,
            0.1,
            200.0,
        );
        let center = map_center(&network);
        let mut fx = HudFx::new(TeamPalette::Standard);
        // A full ring of four-digit crits on screen (the stub's render tick is 0).
        for index in 0..TEXT_CAPACITY {
            fx.spawn_text(CombatText {
                amount: 9_000 + index as u32,
                crit: true,
                world: [center + (index % 7) as f32, center],
                ..CombatText::default()
            });
        }
        let mut ui = UiDrawList::new();
        draw_combat_text(&mut ui, &network, &camera, 1280, 720, &fx);
        let warm = ui.len();
        // Each drawn text is a shadow run and a fill run of up to five glyphs ("9319!").
        assert!(
            warm > 0 && warm <= MAX_DRAWN_TEXTS * 2 * 5 * 6,
            "{warm} vertices"
        );
        let ((), allocations) = crate::app::allocation_probe::probe(|| {
            ui.clear();
            draw_combat_text(&mut ui, &network, &camera, 1280, 720, &fx);
        });
        assert_eq!(allocations, 0, "steady-state combat-text allocation");
        assert_eq!(ui.len(), warm);
    }
}
