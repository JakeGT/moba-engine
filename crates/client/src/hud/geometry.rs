//! HUD geometry.

use super::{Camera, FixVec2, NetworkView, Vec2, Vec3, Vec4, dequantize_position};

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
