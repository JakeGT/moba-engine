//! HUD/layout drawing shared by network and local-preview modes.

use std::time::Instant;

use moba_core::math::{Vec2, Vec3, Vec4};
use moba_core::{Fix, FixVec2};
use moba_game::{
    EMPTY_ITEM, LEVEL_CAP, MELEE_MINION, NEXUS, RANGED_MINION, TOWER, Team, xp_to_next,
};
use moba_net::protocol::{MatchPhase, dequantize_position};
use moba_platform::{InputState, Key};
use moba_render::{Camera, UiDrawList, VisionCircles};
use moba_sim::Simulation;

use crate::app::NetworkView;

pub(crate) fn rendered_entity_position(
    network: &NetworkView,
    entity: &moba_net::NetEntity,
    render_tick: f32,
) -> FixVec2 {
    if Some(entity.net_id) == network.own_net_id {
        network.prediction.position()
    } else {
        network
            .interpolator
            .position(entity.net_id, render_tick)
            .unwrap_or_else(|| dequantize_position(entity.x, entity.y))
    }
}

pub(crate) fn project_to_screen(
    camera: &Camera,
    world: Vec3,
    width: u32,
    height: u32,
) -> Option<Vec2> {
    let clip = camera.view_proj * Vec4::new(world.x, world.y, world.z, 1.0);
    if clip.w <= 0.0001 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !(0.0..=1.0).contains(&ndc.z) {
        return None;
    }
    let x = (ndc.x * 0.5 + 0.5) * width as f32;
    let y = (1.0 - ndc.y) * 0.5 * height as f32;
    Some(Vec2::new(x, y))
}

pub(crate) fn unit_hud_height(kind: u8) -> f32 {
    match kind {
        0 => 6.2,
        1 => 3.2,
        2 => 2.8,
        3 => 10.5,
        4 => 17.0,
        _ => 3.0,
    }
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
            Vec4::new(0.35, 0.62, 1.0, 1.0)
        } else {
            Vec4::new(1.0, 0.42, 0.32, 1.0)
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
            ui.rect(
                x + bar_width * 0.5 - 5.0,
                y - 5.0,
                10.0,
                2.0,
                Vec4::new(0.95, 0.78, 0.3, 1.0),
            );
        }
    }
}

