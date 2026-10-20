//! Network/shop/spectator input routing.

use moba_core::math::{Vec2, Vec4};
use moba_core::{Fix, FixVec2};
use moba_net::protocol::{MatchPhase, WireCommand, dequantize_position, quantize_position};
use moba_platform::{InputState, Key, MouseButton};
use moba_render::Camera;

use crate::app::NetworkView;
use crate::hud::{HitList, HudAction, map_center, map_extent, minimap_world, rect_at};

fn handle_spectator_input(
    network: &mut NetworkView,
    input: &InputState,
    bindings: &crate::settings::Bindings,
    sensitivity: f32,
) {
    let pan_speed = if input.is_key_down(Key::Shift) {
        14.0
    } else {
        5.0
    };
    if input.is_key_down(bindings.move_up) {
        network.spectator_pan.z -= pan_speed;
    }
    if input.is_key_down(bindings.move_down) {
        network.spectator_pan.z += pan_speed;
    }
    if input.is_key_down(bindings.move_left) {
        network.spectator_pan.x -= pan_speed;
    }
    if input.is_key_down(bindings.move_right) {
        network.spectator_pan.x += pan_speed;
    }
    let pan_limit = map_center(network);
    network.spectator_pan.x = network.spectator_pan.x.clamp(-pan_limit, pan_limit);
    network.spectator_pan.z = network.spectator_pan.z.clamp(-pan_limit, pan_limit);
    if input.is_key_down(bindings.camera_left) {
        network.spectator_yaw -= 0.03 * sensitivity;
    }
    if input.is_key_down(bindings.camera_right) {
        network.spectator_yaw += 0.03 * sensitivity;
    }
    if input.was_key_pressed(bindings.camera_up) {
        network.spectator_zoom =
            (network.spectator_zoom - 10.0 * sensitivity).max(crate::settings::CAMERA_ZOOM_MIN);
    }
    if input.was_key_pressed(bindings.camera_down) {
        network.spectator_zoom =
            (network.spectator_zoom + 10.0 * sensitivity).min(crate::settings::CAMERA_ZOOM_MAX);
    }
}

/// Input focus for the network client.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum InputFocus {
    Gameplay,
    Shop,
}

impl InputFocus {
    pub(crate) const fn from_shop(shop: bool) -> Self {
        if shop { Self::Shop } else { Self::Gameplay }
    }
}

fn handle_shop_input(network: &mut NetworkView, input: &InputState, ui_action: Option<HudAction>) {
    let page_count = network.client.items().len().div_ceil(8).max(1);
    if input.was_key_pressed(Key::ArrowLeft) {
        network.shop_page = network.shop_page.saturating_sub(1);
    }
    if input.was_key_pressed(Key::ArrowRight) {
        network.shop_page = (network.shop_page + 1).min(page_count - 1);
    }
    network.shop_page = network.shop_page.min(page_count - 1);
    for (key, offset) in [
        (Key::Digit1, 0u8),
        (Key::Digit2, 1),
        (Key::Digit3, 2),
        (Key::Digit4, 3),
        (Key::Digit5, 4),
        (Key::Digit6, 5),
        (Key::Digit7, 6),
        (Key::Digit8, 7),
    ] {
        if input.was_key_pressed(key) {
            let item = network.shop_page * 8 + usize::from(offset);
            if item < network.client.items().len() {
                let _ = network
                    .client
                    .send_command(WireCommand::BuyItem { item: item as u8 });
            }
        }
    }
    if let Some(HudAction::ShopItem(item)) = ui_action
        && input.was_mouse_pressed(MouseButton::Left)
    {
        let _ = network.client.send_command(WireCommand::BuyItem { item });
    }
}

