//! Ability bar and tooltip.

use moba_core::Fix;
use moba_core::math::Vec4;
use moba_platform::InputState;

use crate::app::NetworkView;
use moba_net::protocol::NetEntity;

use crate::hud::{UiDrawList, theme};
use crate::hud_fmt;

/// How one ability slot is drawn.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SlotState {
    /// Remaining cooldown fraction for the sweep, 0..1 (0 = ready).
    pub(crate) cooldown: f32,
    /// The hero cannot pay the mana cost (tinted, independent of the cooldown).
    pub(crate) no_mana: bool,
}

impl SlotState {
    pub(crate) fn ready(self) -> bool {
        self.cooldown <= 0.0 && !self.no_mana
    }
}

/// Slot state from the replicated remaining cooldown and mana, and the catalogue cost and
/// cooldown. Without an own hero (spectating, before the first snapshot) nothing is tinted.
pub(crate) fn slot_state(
    remaining_ticks: u16,
    cooldown_ticks: u32,
    mana_cost: i32,
    mana: i32,
    has_hero: bool,
) -> SlotState {
    let cooldown = if remaining_ticks == 0 {
        0.0
    } else {
        // Clamp if the remaining time exceeds the catalogue cooldown.
        (f32::from(remaining_ticks) / cooldown_ticks.max(1) as f32).min(1.0)
    };
    SlotState {
        cooldown,
        no_mana: has_hero && mana < mana_cost,
    }
}

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
    let mana = own.map_or(0, |entity| i32::from(entity.mana));
    let mut hovered_ability = None;
    for slot in 0..3 {
        let box_size = 38.0;
        let bx = x + 14.0 + slot as f32 * 48.0;
        let by = y + 146.0;
        let key = ["Q", "W", "E"][slot];
        let ability = network
            .client
            .abilities()
            .get(usize::from(ability_ids[slot]));
        let state = slot_state(
            cooldowns[slot],
            ability.map_or(0, |ability| ability.cooldown_ticks),
            ability.map_or(0, |ability| ability.mana_cost),
            mana,
            own.is_some(),
        );
        ui.rect_gradient(
            bx,
            by,
            box_size,
            box_size,
            Vec4::new(track.x + 0.06, track.y + 0.08, track.z + 0.11, track.w),
            track,
        );
        let icon = [0u32, 1, 6][slot];
        let icon_color = if state.ready() { text } else { muted };
        ui.icon(bx + 9.0, by + 7.0, 20.0, icon, icon_color);
        if state.no_mana {
            ui.rect(bx, by, box_size, box_size, theme::NO_MANA);
        }
        if state.cooldown > 0.0 {
            ui.sweep(
                bx,
                by,
                box_size,
                state.cooldown,
                theme::COOLDOWN,
                theme::COOLDOWN_EDGE,
            );
            let label = super::model::cooldown_label(cooldowns[slot]);
            let scale = 2.4;
            ui.text_outlined(
                bx + (box_size - UiDrawList::text_width(&label, scale)) * 0.5,
                by + (box_size - 7.0 * scale) * 0.5,
                scale,
                text,
                &label,
            );
        }
        let frame = if state.ready() { theme::GOLD } else { border };
        ui.outline(bx, by, box_size, box_size, 1.0, frame);
        ui.text_outlined(bx + 3.0, by + 2.5, 1.3, muted, key);
        if let Some(ability) = ability.filter(|ability| ability.mana_cost > 0) {
            let cost = hud_fmt!("{}", ability.mana_cost);
            let color = if state.no_mana {
                theme::RED_ACCENT
            } else {
                theme::HEALTHBAR_MANA
            };
            ui.text_outlined(
                bx + box_size - 3.0 - UiDrawList::text_width(&cost, 1.2),
                by + box_size - 11.0,
                1.2,
                color,
                &cost,
            );
        }
        if input.cursor().is_some_and(|cursor| {
            cursor.x >= bx
                && cursor.x <= bx + box_size
                && cursor.y >= by
                && cursor.y <= by + box_size
        }) {
            hovered_ability = Some(slot);
        }
    }
    if let Some(slot) = hovered_ability
        && let Some(ability) = network
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
        ui.panel(tx, ty, 270.0, 72.0, theme::TOOLTIP_BG, theme::GOLD);
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

#[cfg(test)]
mod tests {
    use super::slot_state;

    #[test]
    fn slot_state_covers_cooldown_and_mana() {
        let ready = slot_state(0, 300, 40, 100, true);
        assert!(ready.ready());
        let cooling = slot_state(150, 300, 40, 100, true);
        assert!((cooling.cooldown - 0.5).abs() < 1e-6 && !cooling.no_mana);
        let broke = slot_state(0, 300, 40, 10, true);
        assert!(broke.no_mana && !broke.ready());
        assert_eq!(slot_state(500, 300, 0, 0, true).cooldown, 1.0);
        assert!(!slot_state(0, 300, 40, 0, false).no_mana);
        assert_eq!(crate::hud::model::cooldown_label(95).as_str(), "4");
        assert_eq!(crate::hud::model::cooldown_label(12).as_str(), "0.4");
        assert_eq!(crate::hud::model::cooldown_label(1).as_str(), "0.1");
        assert_eq!(crate::hud::model::cooldown_label(29).as_str(), "1.0");
        assert_eq!(crate::hud::model::cooldown_label(30).as_str(), "1");
    }
}
