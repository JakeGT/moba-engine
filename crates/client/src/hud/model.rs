//! HUD model.

use super::theme;
use super::{
    Camera, Fix, HitList, HudAction, HudString, InputState, Key, NetworkView, Simulation, Team,
    UiDrawList, Vec4, draw_health_bars, draw_minimap, draw_perf, draw_world_circle, register,
};
use crate::{hud_fmt, hud_static};
use moba_core::FixVec2;
use moba_net::protocol::MatchPhase;

/// Borrowed view of one network unit for HUD drawing.
#[derive(Copy, Clone)]
pub(crate) struct HudUnit<'a>(&'a moba_net::NetEntity);

impl<'a> HudUnit<'a> {
    pub(crate) fn new(entity: &'a moba_net::NetEntity) -> Self {
        Self(entity)
    }

    pub(crate) fn net_id(self) -> u16 {
        self.0.net_id
    }
    pub(crate) fn kind(self) -> u8 {
        self.0.kind
    }
    pub(crate) fn team(self) -> u8 {
        self.0.team
    }
    pub(crate) fn position(self) -> FixVec2 {
        self.0.position()
    }
    pub(crate) fn health(self) -> i32 {
        i32::from(self.0.health)
    }
    pub(crate) fn max_health(self) -> i32 {
        i32::from(self.0.max_health)
    }
    pub(crate) fn dead(self) -> bool {
        self.0.is_dead()
    }

    pub(crate) fn hero_pick(self) -> u8 {
        self.0.hero_pick
    }
    pub(crate) fn status_bits(self) -> u16 {
        self.0.status_bits
    }
}

/// Square world extent from the handshake map.
pub(crate) fn map_extent(network: &NetworkView) -> f32 {
    f32::from(network.client.map().extent)
}

/// World-to-render centre offset.
pub(crate) fn map_center(network: &NetworkView) -> f32 {
    map_extent(network) * 0.5
}

/// Iterates the current network view as HUD units.
pub(crate) fn hud_units(network: &NetworkView) -> impl Iterator<Item = HudUnit<'_>> {
    network
        .client
        .snapshot()
        .into_iter()
        .flat_map(|state| state.entities.iter().map(HudUnit))
}

/// Visits each lane's valid minimap points.
pub(crate) fn map_lanes(network: &NetworkView, mut visit: impl FnMut(&[[i16; 2]])) {
    let map = network.client.map();
    for lane in map.lanes.iter().take(usize::from(map.lane_count)) {
        let count = usize::from(lane.point_count).min(lane.points.len());
        visit(&lane.points[..count]);
    }
}

/// Visits each minimap wall endpoint pair.
pub(crate) fn map_walls(network: &NetworkView, mut visit: impl FnMut([i16; 2], [i16; 2])) {
    let map = network.client.map();
    for wall in map.walls.iter().take(usize::from(map.wall_count)) {
        visit(wall.a, wall.b);
    }
}

pub(crate) use moba_net::protocol::{
    STATUS_BURN, STATUS_HASTE, STATUS_ROOT, STATUS_SHIELD, STATUS_SILENCE, STATUS_SLOW,
};

/// Frame-level HUD data shared by the network and local-preview modes.
pub(crate) struct HudModel<'a> {
    source: HudSource<'a>,
}

