//! HUD healthbars.

use super::theme;
use super::{
    Camera, Instant, NetworkView, UiDrawList, Vec3, Vec4, project_to_screen,
    rendered_entity_position, unit_hud_height,
};

pub(crate) fn draw_health_bars(
    ui: &mut UiDrawList,
    network: &NetworkView,
    camera: &Camera,
    width: u32,
    height: u32,
) {
    let Some(snapshot) = network.client.snapshot() else {
        return;
    };
    let render_tick = network.interpolator.render_tick(Instant::now());
    let width_f = width as f32;
    for entity in &snapshot.entities {
        if entity.is_dead() || (entity.kind != 0 && entity.kind < 3) {
            continue;
        }
        let position = rendered_entity_position(network, entity, render_tick);
        let world = Vec3::new(
            position.x.to_f32() - 128.0,
            unit_hud_height(entity.kind),
            position.y.to_f32() - 128.0,
        );
        let Some(screen) = project_to_screen(camera, world, width, height) else {
            continue;
        };
        let bar_width = if entity.kind == 0 { 46.0 } else { 64.0 };
        let bar_height = if entity.kind == 0 { 5.0 } else { 7.0 };
        let x = (screen.x - bar_width * 0.5).clamp(4.0, (width_f - bar_width - 4.0).max(4.0));
        let y = (screen.y - 8.0).clamp(4.0, height as f32 - 4.0);
        let fraction = if entity.max_health == 0 {
            0.0
        } else {
            f32::from(entity.health) / f32::from(entity.max_health)
        };
        let team_color = if entity.team == 0 {
            theme::BLUE_ACCENT
        } else {
            theme::RED_ACCENT
        };
        ui.rect(
            x,
            y,
            bar_width,
            bar_height,
            Vec4::new(0.02, 0.03, 0.04, 0.85),
        );
        ui.rect(
            x,
            y,
            bar_width * fraction.clamp(0.0, 1.0),
            bar_height,
            Vec4::new(0.2, 0.85, 0.38, 1.0),
        );
        ui.outline(x, y, bar_width, bar_height, 1.0, team_color);
        if entity.kind == 0 {
            let mut icon_x = x;
            for (flag, color) in [
                (
                    moba_net::protocol::ENTITY_FLAG_SHIELD,
                    Vec4::new(0.3, 0.75, 1.0, 1.0),
                ),
                (
                    moba_net::protocol::ENTITY_FLAG_SLOW,
                    Vec4::new(0.6, 0.4, 0.95, 1.0),
                ),
                (
                    moba_net::protocol::ENTITY_FLAG_ROOT,
                    Vec4::new(0.95, 0.8, 0.25, 1.0),
                ),
                (
                    moba_net::protocol::ENTITY_FLAG_SILENCE,
                    Vec4::new(0.7, 0.7, 0.75, 1.0),
                ),
                (
                    moba_net::protocol::ENTITY_FLAG_BURN,
                    Vec4::new(1.0, 0.45, 0.12, 1.0),
                ),
                (
                    moba_net::protocol::ENTITY_FLAG_HASTE,
                    Vec4::new(0.3, 0.95, 0.4, 1.0),
                ),
            ] {
                if entity.flags & flag != 0 {
                    ui.rect(icon_x, y + bar_height + 2.0, 4.0, 3.0, color);
                    icon_x += 6.0;
                }
            }
        }
        if Some(entity.net_id) == network.own_net_id {
            ui.rect(x + bar_width * 0.5 - 5.0, y - 5.0, 10.0, 2.0, theme::GOLD);
        }
    }
}
