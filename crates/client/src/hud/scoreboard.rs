//! Tab scoreboard overlay.

use crate::hud::theme;
use moba_core::math::Vec4;
use moba_net::protocol::SnapshotState;

use crate::app::NetworkView;
use crate::hud::{HitList, HudAction, register};
use crate::hud_fmt;

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_scoreboard(
    ui: &mut crate::hud::UiDrawList,
    network: &NetworkView,
    snapshot: Option<&SnapshotState>,
    width_f: f32,
    height_f: f32,
    text: Vec4,
    muted: Vec4,
    border: Vec4,
    hits: &mut HitList,
) {
    let _ = width_f;
    let panel_width = 620.0;
    let panel_height = 330.0;
    let sx = (width_f - panel_width) * 0.5;
    let sy = (height_f - panel_height) * 0.5;
    ui.panel(
        sx,
        sy,
        panel_width,
        panel_height,
        theme::SCOREBOARD_BG,
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
            theme::SCOREBOARD_BLUE
        } else {
            theme::SCOREBOARD_RED
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
