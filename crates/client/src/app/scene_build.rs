//! Scene/unit instance construction for the network client.

use std::time::Instant;

use moba_core::FixVec2;
use moba_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use moba_game::Team;
use moba_net::protocol::{
    STATUS_BURN, STATUS_HASTE, STATUS_ROOT, STATUS_SHIELD, STATUS_SILENCE, STATUS_SLOW,
};
use moba_render::{Camera, UnitInstance};
use moba_sim::UnitKind;

use crate::app::NetworkView;
use crate::hud::{HudUnit, map_center, rendered_entity_position};

pub(crate) fn map_theme(theme: u8) -> ([f32; 4], Vec4) {
    match theme {
        1 => ([0.20, 0.20, 0.19, 1.0], Vec4::new(0.34, 0.33, 0.30, 1.0)),
        2 => ([0.48, 0.56, 0.68, 1.0], Vec4::new(0.62, 0.68, 0.76, 1.0)),
        3 => ([0.34, 0.25, 0.14, 1.0], Vec4::new(0.48, 0.36, 0.22, 1.0)),
        4 => ([0.15, 0.08, 0.26, 1.0], Vec4::new(0.32, 0.24, 0.44, 1.0)),
        _ => ([0.10, 0.16, 0.11, 1.0], Vec4::new(0.24, 0.21, 0.16, 1.0)),
    }
}

pub(crate) fn rebuild_network_units(network: &NetworkView, units: &mut Vec<UnitInstance>) {
    units.clear();
    let Some(latest) = network.interpolator.latest() else {
        return;
    };
    let wire_map = network.client.map();
    let center = map_center(network);
    let (_, wall_color) = map_theme(wire_map.theme);
    for wall in wire_map.walls.iter().take(usize::from(wire_map.wall_count)) {
        let a = Vec2::new(f32::from(wall.a[0]), f32::from(wall.a[1]));
        let b = Vec2::new(f32::from(wall.b[0]), f32::from(wall.b[1]));
        let direction = b - a;
        let length = direction.length().max(0.5);
        let angle = direction.y.atan2(direction.x);
        let mid = (a + b) * 0.5;
        let transform = Mat4::from_scale_rotation_translation(
            Vec3::new(length, 4.0, 2.5),
            Quat::from_rotation_y(-angle),
            Vec3::new(mid.x - center, 2.0, mid.y - center),
        );
        units.push(UnitInstance::new(transform, wall_color));
    }
    let render_tick = network.interpolator.render_tick(Instant::now());
    for entity in &latest.entities {
        if entity.is_dead() {
            continue;
        }
        let Some(kind) = kind_from_u8(entity.kind) else {
            continue;
        };
        let Some(team) = team_from_u8(entity.team) else {
            continue;
        };
        let position = rendered_entity_position(network, HudUnit::new(entity), render_tick);
        let fraction = if entity.max_health == 0 {
            1.0
        } else {
            f32::from(entity.health) / f32::from(entity.max_health)
        };
        if kind == UnitKind::Hero {
            let previous = network
                .interpolator
                .position(entity.net_id, render_tick - 2.0)
                .unwrap_or(position);
            let dx = position.x.to_f32() - previous.x.to_f32();
            let dy = position.y.to_f32() - previous.y.to_f32();
            let speed_sq = dx * dx + dy * dy;
            let (facing, gait) = if speed_sq > 0.0001 {
                (dy.atan2(dx), 1.0)
            } else {
                (0.0, 0.0)
            };
            push_hero_rig(
                units,
                position,
                team,
                fraction,
                center,
                render_tick / 30.0,
                f32::from(entity.net_id),
                facing,
                gait,
            );
            continue;
        }
        push_unit(
            units,
            position,
            kind,
            team,
            fraction,
            entity.status_bits,
            center,
        );
    }
    for projectile in &network.projectiles {
        let color = if projectile.team == 0 {
            Vec4::new(0.35, 0.72, 1.0, 1.0)
        } else {
            Vec4::new(1.0, 0.45, 0.3, 1.0)
        };
        units.push(UnitInstance::at(
            Vec3::new(
                projectile.position.x.to_f32() - center,
                1.2,
                projectile.position.y.to_f32() - center,
            ),
            1.2 + projectile.radius * 0.2 + projectile.arc_height(),
            color,
        ));
    }
}