pub(crate) fn ground_orders_allowed(
    ui_action: Option<HudAction>,
    phase: MatchPhase,
    focus: InputFocus,
) -> bool {
    ui_action.is_none() && phase == MatchPhase::Playing && matches!(focus, InputFocus::Gameplay)
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One ordered focus/input sequence.
pub(crate) fn handle_network_input(
    network: &mut NetworkView,
    input: &InputState,
    ui_input: &InputState,
    camera: &Camera,
    width: u32,
    height: u32,
    shop: &mut bool,
    hits: &HitList,
    bindings: &crate::settings::Bindings,
    sensitivity: f32,
) {
    if network.spectator {
        handle_spectator_input(network, input, bindings, sensitivity);
        return;
    }
    let ui_rect = ui_input
        .cursor()
        .and_then(|cursor| rect_at(hits, cursor.x, cursor.y));
    let ui_action = ui_rect.map(|rect| rect.action);
    if let (Some(rect), Some(cursor)) = (ui_rect, ui_input.cursor())
        && rect.action == HudAction::Minimap
        && (input.was_mouse_pressed(MouseButton::Left)
            || input.was_mouse_pressed(MouseButton::Right))
    {
        let (x, y) = minimap_world(rect, cursor.x, cursor.y, map_extent(network));
        let target = FixVec2::new(Fix::from_f32(x), Fix::from_f32(y));
        let (x, y) = quantize_position(target);
        if network
            .client
            .send_command(WireCommand::MoveTo { x, y })
            .is_ok()
        {
            network.prediction.set_target(Some(target));
        }
        return;
    }
    if matches!(InputFocus::from_shop(*shop), InputFocus::Gameplay) {
        for (slot, key) in [
            Key::Digit1,
            Key::Digit2,
            Key::Digit3,
            Key::Digit4,
            Key::Digit5,
            Key::Digit6,
        ]
        .into_iter()
        .enumerate()
        {
            if input.was_key_pressed(key) {
                let _ = network
                    .client
                    .send_command(WireCommand::UseItem { slot: slot as u8 });
            }
        }
    }
    if input.was_key_pressed(bindings.ready) && network.phase == MatchPhase::Lobby {
        let ready = !network.ready;
        if network
            .client
            .send_command(WireCommand::Ready { ready })
            .is_ok()
        {
            network.ready = ready;
        }
    }
    if input.was_key_pressed(bindings.surrender) && network.phase == MatchPhase::Playing {
        let _ = network.client.send_command(WireCommand::Surrender);
    }
    if input.was_key_pressed(bindings.shop) {
        *shop = !*shop;
    }
    match InputFocus::from_shop(*shop) {
        InputFocus::Shop => {
            handle_shop_input(network, input, ui_action);
            return;
        }
        InputFocus::Gameplay => {}
    }
    if !ground_orders_allowed(ui_action, network.phase, InputFocus::from_shop(*shop)) {
        return;
    }
    let Some(cursor) = input.cursor() else {
        return;
    };
    let Some(ground) = cursor_ground(camera, cursor, width, height, map_center(network)) else {
        return;
    };

    if input.was_mouse_pressed(MouseButton::Right) {
        let (x, y) = quantize_position(ground);
        if network
            .client
            .send_command(WireCommand::MoveTo { x, y })
            .is_ok()
        {
            network.prediction.set_target(Some(ground));
        }
    }
    let attack_button = match bindings.attack_button {
        crate::settings::MouseBinding::Left => MouseButton::Left,
        crate::settings::MouseBinding::Right => MouseButton::Right,
    };
    if input.was_mouse_pressed(attack_button)
        && let Some(target) = nearest_enemy(network, ground)
    {
        let _ = network
            .client
            .send_command(WireCommand::Attack { net_id: target });
    }
    for (key, slot) in [
        (bindings.cast_q, 0u8),
        (bindings.cast_w, 1),
        (bindings.cast_e, 2),
    ] {
        if input.was_key_pressed(key) {
            let (x, y) = quantize_position(ground);
            let _ = network
                .client
                .send_command(WireCommand::CastAbility { slot, x, y });
        }
    }
    if input.was_key_pressed(bindings.stop) {
        let _ = network.client.send_command(WireCommand::Stop);
        network.prediction.set_target(None);
    }
}

fn nearest_enemy(network: &NetworkView, ground: FixVec2) -> Option<u16> {
    let own_team = network.client.team().index() as u8;
    let snapshot = network.client.snapshot()?;
    let mut best: Option<(Fix, u16)> = None;
    for entity in &snapshot.entities {
        if entity.team == own_team || entity.is_dead() {
            continue;
        }
        let position = dequantize_position(entity.x, entity.y);
        let distance = position.distance_sq(ground);
        if distance <= Fix::from_i32(4) * Fix::from_i32(4)
            && best.is_none_or(|(best_distance, _)| distance < best_distance)
        {
            best = Some((distance, entity.net_id));
        }
    }
    best.map(|(_, id)| id)
}

pub(crate) fn cursor_ground(
    camera: &Camera,
    cursor: Vec2,
    width: u32,
    height: u32,
    center: f32,
) -> Option<FixVec2> {
    if width == 0 || height == 0 {
        return None;
    }
    let ndc_x = 2.0 * cursor.x / width as f32 - 1.0;
    let ndc_y = 1.0 - 2.0 * cursor.y / height as f32;
    let inverse = camera.view_proj.inverse();
    let near_h = inverse * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
    let far_h = inverse * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
    if near_h.w.abs() < 1e-6 || far_h.w.abs() < 1e-6 {
        return None;
    }
    let near = near_h.truncate() / near_h.w;
    let far = far_h.truncate() / far_h.w;
    let direction = far - near;
    if direction.y.abs() < 1e-5 {
        return None;
    }
    let distance = -near.y / direction.y;
    if distance < 0.0 {
        return None;
    }
    let hit = near + direction * distance;
    Some(FixVec2::new(
        Fix::from_f32(hit.x + center),
        Fix::from_f32(hit.z + center),
    ))
}
