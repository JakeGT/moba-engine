//! HUD minimap.

use super::theme;
use super::{
    Fix, FixVec2, HitList, HudAction, MELEE_MINION, NEXUS, NetworkView, RANGED_MINION, TOWER,
    UiDrawList, Vec2, Vec4, VisionCircles, dequantize_position, register,
};

pub(crate) fn draw_minimap(
    ui: &mut UiDrawList,
    network: &NetworkView,
    width: u32,
    height: u32,
    hits: &mut HitList,
) {
    let Some(snapshot) = network.client.snapshot() else {
        return;
    };
    let size = 188.0;
    let margin = 24.0;
    let x = width as f32 - size - margin;
    let y = height as f32 - size - margin;
    ui.panel(
        x,
        y,
        size,
        size,
        Vec4::new(0.01, 0.02, 0.035, 0.82),
        theme::PANEL_CHROME,
    );
    register(hits, HudAction::Block, x, y, size, size);
    let inner = size - 20.0;
    let left = x + 10.0;
    let top = y + 10.0;
    ui.rect(left, top, inner, inner, Vec4::new(0.04, 0.08, 0.06, 0.9));
    let map = |position: FixVec2| {
        Vec2::new(
            left + position.x.to_f32() / 256.0 * inner,
            top + position.y.to_f32() / 256.0 * inner,
        )
    };
    let lane_color = Vec4::new(0.22, 0.38, 0.28, 0.85);
    let wire_map = network.client.map();
    for lane in wire_map.lanes.iter().take(usize::from(wire_map.lane_count)) {
        let count = usize::from(lane.point_count).min(lane.points.len());
        for pair in lane.points[..count].windows(2) {
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
    }
    let wall_color = Vec4::new(0.55, 0.42, 0.28, 1.0);
    for wall in wire_map.walls.iter().take(usize::from(wire_map.wall_count)) {
        let a = map(FixVec2::from_ints(
            i32::from(wall.a[0]),
            i32::from(wall.a[1]),
        ));
        let b = map(FixVec2::from_ints(
            i32::from(wall.b[0]),
            i32::from(wall.b[1]),
        ));
        ui.line(a.x, a.y, b.x, b.y, 2.0, wall_color);
    }
    for entity in &snapshot.entities {
        if entity.is_dead() {
            continue;
        }
        let position = dequantize_position(entity.x, entity.y);
        let point = map(position);
        let team_color = if entity.team == 0 {
            theme::BLUE_ACCENT
        } else {
            theme::RED_ACCENT
        };
        let dot = if entity.kind == 0 || entity.kind >= 3 {
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
        if Some(entity.net_id) == network.own_net_id {
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
    let wire_map = network.client.map();
    for wall in wire_map.walls.iter().take(usize::from(wire_map.wall_count)) {
        vision.add_wall(
            f32::from(wall.a[0]) - 128.0,
            f32::from(wall.a[1]) - 128.0,
            f32::from(wall.b[0]) - 128.0,
            f32::from(wall.b[1]) - 128.0,
        );
    }
    let Some(snapshot) = network.client.snapshot() else {
        return vision;
    };
    let own_team = network.client.team().index() as u8;
    let add = |entity: &moba_net::NetEntity, vision: &mut VisionCircles| {
        if entity.team != own_team || entity.is_dead() {
            return;
        }
        let radius = vision_radius(network, entity);
        let position = dequantize_position(entity.x, entity.y);
        vision.add(
            position.x.to_f32() - 128.0,
            position.y.to_f32() - 128.0,
            radius,
        );
    };
    // Heroes and structures are the most useful fog anchors; minions fill the rest.
    for priority in [0u8, 1, 2] {
        for entity in &snapshot.entities {
            let kind_priority = match entity.kind {
                0 => 0,
                3 | 4 => 1,
                _ => 2,
            };
            if kind_priority == priority {
                add(entity, &mut vision);
            }
        }
    }
    vision
}

pub(crate) fn vision_radius(network: &NetworkView, entity: &moba_net::NetEntity) -> f32 {
    match entity.kind {
        0 => network
            .client
            .heroes()
            .get(usize::from(entity.hero_pick))
            .map_or(35.0, |hero| Fix::from_bits(hero.vision_bits).to_f32()),
        1 => MELEE_MINION.vision_range.to_f32(),
        2 => RANGED_MINION.vision_range.to_f32(),
        3 => TOWER.vision_range.to_f32(),
        4 => NEXUS.vision_range.to_f32(),
        _ => 0.0,
    }
}
