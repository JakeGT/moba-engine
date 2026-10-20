//! HUD minimap.

use super::theme;
use super::{
    Fix, FixVec2, HitList, HudAction, NetworkView, UiDrawList, Vec2, VisionCircles, hud_units,
    map_center, map_extent, map_lanes, map_walls, register,
};

pub(crate) fn draw_minimap(
    ui: &mut UiDrawList,
    network: &NetworkView,
    width: u32,
    height: u32,
    hits: &mut HitList,
) {
    let size = 188.0;
    let margin = 24.0;
    let x = width as f32 - size - margin;
    let y = height as f32 - size - margin;
    ui.panel(x, y, size, size, theme::MINIMAP_BG, theme::PANEL_CHROME);
    register(hits, HudAction::Minimap, x, y, size, size);
    let extent = map_extent(network);
    let inner = size - 20.0;
    let left = x + 10.0;
    let top = y + 10.0;
    ui.rect(left, top, inner, inner, theme::MINIMAP_GROUND);
    let map = |position: FixVec2| {
        Vec2::new(
            left + position.x.to_f32() / extent * inner,
            top + position.y.to_f32() / extent * inner,
        )
    };
    let lane_color = theme::MINIMAP_LANE;
    map_lanes(network, |points| {
        for pair in points.windows(2) {
            let a = map(FixVec2::from_ints(
                i32::from(pair[0][0]),
                i32::from(pair[0][1]),
            ));
            let b = map(FixVec2::from_ints(
                i32::from(pair[1][0]),
                i32::from(pair[1][1]),
            ));
            ui.line(a.x, a.y, b.x, b.y, 1.5, lane_color);
        }
    });
    let wall_color = theme::MINIMAP_WALL;
    map_walls(network, |a, b| {
        let a = map(FixVec2::from_ints(i32::from(a[0]), i32::from(a[1])));
        let b = map(FixVec2::from_ints(i32::from(b[0]), i32::from(b[1])));
        ui.line(a.x, a.y, b.x, b.y, 2.0, wall_color);
    });
    for unit in hud_units(network) {
        if unit.dead() {
            continue;
        }
        let point = map(unit.position());
        let team_color = if unit.team() == 0 {
            theme::BLUE_ACCENT
        } else {
            theme::RED_ACCENT
        };
        let dot = if unit.kind() == 0 || unit.kind() >= 3 {
            5.0
        } else {
            3.0
        };
        ui.rect(
            point.x - dot * 0.5,
            point.y - dot * 0.5,
            dot,
            dot,
            team_color,
        );
        if Some(unit.net_id()) == network.own_net_id {
            ui.outline(point.x - 4.5, point.y - 4.5, 9.0, 9.0, 1.5, theme::GOLD);
        }
    }
    ui.text(x + 10.0, y + size - 14.0, 1.5, theme::MUTED, "MAP");
}

pub(crate) fn build_vision(network: &NetworkView) -> VisionCircles {
    let mut vision = VisionCircles::default();
    if network.spectator {
        return vision;
    }
    let center = map_center(network);
    map_walls(network, |a, b| {
        vision.add_wall(
            f32::from(a[0]) - center,
            f32::from(a[1]) - center,
            f32::from(b[0]) - center,
            f32::from(b[1]) - center,
        );
    });
    let own_team = network.client.team().index() as u8;
    let add = |unit: super::HudUnit<'_>, vision: &mut VisionCircles| {
        if unit.team() != own_team || unit.dead() {
            return;
        }
        let radius = vision_radius(network, unit);
        let position = unit.position();
        vision.add(
            position.x.to_f32() - center,
            position.y.to_f32() - center,
            radius,
        );
    };
    // Heroes and structures are the most useful fog anchors; minions fill the rest.
    for priority in [0u8, 1, 2] {
        for unit in hud_units(network) {
            let kind_priority = match unit.kind() {
                0 => 0,
                3 | 4 => 1,
                _ => 2,
            };
            if kind_priority == priority {
                add(unit, &mut vision);
            }
        }
    }
    vision
}

pub(crate) fn vision_radius(network: &NetworkView, unit: super::HudUnit<'_>) -> f32 {
    match unit.kind() {
        0 => network
            .client
            .heroes()
            .get(usize::from(unit.hero_pick()))
            .map_or(35.0, |hero| Fix::from_bits(hero.vision_bits).to_f32()),
        1 => Fix::from_bits(network.client.rules().melee_minion.vision_range_bits).to_f32(),
        2 => Fix::from_bits(network.client.rules().ranged_minion.vision_range_bits).to_f32(),
        3 => Fix::from_bits(network.client.rules().tower.vision_range_bits).to_f32(),
        4 => Fix::from_bits(network.client.rules().nexus.vision_range_bits).to_f32(),
        _ => 0.0,
    }
}
