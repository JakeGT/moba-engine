//! HUD healthbars.

use super::theme;
use super::{
    Camera, NetworkView, UiDrawList, Vec3, hud_units, map_center, project_to_screen,
    rendered_entity_position, unit_hud_height,
};

pub(crate) fn draw_health_bars(
    ui: &mut UiDrawList,
    network: &NetworkView,
    camera: &Camera,
    width: u32,
    height: u32,
) {
    let render_tick = network.render_tick();
    let center = map_center(network);
    let width_f = width as f32;
    for unit in hud_units(network) {
        if unit.dead() || (unit.kind() != 0 && unit.kind() < 3) {
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
        let bar_width = if unit.kind() == 0 { 46.0 } else { 64.0 };
        let bar_height = if unit.kind() == 0 { 5.0 } else { 7.0 };
        let x = (screen.x - bar_width * 0.5).clamp(4.0, (width_f - bar_width - 4.0).max(4.0));
        let y = (screen.y - 8.0).clamp(4.0, height as f32 - 4.0);
        let fraction = if unit.max_health() == 0 {
            0.0
        } else {
            unit.health() as f32 / unit.max_health() as f32
        };
        let team_color = if unit.team() == 0 {
            theme::BLUE_ACCENT
        } else {
            theme::RED_ACCENT
        };
        ui.rect(x, y, bar_width, bar_height, theme::HEALTHBAR_BG);
        ui.rect(
            x,
            y,
            bar_width * fraction.clamp(0.0, 1.0),
            bar_height,
            theme::HEALTHBAR_ALLY,
        );
        ui.outline(x, y, bar_width, bar_height, 1.0, team_color);
        if unit.kind() == 0 {
            let mut icon_x = x;
            for (active, color) in [
                (
                    unit.status_bits() & crate::hud::STATUS_SHIELD != 0,
                    theme::SHIELD,
                ),
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
                    ui.rect(icon_x, y + bar_height + 2.0, 4.0, 3.0, color);
                    icon_x += 6.0;
                }
            }
        }
        if Some(unit.net_id()) == network.own_net_id {
            ui.rect(x + bar_width * 0.5 - 5.0, y - 5.0, 10.0, 2.0, theme::GOLD);
        }
    }
}