#[allow(clippy::too_many_arguments)] // Rig pose inputs are explicit.
pub(crate) fn push_hero_rig(
    units: &mut Vec<UnitInstance>,
    position: FixVec2,
    team: Team,
    health_fraction: f32,
    center: f32,
    time: f32,
    phase_offset: f32,
    facing: f32,
    gait: f32,
) {
    let color = match team {
        Team::Blue => Vec4::new(0.28, 0.55, 1.0, 1.0),
        Team::Red => Vec4::new(1.0, 0.34, 0.28, 1.0),
    };
    let brightness = 0.55 + 0.45 * health_fraction.clamp(0.0, 1.0);
    let color = Vec4::new(
        color.x * brightness,
        color.y * brightness,
        color.z * brightness,
        1.0,
    );
    let base = Vec3::new(
        position.x.to_f32() - center,
        0.0,
        position.y.to_f32() - center,
    );
    let gait = gait.clamp(0.0, 1.0);
    let phase = (time * 6.0 + phase_offset).sin() * gait;
    let swing = (time * 9.0 + phase_offset).sin() * 0.35 * gait;
    let facing_rotation = Quat::from_rotation_y(facing);
    let part = |offset: Vec3, scale: Vec3, rotation: f32| {
        Mat4::from_scale_rotation_translation(
            scale,
            facing_rotation * Quat::from_rotation_z(rotation),
            base + facing_rotation * offset,
        )
    };
    // Torso, head, swinging arms and counter-swinging legs form a readable low-poly rig.
    units.push(UnitInstance::new(
        part(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.9, 1.4, 0.6), 0.0),
        color,
    ));
    units.push(UnitInstance::new(
        part(Vec3::new(0.0, 3.15, 0.0), Vec3::splat(0.7), 0.0),
        color,
    ));
    units.push(UnitInstance::new(
        part(
            Vec3::new(-0.72, 2.0 + phase * 0.08, 0.0),
            Vec3::new(0.3, 1.2, 0.3),
            swing,
        ),
        color,
    ));
    units.push(UnitInstance::new(
        part(
            Vec3::new(0.72, 2.0 - phase * 0.08, 0.0),
            Vec3::new(0.3, 1.2, 0.3),
            -swing,
        ),
        color,
    ));
    units.push(UnitInstance::new(
        part(
            Vec3::new(-0.28, 0.65, 0.0),
            Vec3::new(0.32, 1.3, 0.34),
            -swing,
        ),
        color,
    ));
    units.push(UnitInstance::new(
        part(
            Vec3::new(0.28, 0.65, 0.0),
            Vec3::new(0.32, 1.3, 0.34),
            swing,
        ),
        color,
    ));
}

pub(crate) fn push_unit(
    units: &mut Vec<UnitInstance>,
    position: FixVec2,
    kind: UnitKind,
    team: Team,
    health_fraction: f32,
    status: u16,
    center: f32,
) {
    let team_color = match team {
        Team::Blue => Vec4::new(0.20, 0.46, 1.0, 1.0),
        Team::Red => Vec4::new(1.0, 0.24, 0.18, 1.0),
    };
    let (height, mut color) = match kind {
        UnitKind::Hero => (4.6, team_color),
        UnitKind::MeleeMinion => (
            2.2,
            Vec4::new(
                team_color.x * 0.72 + 0.03,
                team_color.y * 0.72 + 0.03,
                team_color.z * 0.72 + 0.03,
                1.0,
            ),
        ),
        UnitKind::RangedMinion => (
            1.8,
            Vec4::new(
                team_color.x * 0.62 + 0.05,
                team_color.y * 0.62 + 0.05,
                team_color.z * 0.62 + 0.05,
                1.0,
            ),
        ),
        UnitKind::Tower => (
            9.0,
            Vec4::new(
                team_color.x * 0.60 + 0.12,
                team_color.y * 0.60 + 0.12,
                team_color.z * 0.60 + 0.12,
                1.0,
            ),
        ),
        UnitKind::Nexus => (15.0, Vec4::new(1.0, 0.76, 0.22, 1.0)),
    };
    let brightness = 0.55 + 0.45 * health_fraction.clamp(0.0, 1.0);
    color.x *= brightness;
    color.y *= brightness;
    color.z *= brightness;
    if status & STATUS_SHIELD != 0 {
        color = color * 0.65 + Vec4::new(0.15, 0.65, 0.95, 1.0) * 0.35;
    }
    if status & STATUS_SLOW != 0 {
        color = color * 0.75 + Vec4::new(0.35, 0.2, 0.75, 1.0) * 0.25;
    }
    if status & STATUS_ROOT != 0 {
        color = color * 0.7 + Vec4::new(0.9, 0.75, 0.18, 1.0) * 0.3;
    }
    if status & STATUS_SILENCE != 0 {
        color = color * 0.7 + Vec4::new(0.55, 0.55, 0.6, 1.0) * 0.3;
    }
    if status & STATUS_BURN != 0 {
        color = color * 0.7 + Vec4::new(1.0, 0.42, 0.1, 1.0) * 0.3;
    }
    if status & STATUS_HASTE != 0 {
        color = color * 0.75 + Vec4::new(0.25, 0.95, 0.4, 1.0) * 0.25;
    }
    let world_position = Vec3::new(
        position.x.to_f32() - center,
        0.0,
        position.y.to_f32() - center,
    );
    units.push(UnitInstance::at(world_position, height, color));
}

pub(crate) fn kind_from_u8(value: u8) -> Option<UnitKind> {
    Some(match value {
        0 => UnitKind::Hero,
        1 => UnitKind::MeleeMinion,
        2 => UnitKind::RangedMinion,
        3 => UnitKind::Tower,
        4 => UnitKind::Nexus,
        _ => return None,
    })
}

pub(crate) fn team_from_u8(value: u8) -> Option<Team> {
    Some(match value {
        0 => Team::Blue,
        1 => Team::Red,
        _ => return None,
    })
}

pub(crate) fn network_camera(network: &NetworkView, aspect: f32, elapsed: f32) -> Camera {
    if network.spectator {
        let idle_yaw = network.spectator_yaw + (elapsed * 0.002).sin() * 0.02;
        return Camera::orbit(
            network.spectator_pan,
            network.spectator_zoom,
            idle_yaw,
            0.92,
            aspect,
        );
    }
    if std::env::var("MOBA_CAMERA").is_ok_and(|value| value == "overview") {
        return Camera::orbit(Vec3::ZERO, 240.0, -0.7, 0.92, aspect);
    }
    let predicted = network.prediction.position();
    let world_center = map_center(network);
    let center = Vec3::new(
        predicted.x.to_f32() - world_center,
        0.0,
        predicted.y.to_f32() - world_center,
    );
    Camera::orbit(center, 46.0, -0.7, 0.95, aspect)
}