enum HudSource<'a> {
    Network(&'a NetworkView),
    Local(&'a Simulation),
}

impl<'a> HudModel<'a> {
    pub(crate) fn network(network: &'a NetworkView) -> Self {
        Self {
            source: HudSource::Network(network),
        }
    }

    pub(crate) fn local(simulation: &'a Simulation) -> Self {
        Self {
            source: HudSource::Local(simulation),
        }
    }

    pub(crate) fn tick(&self) -> u32 {
        match self.source {
            HudSource::Network(network) => network.client.snapshot().map_or(0, |state| state.tick),
            HudSource::Local(simulation) => simulation.current_tick().get(),
        }
    }

    pub(crate) fn phase(&self) -> MatchPhase {
        match self.source {
            HudSource::Network(network) => network.phase,
            HudSource::Local(_) => MatchPhase::Playing,
        }
    }

    pub(crate) fn countdown(&self) -> u16 {
        match self.source {
            HudSource::Network(network) => network.countdown,
            HudSource::Local(_) => 0,
        }
    }

    pub(crate) fn score(&self) -> (u32, u32) {
        match self.source {
            HudSource::Network(network) => network
                .client
                .snapshot()
                .map_or((0, 0), |state| (state.blue_score, state.red_score)),
            HudSource::Local(_) => (0, 0),
        }
    }

    pub(crate) fn wave(&self) -> u32 {
        match self.source {
            HudSource::Network(network) => network
                .client
                .snapshot()
                .map_or(0, |state| u32::try_from(state.entities.len()).unwrap_or(0)),
            HudSource::Local(simulation) => simulation.wave(),
        }
    }

    pub(crate) fn unit_count(&self) -> u32 {
        match self.source {
            HudSource::Network(network) => network
                .client
                .snapshot()
                .map_or(0, |state| u32::try_from(state.entities.len()).unwrap_or(0)),
            HudSource::Local(simulation) => {
                u32::try_from(simulation.unit_count()).unwrap_or(u32::MAX)
            }
        }
    }
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
    hits: &mut HitList,
) {
    hits.clear();
    ui.clear();
    if width == 0 || height == 0 {
        return;
    }
    let bg = theme::PANEL_BG;
    let border = theme::PANEL_CHROME;
    let text = theme::TEXT;
    let muted = theme::MUTED;
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
    let model = HudModel::network(network);
    let tick = model.tick();
    let seconds = tick / 30;
    let timer = hud_fmt!("{:02}:{:02}", seconds / 60, seconds % 60);
    let (blue, red) = model.score();
    let timer_width = 360.0;
    ui.panel(
        (width_f - timer_width) * 0.5,
        14.0,
        timer_width,
        72.0,
        bg,
        border,
    );
    register(
        hits,
        HudAction::Block,
        (width_f - timer_width) * 0.5,
        14.0,
        timer_width,
        72.0,
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
        &hud_fmt!("BLUE {blue}"),
    );
    ui.text(
        width_f.midpoint(timer_width) - 18.0 - UiDrawList::text_width(&hud_fmt!("RED {red}"), 2.0),
        52.0,
        2.0,
        theme::RED_ACCENT,
        &hud_fmt!("RED {red}"),
    );

    // Connection/status block.
    ui.panel(18.0, 18.0, 250.0, 82.0, bg, border);
    register(hits, HudAction::Block, 18.0, 18.0, 250.0, 82.0);
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
        &hud_fmt!(
            "SLOT {}   PING {} MS",
            if network.spectator {
                hud_static!("--")
            } else {
                hud_fmt!("{}", network.own_slot)
            },
            ping
        ),
    );
    ui.text(
        30.0,
        74.0,
        2.0,
        muted,
        &hud_fmt!("TICK {}   {} SNAPSHOTS", tick, stats.snapshots),
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
        register(hits, HudAction::Block, x, y, panel_w, panel_h);
        let level = progress.level.max(1);
        let rules = network.client.rules();
        let xp_needed = if level >= rules.level_cap {
            0
        } else {
            rules
                .xp_base
                .saturating_add(rules.xp_per_level.saturating_mul(u32::from(level)))
        };
        ui.text_shadowed(x + 14.0, y + 12.0, 3.0, text, hero_name);
        ui.text(
            x + 14.0 + UiDrawList::text_width(hero_name, 3.0) + 14.0,
            y + 16.0,
            2.0,
            theme::GOLD,
            &hud_fmt!("LV {level}"),
        );
        ui.text(
            x + 250.0,
            y + 16.0,
            2.0,
            muted,
            &hud_fmt!(
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
        ui.text(
            x + 14.0,
            y + 42.0,
            2.0,
            muted,
            &hud_fmt!("HP {hp}/{max_hp}"),
        );
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
        ui.text(
            x + 14.0,
            y + 78.0,
            2.0,
            muted,
            &hud_fmt!("MP {mp}/{max_mp}"),
        );
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
            hud_static!("XP MAX")
        } else {
            hud_fmt!("XP {}/{}", progress.xp, xp_needed)
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
        let mut hovered_ability = None;
        for slot in 0..3 {
            let box_size = 38.0;
            let bx = x + 14.0 + slot as f32 * 48.0;
            let by = y + 146.0;
            let key = ["Q", "W", "E"][slot];
            ui.rect(bx, by, box_size, box_size, track);
            ui.outline(bx, by, box_size, box_size, 1.0, border);
            let icon = [0u32, 1, 6][slot];
            ui.icon(bx + 9.0, by + 5.0, 20.0, icon, text);
            ui.text(bx + 3.0, by + 27.0, 1.6, muted, key);
            register(
                hits,
                HudAction::Ability(u8::try_from(slot).unwrap_or(0)),
                bx,
                by,
                box_size,
                box_size,
            );
            if input.cursor().is_some_and(|cursor| {
                cursor.x >= bx
                    && cursor.x <= bx + box_size
                    && cursor.y >= by
                    && cursor.y <= by + box_size
            }) {
                hovered_ability = Some(slot);
            }
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
                ui.text(bx + 4.0, by + 24.0, 1.8, text, &hud_fmt!("{seconds:.1}"));
            }
        }
        if let Some(slot) = hovered_ability {
            if let Some(ability) = network
                .client
                .abilities()
                .get(usize::from(hero.abilities[slot]))
            {
                let name = ability.name_str();
                let kind = match ability.targeting_kind {
                    0 => "SELF",
                    1 => "POINT",
                    _ => "DIRECTION",
                };
                let range = Fix::from_bits(ability.cast_range_bits).to_f32();
                let tx = (x + 14.0 + slot as f32 * 48.0).min(width_f - 270.0);
                let ty = (y + 146.0 + 46.0).min(height_f - 86.0);
                ui.panel(
                    tx,
                    ty,
                    270.0,
                    72.0,
                    Vec4::new(0.01, 0.02, 0.035, 0.96),
                    theme::GOLD,
                );
                ui.text(tx + 10.0, ty + 8.0, 2.0, theme::GOLD, name);
                ui.text(
                    tx + 10.0,
                    ty + 28.0,
                    1.7,
                    text,
                    &hud_fmt!(
                        "{kind}  RANGE {range:.1}  CD {:.1}s  MANA {}",
                        f32::from(u16::try_from(ability.cooldown_ticks).unwrap_or(u16::MAX)) / 30.0,
                        ability.mana_cost
                    ),
                );
                let effect = ability
                    .effects
                    .get(usize::from(ability.effect_count.saturating_sub(1)))
                    .copied()
                    .unwrap_or_default();
                let kind = match effect.kind {
                    0 => "NO EFFECT",
                    1 => "DAMAGE",
                    2 => "HEAL",
                    3 => "MODIFIER",
                    4 => "DISPLACE",
                    5 => "PROJECTILE",
                    6 => "AREA",
                    _ => "ZONE",
                };
                ui.text(
                    tx + 10.0,
                    ty + 48.0,
                    1.7,
                    muted,
                    &hud_fmt!("{kind}  AMOUNT {}", effect.amount),
                );
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
            if item != 0xFF {
                if let Some(definition) = network.client.items().get(usize::from(item)) {
                    ui.icon(bx + 7.0, by + 3.0, 20.0, u32::from(item) % 16, text);
                    let name = definition.name_str();
                    let short = name.get(..4).unwrap_or(name);
                    ui.text(bx + 1.0, by + 19.0, 1.2, muted, short);
                }
            }
        }
    }
    draw_minimap(ui, network, width, height, hits);
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
        register(hits, HudAction::Block, sx, sy, panel_width, panel_height);
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
                &hud_fmt!("SLOT {slot}  {name}  LV {level}"),
            );
            ui.text(sx + 400.0, y, 2.0, muted, &hud_fmt!("{hp}/{max_hp} HP"));
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
        register(hits, HudAction::Block, sx, sy, shop_w, shop_h);
        ui.text_shadowed(sx + 24.0, sy + 18.0, 3.5, text, "SHOP");
        ui.text(
            sx + shop_w - 180.0,
            sy + 26.0,
            2.0,
            theme::GOLD,
            &hud_fmt!("{} GOLD", progress.gold),
        );
        let page = network.shop_page;
        let mut hovered_item = None;
        for (index, item) in network
            .client
            .items()
            .iter()
            .skip(page * 8)
            .take(8)
            .enumerate()
        {
            let row_y = sy + 62.0 + index as f32 * 36.0;
            let item_index = u8::try_from(page * 8 + index).unwrap_or(u8::MAX);
            let row_x = sx + 24.0;
            let row_w = shop_w - 48.0;
            register(
                hits,
                HudAction::ShopItem(item_index),
                row_x,
                row_y - 4.0,
                row_w,
                32.0,
            );
            if input.cursor().is_some_and(|cursor| {
                cursor.x >= row_x
                    && cursor.x <= row_x + row_w
                    && cursor.y >= row_y - 4.0
                    && cursor.y <= row_y + 28.0
            }) {
                hovered_item = input.cursor().map(|cursor| (cursor.x, cursor.y, item));
            }
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
                &hud_fmt!("[{}]", page * 8 + index + 1),
            );
            ui.icon(
                sx + 46.0,
                row_y - 2.0,
                20.0,
                u32::from(item_index) % 16,
                text,
            );
            ui.text(sx + 70.0, row_y, 2.0, name_color, item.name_str());
            ui.text(
                sx + 300.0,
                row_y,
                2.0,
                if affordable {
                    theme::GOLD
                } else {
                    Vec4::new(0.55, 0.4, 0.3, 1.0)
                },
                &hud_fmt!("{}G", item.cost),
            );
            ui.text(sx + 390.0, row_y, 1.7, muted, &item_summary(item));
        }
        if let Some((cursor_x, cursor_y, item)) = hovered_item {
            let name = item.name_str();
            let summary = item_summary(item);
            let tx = (cursor_x + 14.0).min(width_f - 270.0);
            let ty = (cursor_y + 14.0).min(height_f - 74.0);
            ui.panel(
                tx,
                ty,
                260.0,
                60.0,
                Vec4::new(0.01, 0.02, 0.035, 0.96),
                theme::GOLD,
            );
            ui.text(tx + 10.0, ty + 8.0, 2.0, theme::GOLD, name);
            ui.text(tx + 10.0, ty + 30.0, 1.8, text, &summary);
        }
        ui.text(
            sx + 24.0,
            sy + shop_h - 30.0,
            1.8,
            muted,
            &hud_fmt!(
                "B CLOSE   CLICK OR NUMBER BUY   ARROWS PAGE {}/{}   PURCHASES REQUIRE OWN NEXUS",
                page + 1,
                network.client.items().len().div_ceil(8).max(1)
            ),
        );
    }
    if matches!(model.phase(), MatchPhase::Lobby | MatchPhase::Countdown) {
        let seconds = u32::from(model.countdown()).div_ceil(30);
        let (label, footer) = if network.spectator {
            (
                hud_fmt!("SPECTATOR — MATCH STARTS IN {seconds}"),
                hud_static!("ALL VISIBLE   WASD PAN   ARROWS ROTATE   UP/DOWN ZOOM"),
            )
        } else if model.phase() == MatchPhase::Lobby {
            if network.ready {
                (
                    hud_static!("READY — WAITING FOR PLAYERS"),
                    hud_fmt!(
                        "{}/{} PLAYERS READY",
                        network.ready_count,
                        network.player_count
                    ),
                )
            } else {
                (
                    hud_static!("PRESS R TO READY"),
                    hud_fmt!(
                        "{}/{} PLAYERS READY",
                        network.ready_count,
                        network.player_count
                    ),
                )
            }
        } else {
            (
                hud_fmt!("MATCH STARTS IN {seconds}"),
                hud_static!("BOTS FILL EMPTY SLOTS"),
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
        let seconds = u32::from(model.countdown()).div_ceil(30);
        let footer = if model.phase() == MatchPhase::Results {
            hud_fmt!("NEXT MATCH IN {seconds}   /   ESC TO EXIT")
        } else {
            hud_static!("PRESS ESC TO EXIT")
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

pub(crate) fn item_summary(item: &moba_net::protocol::WireItem) -> HudString {
    use std::fmt::Write as _;

    let mut out = HudString::new();
    let mut first = true;
    let mut push = |label: &str, value: i32| {
        if value != 0 {
            if !first {
                out.push_str("  ");
            }
            let _ = write!(out, "+{value} {label}");
            first = false;
        }
    };
    push("HP", item.health);
    push("MP", item.mana);
    push("DMG", item.attack_damage);
    push("ARMOR", item.armor);
    push("MR", item.magic_resist);
    if item.move_speed_bits != 0 {
        if !first {
            out.push_str("  ");
        }
        let _ = write!(out, "+{} MS", Fix::from_bits(item.move_speed_bits).to_f32());
    }
    out
}

pub(crate) fn build_local_hud(
    ui: &mut UiDrawList,
    simulation: &Simulation,
    width: u32,
    height: u32,
    frame_ms: f32,
    fps: f32,
    hits: &mut HitList,
) {
    hits.clear();
    let model = HudModel::local(simulation);
    ui.clear();
    draw_perf(ui, width as f32, frame_ms, fps);
    if width == 0 || height == 0 {
        return;
    }
    let bg = theme::PANEL_BG;
    let border = theme::PANEL_CHROME;
    let text = theme::TEXT;
    let muted = theme::MUTED;
    ui.panel(18.0, 18.0, 330.0, 112.0, bg, border);
    register(hits, HudAction::Block, 18.0, 18.0, 330.0, 112.0);
    ui.text_shadowed(32.0, 30.0, 3.0, text, "LOCAL BOT PREVIEW");
    ui.text(
        32.0,
        64.0,
        2.0,
        muted,
        &hud_fmt!(
            "TICK {}   WAVE {}   UNITS {}",
            model.tick(),
            model.wave(),
            model.unit_count()
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

#[cfg(test)]
mod tests {
    use super::{HitList, HudModel, MatchPhase, Simulation, UiDrawList, build_local_hud};
    use crate::{hud_fmt, hud_static};
    use moba_sim::SimConfig;

    #[test]
    fn local_hud_builds_a_draw_list() {
        let simulation = Simulation::new(SimConfig::default());
        let mut ui = UiDrawList::new();
        let mut hits = HitList::new();
        build_local_hud(&mut ui, &simulation, 1280, 720, 16.0, 60.0, &mut hits);
        assert_ne!(ui.vertices().len(), 0);
        assert!(!hits.is_empty());
    }

    #[test]
    fn local_hud_has_zero_steady_state_allocations() {
        let simulation = Simulation::new(SimConfig::default());
        let mut ui = UiDrawList::new();
        let mut hits = HitList::new();
        // Warm retained draw-list capacity before measuring.
        build_local_hud(&mut ui, &simulation, 1280, 720, 16.0, 60.0, &mut hits);
        let ((), allocations) = crate::app::allocation_probe::probe(|| {
            build_local_hud(&mut ui, &simulation, 1280, 720, 16.0, 60.0, &mut hits);
        });
        assert_eq!(allocations, 0, "steady-state HUD allocation");
    }

    #[test]
    fn hud_model_reports_local_frame_state() {
        let simulation = Simulation::new(SimConfig::default());
        let model = HudModel::local(&simulation);
        assert_eq!(model.tick(), simulation.current_tick().get());
        assert_eq!(model.wave(), simulation.wave());
        assert_eq!(model.unit_count() as usize, simulation.unit_count());
        assert_eq!(model.score(), (0, 0));
        assert_eq!(model.countdown(), 0);
        assert_eq!(model.phase(), MatchPhase::Playing);
    }

    #[test]
    fn hud_text_is_stack_bounded() {
        let text = hud_fmt!("{}", "x".repeat(400));
        assert_eq!(text.capacity(), 256);
        assert!(text.len() <= 256);
        let long = hud_static!("a short fixed label");
        assert_eq!(long.capacity(), 256);
    }
}
