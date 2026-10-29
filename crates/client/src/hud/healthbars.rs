//! World-space health bars (R7a styling): team/palette fill, damage ghost, shield segment,
//! tick marks, hero level and mana.

use moba_core::math::Vec4;

use super::fx::{HudFx, has_health_bar};
use super::theme;
use super::{
    Camera, NetworkView, UiDrawList, Vec3, hud_units, map_center, project_to_screen,
    rendered_entity_position, unit_hud_height,
};
use crate::hud_fmt;

/// Health per minor tick mark on hero bars; every tenth tick is drawn stronger.
pub(crate) const HERO_TICK_HEALTH: i32 = 100;
/// Health per tick mark on structure bars.
pub(crate) const STRUCTURE_TICK_HEALTH: i32 = 500;
/// Nominal shield segment (fraction of maximum health): snapshots carry only the shield bit.
pub(crate) const SHIELD_NOMINAL: f32 = 0.12;

/// Horizontal fill fractions of one bar, scaled so health + shield fits the bar.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct BarLayout {
    pub(crate) health: f32,
    /// End of the ghost segment (>= `health`).
    pub(crate) ghost: f32,
    /// End of the shield segment (>= `health`).
    pub(crate) shield: f32,
    /// Pixels between tick marks (0 = none).
    pub(crate) tick_spacing: f32,
}

/// Lays out one bar of `width` pixels.
pub(crate) fn bar_layout(
    health: i32,
    max_health: i32,
    ghost: f32,
    shielded: bool,
    tick_health: i32,
    width: f32,
) -> BarLayout {
    let max = max_health.max(1) as f32;
    let health = health.clamp(0, max_health.max(1)) as f32;
    let shield = if shielded { SHIELD_NOMINAL * max } else { 0.0 };
    // When the shield would overflow, the whole bar rescales (genre convention).
    let total = max.max(health + shield);
    let tick_spacing = if tick_health > 0 {
        let spacing = width * tick_health as f32 / total;
        if spacing >= 3.0 { spacing } else { 0.0 }
    } else {
        0.0
    };
    BarLayout {
        health: health / total,
        ghost: ghost.clamp(health, max) / total,
        shield: (health + shield) / total,
        tick_spacing,
    }
}

