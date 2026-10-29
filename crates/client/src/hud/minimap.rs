//! HUD minimap.
//!
//! With a baked minimap image (R7b, `app::offscreen`) the panel shows the top-down render of
//! the map's terrain, walls and props, darkened outside allied vision; otherwise a flat
//! ground with wall lines. Lanes, units and the own-hero frame are drawn on top either way.

use arrayvec::ArrayVec;
use moba_core::math::Vec4;
use moba_render::UiImage;

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
    palette: theme::TeamPalette,
    baked: bool,
) {
    let own_team = network.client.team().index() as u8;
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
    if baked {
        ui.image(
            left,
            top,
            inner,
            inner,
            UiImage::MINIMAP,
            [0.0, 0.0, 1.0, 1.0],
            Vec4::ONE,
        );
        if !network.spectator {
            draw_minimap_fog(ui, network, [left, top, inner], map_extent(network));
        }
    } else {
        ui.rect(left, top, inner, inner, theme::MINIMAP_GROUND);
    }
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
    // The baked image already shows the walls.
    if !baked {
        let wall_color = theme::MINIMAP_WALL;
        map_walls(network, |a, b| {
            let a = map(FixVec2::from_ints(i32::from(a[0]), i32::from(a[1])));
            let b = map(FixVec2::from_ints(i32::from(b[0]), i32::from(b[1])));
            ui.line(a.x, a.y, b.x, b.y, 2.0, wall_color);
        });
    }
    for unit in hud_units(network) {
        if unit.dead() {
            continue;
        }
        let point = map(unit.position());
        // Allies and enemies in the configured palette (the gold frame marks the own hero).
        let team_color = palette.relation(unit.team(), own_team, false);
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

/// Fog cells per minimap side.
const FOG_GRID: usize = 24;
/// Allied vision sources the fog considers (more are ignored).
const FOG_SOURCES: usize = 160;
const FOG_COLOR: Vec4 = Vec4::new(0.01, 0.015, 0.03, 0.55);

/// Darkens the minimap outside allied vision: a `FOG_GRID` grid whose cell centres no allied
/// unit's vision circle covers, one rectangle per horizontal run.
///
/// Presentation only, from units the client already has: the server withholds what the team
/// cannot see, so this never reveals anything (and, like the HUD markers, it ignores the
/// vision-blocking walls the terrain fog traces). Never allocates.
pub(crate) fn draw_minimap_fog(
    ui: &mut UiDrawList,
    network: &NetworkView,
    [left, top, size]: [f32; 3],
    extent: f32,
) {
    let own_team = network.client.team().index() as u8;
    let mut sources: ArrayVec<(f32, f32, f32), FOG_SOURCES> = ArrayVec::new();
    for unit in hud_units(network) {
        if unit.team() != own_team || unit.dead() {
            continue;
        }
        let radius = vision_radius(network, unit);
        let position = unit.position();
        if radius > 0.0
            && sources
                .try_push((position.x.to_f32(), position.y.to_f32(), radius))
                .is_err()
        {
            break;
        }
    }
    fog_runs(&sources, extent, |row, first, count| {
        let cell = size / FOG_GRID as f32;
        ui.rect(
            left + first as f32 * cell,
            top + row as f32 * cell,
            count as f32 * cell,
            cell,
            FOG_COLOR,
        );
    });
}

/// Calls `run(row, first column, length)` for every horizontal run of fogged cells: cells
/// whose centre (in map units, `extent` across) no `(x, y, radius)` source covers.
fn fog_runs(sources: &[(f32, f32, f32)], extent: f32, mut run: impl FnMut(usize, usize, usize)) {
    let cell = extent / FOG_GRID as f32;
    for row in 0..FOG_GRID {
        let y = (row as f32 + 0.5) * cell;
        // Sources whose circle reaches this row's centre line.
        let mut near: ArrayVec<(f32, f32, f32), FOG_SOURCES> = ArrayVec::new();
        for &(sx, sy, radius) in sources {
            if (sy - y).abs() <= radius {
                near.push((sx, sy, radius * radius));
            }
        }
        let mut start = None;
        for column in 0..=FOG_GRID {
            let fogged = column < FOG_GRID && {
                let x = (column as f32 + 0.5) * cell;
                !near.iter().any(|&(sx, sy, radius_sq)| {
                    let (dx, dy) = (x - sx, y - sy);
                    dx * dx + dy * dy <= radius_sq
                })
            };
            match (fogged, start) {
                (true, None) => start = Some(column),
                (false, Some(first)) => {
                    run(row, first, column - first);
                    start = None;
                }
                _ => {}
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{FOG_GRID, fog_runs};

    #[test]
    fn fog_covers_everything_outside_the_vision_circles() {
        let mut runs = Vec::new();
        fog_runs(&[], 240.0, |row, first, count| {
            runs.push((row, first, count));
        });
        assert_eq!(runs.len(), FOG_GRID, "one full run per row with no vision");
        assert!(
            runs.iter()
                .all(|&(_, first, count)| first == 0 && count == FOG_GRID)
        );

        // One circle in the middle of a 240-unit map (10-unit cells) clears a hole.
        let mut fogged = [[false; FOG_GRID]; FOG_GRID];
        fog_runs(&[(120.0, 120.0, 30.0)], 240.0, |row, first, count| {
            fogged[row][first..first + count].fill(true);
        });
        assert!(!fogged[12][12] && !fogged[11][11], "the centre is visible");
        assert!(fogged[12][16], "45 units away is fogged");
        assert!(fogged[0][0] && fogged[23][23]);
        let clear = fogged.iter().flatten().filter(|cell| !**cell).count();
        // About pi * 3^2 cells of a 10-unit grid.
        assert!((24..=32).contains(&clear), "{clear} visible cells");
    }
}
