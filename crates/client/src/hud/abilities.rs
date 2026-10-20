//! Ability bar and tooltip.

use moba_core::Fix;
use moba_core::math::Vec4;
use moba_platform::InputState;

use crate::app::NetworkView;
use moba_net::protocol::NetEntity;

use crate::hud::theme;
use crate::hud_fmt;

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn draw_abilities(
    ui: &mut crate::hud::UiDrawList,
    network: &NetworkView,
    ability_ids: [u8; 3],
    own: Option<&NetEntity>,
    x: f32,
    y: f32,
    width_f: f32,
    height_f: f32,
    text: Vec4,
    muted: Vec4,
    border: Vec4,
    track: Vec4,
    input: &InputState,
) {
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
            .get(usize::from(ability_ids[slot]))
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
            .get(usize::from(ability_ids[slot]))
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
}