#[allow(clippy::too_many_lines)] // One bar: frame, segments, ticks, mana, badge, pips.
pub(crate) fn draw_health_bars(
    ui: &mut UiDrawList,
    network: &NetworkView,
    camera: &Camera,
    width: u32,
    height: u32,
    fx: &HudFx,
) {
    let render_tick = network.render_tick();
    let center = map_center(network);
    let width_f = width as f32;
    let own_team = network.client.team().index() as u8;
    for unit in hud_units(network) {
        if unit.dead() || !has_health_bar(unit.kind()) {
            continue;
        }
        let position = rendered_entity_position(network, unit, render_tick);
        let world = Vec3::new(
            position.x.to_f32() - center,
            unit_hud_height(unit.kind()),
            position.y.to_f32() - center,
        );
        let Some(screen) = project_to_screen(camera, world, width, height) else {
            continue;
        };
        // Units just off screen keep an edge-clamped bar; farther ones are culled instead of
        // piling up along the border.
        if !(-48.0..=width_f + 48.0).contains(&screen.x)
            || !(-48.0..=height as f32 + 48.0).contains(&screen.y)
        {
            continue;
        }
        let hero = unit.kind() == 0;
        let own = Some(unit.net_id()) == network.own_net_id;
        let bar_width = if hero { 72.0 } else { 88.0 };
        let bar_height = if hero { 8.0 } else { 7.0 };
        let x = (screen.x - bar_width * 0.5).clamp(18.0, (width_f - bar_width - 4.0).max(18.0));
        let y = (screen.y - 10.0).clamp(6.0, height as f32 - 12.0);
        // Health and ghost at the render tick, in step with the interpolated units and the
        // combat numbers (the newest snapshot leads them by the interpolation delay).
        let track = fx.track(unit.net_id());
        let health = track.map_or(unit.health(), |track| track.health_at(render_tick));
        let ghost = track.map_or(health as f32, |track| track.ghost_at(render_tick));
        let shielded = unit.status_bits() & crate::hud::STATUS_SHIELD != 0;
        let layout = bar_layout(
            health,
            unit.max_health(),
            ghost,
            shielded,
            if hero {
                HERO_TICK_HEALTH
            } else {
                STRUCTURE_TICK_HEALTH
            },
            bar_width,
        );
        let fill = fx.palette.relation(unit.team(), own_team, own);
        let mana_height = if hero && unit.max_mana() > 0 {
            3.0
        } else {
            0.0
        };
        let total_height = bar_height + mana_height;

        // Frame and track.
        ui.rect(
            x - 1.0,
            y - 1.0,
            bar_width + 2.0,
            total_height + 2.0,
            theme::HEALTHBAR_FRAME,
        );
        ui.rect(x, y, bar_width, bar_height, theme::HEALTHBAR_BG);
        // Ghost, shield, then the health fill on top (lighter top edge for depth).
        let health_px = bar_width * layout.health;
        ui.rect(
            x + health_px,
            y,
            bar_width * (layout.ghost - layout.health),
            bar_height,
            theme::HEALTH_GHOST,
        );
        ui.rect(
            x + health_px,
            y,
            bar_width * (layout.shield - layout.health),
            bar_height,
            theme::HEALTH_SHIELD,
        );
        ui.rect_gradient(x, y, health_px, bar_height, lighten(fill, 0.25), fill);
        if layout.tick_spacing > 0.0 {
            let ticks_per_major = if hero { 10 } else { 0 };
            let mut index = 1;
            let mut tick_x = layout.tick_spacing;
            while tick_x < bar_width - 0.5 {
                let major = ticks_per_major > 0 && index % ticks_per_major == 0;
                let tick_height = if major { bar_height } else { bar_height * 0.55 };
                ui.rect(
                    x + tick_x - if major { 1.0 } else { 0.5 },
                    y,
                    if major { 2.0 } else { 1.0 },
                    tick_height,
                    theme::HEALTHBAR_TICK,
                );
                tick_x += layout.tick_spacing;
                index += 1;
            }
        }
        if mana_height > 0.0 {
            let mana = unit.mana() as f32 / unit.max_mana().max(1) as f32;
            ui.rect(
                x,
                y + bar_height,
                bar_width,
                mana_height,
                theme::HEALTHBAR_BG,
            );
            ui.rect(
                x,
                y + bar_height + 1.0,
                bar_width * mana.clamp(0.0, 1.0),
                mana_height - 1.0,
                theme::HEALTHBAR_MANA,
            );
        }
        if hero {
            // Level badge on the left.
            let badge = total_height + 2.0;
            ui.rect(
                x - badge - 1.0,
                y - 1.0,
                badge,
                badge,
                theme::HEALTHBAR_FRAME,
            );
            ui.rect(x - badge, y, badge - 2.0, badge - 2.0, darken(fill, 0.55));
            let level = hud_fmt!("{}", unit.level().max(1));
            let scale = 1.15;
            ui.text(
                x - badge - 1.0 + (badge - UiDrawList::text_width(&level, scale)) * 0.5,
                y + (badge - 2.0 - 7.0 * scale) * 0.5,
                scale,
                theme::TEXT,
                &level,
            );
            let mut icon_x = x;
            for (active, color) in [
                (shielded, theme::SHIELD),
                (
                    unit.status_bits() & crate::hud::STATUS_SLOW != 0,
                    theme::SLOW,
                ),
                (
                    unit.status_bits() & crate::hud::STATUS_ROOT != 0,
                    theme::HASTE,
                ),
                (
                    unit.status_bits() & crate::hud::STATUS_SILENCE != 0,
                    theme::ROOT,
                ),
                (
                    unit.status_bits() & crate::hud::STATUS_BURN != 0,
                    theme::BURN,
                ),
                (
                    unit.status_bits() & crate::hud::STATUS_HASTE != 0,
                    theme::REGEN,
                ),
            ] {
                if active {
                    ui.rect(icon_x, y + total_height + 3.0, 5.0, 3.0, color);
                    icon_x += 7.0;
                }
            }
        }
        if own {
            // Gold chevron above the own hero.
            let cx = x + bar_width * 0.5;
            ui.line(cx - 5.0, y - 8.0, cx, y - 4.0, 2.0, theme::GOLD);
            ui.line(cx, y - 4.0, cx + 5.0, y - 8.0, 2.0, theme::GOLD);
        }
    }
}

fn lighten(color: Vec4, amount: f32) -> Vec4 {
    Vec4::new(
        color.x + (1.0 - color.x) * amount,
        color.y + (1.0 - color.y) * amount,
        color.z + (1.0 - color.z) * amount,
        color.w,
    )
}

fn darken(color: Vec4, amount: f32) -> Vec4 {
    Vec4::new(
        color.x * (1.0 - amount),
        color.y * (1.0 - amount),
        color.z * (1.0 - amount),
        0.95,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_orders_health_ghost_and_shield() {
        let layout = bar_layout(500, 1000, 800.0, false, HERO_TICK_HEALTH, 100.0);
        assert_eq!(layout.health, 0.5);
        assert_eq!(layout.ghost, 0.8);
        assert_eq!(layout.shield, 0.5);
        assert_eq!(layout.tick_spacing, 10.0);
    }

    #[test]
    fn a_shield_on_full_health_rescales_the_bar() {
        let layout = bar_layout(1000, 1000, 1000.0, true, HERO_TICK_HEALTH, 100.0);
        assert!(layout.health < 1.0 && (layout.shield - 1.0).abs() < 1e-6);
        let half = bar_layout(500, 1000, 500.0, true, 0, 100.0);
        assert_eq!(half.health, 0.5);
        assert!((half.shield - 0.62).abs() < 1e-6);
        assert_eq!(half.tick_spacing, 0.0);
    }

    #[test]
    fn dense_ticks_are_dropped_and_values_clamp() {
        let layout = bar_layout(20_000, 20_000, 0.0, false, HERO_TICK_HEALTH, 72.0);
        assert_eq!(layout.tick_spacing, 0.0, "a tick every 0.36 px is noise");
        let clamped = bar_layout(-5, 100, 500.0, false, 0, 50.0);
        assert_eq!((clamped.health, clamped.ghost), (0.0, 1.0));
    }
}