pub(crate) fn draw_minimap(ui: &mut UiDrawList, network: &NetworkView, width: u32, height: u32) {
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
        Vec4::new(0.22, 0.42, 0.58, 0.9),
    );
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
            Vec4::new(0.35, 0.62, 1.0, 1.0)
        } else {
            Vec4::new(1.0, 0.42, 0.32, 1.0)
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
            ui.outline(
                point.x - 4.5,
                point.y - 4.5,
                9.0,
                9.0,
                1.5,
                Vec4::new(0.95, 0.78, 0.3, 1.0),
            );
        }
    }
    ui.text(
        x + 10.0,
        y + size - 14.0,
        1.5,
        Vec4::new(0.55, 0.68, 0.78, 1.0),
        "MAP",
    );
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn draw_perf(ui: &mut UiDrawList, width: f32, frame_ms: f32, fps: f32) {
    // Capture/golden images must not depend on wall-clock frame timing.
    if std::env::var_os("MOBA_CAPTURE").is_some() {
        return;
    }
    let label = format!("{fps:.0} FPS   {frame_ms:.1} MS");
    ui.text(
        width - 24.0 - UiDrawList::text_width(&label, 1.8),
        54.0,
        1.8,
        Vec4::new(0.55, 0.62, 0.72, 1.0),
        &label,
    );
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // HUD composition is one pass.
pub(crate) fn build_network_hud(
    ui: &mut UiDrawList,
    network: &NetworkView,
    scoreboard: bool,
    shop: bool,
    input: &InputState,
    camera: &Camera,
    width: u32,
    height: u32,
    frame_ms: f32,
    fps: f32,
) {
    ui.clear();
    if width == 0 || height == 0 {
        return;
    }
    let bg = Vec4::new(0.015, 0.025, 0.045, 0.82);
    let border = Vec4::new(0.22, 0.42, 0.58, 0.9);
    let text = Vec4::new(0.88, 0.94, 1.0, 1.0);
    let muted = Vec4::new(0.55, 0.68, 0.78, 1.0);
    let health = Vec4::new(0.18, 0.82, 0.36, 1.0);
    let mana = Vec4::new(0.22, 0.50, 1.0, 1.0);
    let track = Vec4::new(0.055, 0.075, 0.095, 1.0);
    let width_f = width as f32;
    let height_f = height as f32;
    draw_perf(ui, width_f, frame_ms, fps);
    for (index, entry) in network.kill_feed.iter().enumerate() {
        ui.text(
            width_f - 280.0,
            12.0 + index as f32 * 14.0,
            1.7,
            Vec4::new(0.92, 0.84, 0.62, 1.0),
            &entry.text,
        );
    }

    draw_health_bars(ui, network, camera, width, height);
    let center = network.prediction.position();
    {
        let hero = network
            .client
            .heroes()
            .get(usize::from(network.client.hero_pick()));
        if let Some(hero) = hero {
            for (key, slot) in [(Key::Q, 0usize), (Key::W, 1), (Key::E, 2)] {
                if !input.is_key_down(key) {
                    continue;
                }
                let Some(ability) = network
                    .client
                    .abilities()
                    .get(usize::from(hero.abilities[slot]))
                else {
                    continue;
                };
                let range = Fix::from_bits(ability.cast_range_bits).to_f32();
                let radius = Fix::from_bits(ability.radius_bits).to_f32();
                let ring = Vec4::new(0.35, 0.78, 1.0, 0.55);
                if range > 0.0 {
                    draw_world_circle(ui, camera, center, range, width, height, ring);
                }
                if radius > 0.0 {
                    draw_world_circle(
                        ui,
                        camera,
                        center,
                        radius,
                        width,
                        height,
                        Vec4::new(1.0, 0.78, 0.30, 0.50),
                    );
                }
            }
        }
    }
    let snapshot = network.client.snapshot();
    let own = network
        .own_net_id
        .and_then(|id| snapshot.and_then(|state| state.get(id)))
        .or_else(|| {
            snapshot.and_then(|state| {
                state.entities.iter().find(|entity| {
                    entity.hero_slot == network.own_slot
                        && entity.team == network.client.team().index() as u8
                })
            })
        });
    let hero = network
        .client
        .heroes()
        .get(usize::from(network.client.hero_pick()))
        .copied()
        .unwrap_or_default();
    let hero_name = {
        let name = hero.name_str();
        if name.is_empty() { "UNKNOWN" } else { name }
    };

    // Match timer and team score.
    let tick = snapshot.map_or(0, |state| state.tick);
    let seconds = tick / 30;
    let timer = format!("{:02}:{:02}", seconds / 60, seconds % 60);
    let blue = snapshot.map_or(0, |state| state.blue_score);
    let red = snapshot.map_or(0, |state| state.red_score);
    let timer_width = 360.0;
    ui.panel(
        (width_f - timer_width) * 0.5,
        14.0,
        timer_width,
        72.0,
        bg,
        border,
    );
    ui.text_shadowed(
        (width_f - UiDrawList::text_width(&timer, 3.5)) * 0.5,
        20.0,
        3.5,
        text,
        &timer,
    );
    ui.text(
        (width_f - timer_width) * 0.5 + 18.0,
        52.0,
        2.0,
        Vec4::new(0.4, 0.65, 1.0, 1.0),
        &format!("BLUE {blue}"),
    );
    ui.text(
        width_f.midpoint(timer_width) - 18.0 - UiDrawList::text_width(&format!("RED {red}"), 2.0),
        52.0,
        2.0,
        Vec4::new(1.0, 0.42, 0.32, 1.0),
        &format!("RED {red}"),
    );

    // Connection/status block.
    ui.panel(18.0, 18.0, 250.0, 82.0, bg, border);
    ui.text(
        30.0,
        28.0,
        2.5,
        text,
        if network.spectator {
            "MOBA SPECTATOR"
        } else {
            "MOBA ONLINE"
        },
    );
    let stats = network.client.stats();
    let ping = stats.rtt_ms.unwrap_or(0);
    ui.text(
        30.0,
        54.0,
        2.0,
        muted,
        &format!(
            "SLOT {}   PING {} MS",
            if network.spectator {
                String::from("--")
            } else {
                network.own_slot.to_string()
            },
            ping
        ),
    );
    ui.text(
        30.0,
        74.0,
        2.0,
        muted,
        &format!("TICK {}   {} SNAPSHOTS", tick, stats.snapshots),
    );

    if !network.spectator {
        ui.text(30.0, 108.0, 1.8, muted, "R READY   F5 SURRENDER");
    }
    // Controls.
    let controls = if network.spectator {
        "SPECTATOR VIEW   TAB SCOREBOARD"
    } else {
        "RMB MOVE   LMB ATTACK   Q/W/E CAST"
    };
    ui.text(
        width_f - 24.0 - UiDrawList::text_width(controls, 2.0),
        24.0,
        2.0,
        muted,
        controls,
    );

    let progress = own.map_or(moba_net::protocol::NetProgress::default(), |entity| {
        entity.progress
    });
    if !network.spectator {
        // Hero panel.
        let panel_w = 452.0;
        let panel_h = 196.0;
        let x = 24.0;
        let y = height_f - panel_h - 24.0;
        ui.panel(x, y, panel_w, panel_h, bg, border);
        let level = progress.level.max(1);
        let xp_needed = if level >= LEVEL_CAP {
            0
        } else {
            xp_to_next(level)
        };
        ui.text_shadowed(x + 14.0, y + 12.0, 3.0, text, hero_name);
        ui.text(
            x + 14.0 + UiDrawList::text_width(hero_name, 3.0) + 14.0,
            y + 16.0,
            2.0,
            Vec4::new(0.95, 0.78, 0.3, 1.0),
            &format!("LV {level}"),
        );
        ui.text(
            x + 250.0,
            y + 16.0,
            2.0,
            muted,
            &format!(
                "TEAM {}   {}G",
                if network.client.team() == Team::Blue {
                    "BLUE"
                } else {
                    "RED"
                },
                progress.gold
            ),
        );

        let (hp, max_hp, mp, max_mp) = own.map_or((0, 1, 0, 1), |entity| {
            (
                i32::from(entity.health),
                i32::from(entity.max_health).max(1),
                i32::from(entity.mana),
                i32::from(entity.max_mana).max(1),
            )
        });
        ui.text(x + 14.0, y + 42.0, 2.0, muted, &format!("HP {hp}/{max_hp}"));
        ui.bar(
            x + 14.0,
            y + 58.0,
            panel_w - 28.0,
            14.0,
            hp as f32 / max_hp as f32,
            health,
            track,
            border,
        );
        ui.text(x + 14.0, y + 78.0, 2.0, muted, &format!("MP {mp}/{max_mp}"));
        ui.bar(
            x + 14.0,
            y + 94.0,
            panel_w - 28.0,
            10.0,
            mp as f32 / max_mp as f32,
            mana,
            track,
            border,
        );
        let xp_fraction = if xp_needed == 0 {
            1.0
        } else {
            f32::from(progress.xp) / f32::from(u16::try_from(xp_needed).unwrap_or(u16::MAX))
        };
        let xp_label = if xp_needed == 0 {
            String::from("XP MAX")
        } else {
            format!("XP {}/{}", progress.xp, xp_needed)
        };
        ui.text(x + 14.0, y + 112.0, 1.8, muted, &xp_label);
        ui.bar(
            x + 14.0,
            y + 128.0,
            panel_w - 28.0,
            8.0,
            xp_fraction,
            Vec4::new(0.85, 0.68, 0.25, 1.0),
            track,
            border,
        );

        let cooldowns = own.map_or([0; 3], |entity| entity.cooldowns);
        for slot in 0..3 {
            let box_size = 38.0;
            let bx = x + 14.0 + slot as f32 * 48.0;
            let by = y + 146.0;
            let key = ["Q", "W", "E"][slot];
            ui.rect(bx, by, box_size, box_size, track);
            ui.outline(bx, by, box_size, box_size, 1.0, border);
            ui.text(bx + 8.0, by + 8.0, 2.5, text, key);
            let remaining = cooldowns[slot];
            let max_cooldown = network
                .client
                .abilities()
                .get(usize::from(hero.abilities[slot]))
                .map_or(1, |ability| {
                    u16::try_from(ability.cooldown_ticks).unwrap_or(u16::MAX)
                })
                .max(1);
            if remaining > 0 {
                let fraction = f32::from(remaining) / f32::from(max_cooldown);
                ui.rect(
                    bx,
                    by,
                    box_size,
                    box_size * fraction.clamp(0.0, 1.0),
                    Vec4::new(0.0, 0.0, 0.0, 0.62),
                );
                let seconds = f32::from(remaining) / 30.0;
                ui.text(bx + 4.0, by + 24.0, 1.8, text, &format!("{seconds:.1}"));
            }
        }
        // Inventory.
        ui.text(x + 172.0, y + 148.0, 1.8, muted, "ITEMS");
        for slot in 0..6 {
            let bx = x + 172.0 + slot as f32 * 43.0;
            let by = y + 164.0;
            ui.rect(bx, by, 34.0, 26.0, track);
            ui.outline(bx, by, 34.0, 26.0, 1.0, border);
            let item = progress.items[slot];
            if item != EMPTY_ITEM {
                if let Some(definition) = network.client.items().get(usize::from(item)) {
                    let name = definition.name_str();
                    let short = name.get(..5).unwrap_or(name);
                    ui.text(bx + 2.0, by + 8.0, 1.4, text, short);
                }
            }
        }
    }
    draw_minimap(ui, network, width, height);
    if scoreboard {
        let panel_width = 620.0;
        let panel_height = 330.0;
        let sx = (width_f - panel_width) * 0.5;
        let sy = (height_f - panel_height) * 0.5;
        ui.panel(
            sx,
            sy,
            panel_width,
            panel_height,
            Vec4::new(0.01, 0.02, 0.035, 0.9),
            border,
        );
        ui.text_shadowed(sx + 24.0, sy + 18.0, 3.5, text, "SCOREBOARD");
        ui.text(
            sx + panel_width - 200.0,
            sy + 26.0,
            2.0,
            muted,
            "TAB TO CLOSE",
        );
        let mut rows = [(None, 0u8, 0u8, 0u16, 0u16, 0u8); 10];
        let mut count = 0usize;
        if let Some(state) = snapshot {
            for entity in state
                .entities
                .iter()
                .filter(|entity| entity.hero_slot != 0xFF)
            {
                if count >= rows.len() {
                    break;
                }
                rows[count] = (
                    Some(entity.team),
                    entity.hero_slot,
                    entity.hero_pick,
                    entity.health,
                    entity.max_health,
                    entity.progress.level,
                );
                count += 1;
            }
        }
        rows[..count].sort_unstable_by_key(|(team, slot, _, _, _, _)| (*team, *slot));
        for (index, (team, slot, pick, hp, max_hp, level)) in rows[..count].iter().enumerate() {
            let y = sy + 64.0 + index as f32 * 24.0;
            let color = if *team == Some(0) {
                Vec4::new(0.45, 0.7, 1.0, 1.0)
            } else {
                Vec4::new(1.0, 0.5, 0.4, 1.0)
            };
            let name = network
                .client
                .heroes()
                .get(usize::from(*pick))
                .map_or("UNKNOWN", |hero| {
                    let name = hero.name_str();
                    if name.is_empty() { "UNKNOWN" } else { name }
                });
            ui.text(
                sx + 32.0,
                y,
                2.0,
                color,
                &format!("SLOT {slot}  {name}  LV {level}"),
            );
            ui.text(sx + 400.0, y, 2.0, muted, &format!("{hp}/{max_hp} HP"));
        }
        ui.text(
            sx + 32.0,
            sy + panel_height - 34.0,
            2.0,
            muted,
            "ENEMY ROWS APPEAR ONLY WHILE VISIBLE",
        );
    }
    if shop && !network.spectator {
        let shop_w = 700.0;
        let shop_h = 390.0;
        let sx = (width_f - shop_w) * 0.5;
        let sy = ((height_f - shop_h) * 0.5 - 80.0).max(40.0);
        ui.panel(
            sx,
            sy,
            shop_w,
            shop_h,
            Vec4::new(0.01, 0.02, 0.035, 0.94),
            border,
        );
        ui.text_shadowed(sx + 24.0, sy + 18.0, 3.5, text, "SHOP");
        ui.text(
            sx + shop_w - 180.0,
            sy + 26.0,
            2.0,
            Vec4::new(0.95, 0.78, 0.3, 1.0),
            &format!("{} GOLD", progress.gold),
        );
        let page = network.shop_page;
        for (index, item) in network
            .client
            .items()
            .iter()
            .skip(page * 8)
            .take(8)
            .enumerate()
        {
            let row_y = sy + 62.0 + index as f32 * 36.0;
            let affordable = u32::from(progress.gold) >= item.cost;
            let name_color = if affordable {
                text
            } else {
                Vec4::new(0.45, 0.5, 0.58, 1.0)
            };
            ui.text(
                sx + 24.0,
                row_y,
                2.0,
                muted,
                &format!("[{}]", page * 8 + index + 1),
            );
            ui.text(sx + 70.0, row_y, 2.0, name_color, item.name_str());
            ui.text(
                sx + 300.0,
                row_y,
                2.0,
                if affordable {
                    Vec4::new(0.95, 0.78, 0.3, 1.0)
                } else {
                    Vec4::new(0.55, 0.4, 0.3, 1.0)
                },
                &format!("{}G", item.cost),
            );
            ui.text(sx + 390.0, row_y, 1.7, muted, &item_summary(item));
        }
        ui.text(
            sx + 24.0,
            sy + shop_h - 30.0,
            1.8,
            muted,
            &format!(
                "B CLOSE   NUMBER BUY   ARROWS PAGE {}/{}   PURCHASES REQUIRE OWN NEXUS",
                page + 1,
                network.client.items().len().div_ceil(8).max(1)
            ),
        );
    }
    if matches!(network.phase, MatchPhase::Lobby | MatchPhase::Countdown) {
        let seconds = u32::from(network.countdown).div_ceil(30);
        let (label, footer) = if network.spectator {
            (
                format!("SPECTATOR — MATCH STARTS IN {seconds}"),
                String::from("ALL VISIBLE   WASD PAN   ARROWS ROTATE   UP/DOWN ZOOM"),
            )
        } else if network.phase == MatchPhase::Lobby {
            if network.ready {
                (
                    String::from("READY — WAITING FOR PLAYERS"),
                    format!(
                        "{}/{} PLAYERS READY",
                        network.ready_count, network.player_count
                    ),
                )
            } else {
                (
                    String::from("PRESS R TO READY"),
                    format!(
                        "{}/{} PLAYERS READY",
                        network.ready_count, network.player_count
                    ),
                )
            }
        } else {
            (
                format!("MATCH STARTS IN {seconds}"),
                String::from("BOTS FILL EMPTY SLOTS"),
            )
        };
        let panel_w = 560.0;
        let panel_h = 110.0;
        ui.panel(
            (width_f - panel_w) * 0.5,
            height_f * 0.35,
            panel_w,
            panel_h,
            bg,
            border,
        );
        ui.text_shadowed(
            (width_f - UiDrawList::text_width(&label, 4.0)) * 0.5,
            height_f * 0.35 + 26.0,
            4.0,
            text,
            &label,
        );
        ui.text(
            (width_f - UiDrawList::text_width(&footer, 2.0)) * 0.5,
            height_f * 0.35 + 72.0,
            2.0,
            muted,
            &footer,
        );
    }
    if let Some(outcome) = snapshot
        .map(|state| state.outcome)
        .filter(|outcome| *outcome != 0)
    {
        let winner = if outcome == 1 { Team::Blue } else { Team::Red };
        let victory = winner == network.client.team();
        let label = if victory { "VICTORY" } else { "DEFEAT" };
        let color = if victory {
            Vec4::new(0.35, 1.0, 0.5, 1.0)
        } else {
            Vec4::new(1.0, 0.35, 0.28, 1.0)
        };
        ui.rect(0.0, 0.0, width_f, height_f, Vec4::new(0.0, 0.0, 0.0, 0.35));
        ui.panel(
            (width_f - 460.0) * 0.5,
            height_f * 0.36,
            460.0,
            130.0,
            bg,
            border,
        );
        ui.text_shadowed(
            (width_f - UiDrawList::text_width(label, 7.0)) * 0.5,
            height_f * 0.36 + 24.0,
            7.0,
            color,
            label,
        );
        let seconds = u32::from(network.countdown).div_ceil(30);
        let footer = if network.phase == MatchPhase::Results {
            format!("NEXT MATCH IN {seconds}   /   ESC TO EXIT")
        } else {
            String::from("PRESS ESC TO EXIT")
        };
        ui.text(
            (width_f - UiDrawList::text_width(&footer, 2.0)) * 0.5,
            height_f * 0.36 + 92.0,
            2.0,
            muted,
            &footer,
        );
    }
    if own.is_some_and(|entity| entity.is_dead()) {
        ui.text_shadowed(
            (width_f - UiDrawList::text_width("RESPAWNING", 5.0)) * 0.5,
            height_f * 0.42,
            5.0,
            Vec4::new(1.0, 0.35, 0.25, 1.0),
            "RESPAWNING",
        );
    }
}

pub(crate) fn item_summary(item: &moba_net::protocol::WireItem) -> String {
    let mut parts = Vec::new();
    if item.health != 0 {
        parts.push(format!("+{} HP", item.health));
    }
    if item.mana != 0 {
        parts.push(format!("+{} MP", item.mana));
    }
    if item.attack_damage != 0 {
        parts.push(format!("+{} DMG", item.attack_damage));
    }
    if item.armor != 0 {
        parts.push(format!("+{} ARMOR", item.armor));
    }
    if item.magic_resist != 0 {
        parts.push(format!("+{} MR", item.magic_resist));
    }
    let move_speed = Fix::from_bits(item.move_speed_bits);
    if move_speed != Fix::ZERO {
        parts.push(format!("+{} MS", move_speed.to_f32()));
    }
    parts.join("  ")
}

pub(crate) fn build_local_hud(
    ui: &mut UiDrawList,
    simulation: &Simulation,
    width: u32,
    height: u32,
    frame_ms: f32,
    fps: f32,
) {
    ui.clear();
    draw_perf(ui, width as f32, frame_ms, fps);
    if width == 0 || height == 0 {
        return;
    }
    let bg = Vec4::new(0.015, 0.025, 0.045, 0.82);
    let border = Vec4::new(0.22, 0.42, 0.58, 0.9);
    let text = Vec4::new(0.88, 0.94, 1.0, 1.0);
    let muted = Vec4::new(0.55, 0.68, 0.78, 1.0);
    ui.panel(18.0, 18.0, 330.0, 112.0, bg, border);
    ui.text_shadowed(32.0, 30.0, 3.0, text, "LOCAL BOT PREVIEW");
    ui.text(
        32.0,
        64.0,
        2.0,
        muted,
        &format!(
            "TICK {}   WAVE {}   UNITS {}",
            simulation.current_tick().get(),
            simulation.wave(),
            simulation.unit_count()
        ),
    );
    ui.text(32.0, 88.0, 2.0, muted, "SET MOBA_SERVER TO PLAY ONLINE");
    ui.text(
        width as f32 - 24.0 - UiDrawList::text_width("OFFLINE", 2.0),
        height as f32 - 36.0,
        2.0,
        muted,
        "OFFLINE",
    );
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
