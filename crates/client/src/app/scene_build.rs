//! Scene/unit instance construction for the network client.

use moba_core::FixVec2;
use moba_core::math::{Mat4, Quat, Vec3, Vec4};
use moba_game::Team;
use moba_net::protocol::{
    STATUS_BURN, STATUS_HASTE, STATUS_ROOT, STATUS_SHIELD, STATUS_SILENCE, STATUS_SLOW,
};
use moba_render::{Camera, JointMatrix, UnitInstance};
use moba_sim::UnitKind;

use crate::app::NetworkView;
use crate::app::animation::{AnimationDriver, LodView, RigRef};
use crate::app::map_visuals::MapLibrary;
use crate::app::models::{HeroModel, ModelLibrary};
use crate::app::outlines;
use crate::app::skinning::BonePool;
use crate::app::vfx::{Anchors, UnitAnchor};
use crate::hud::{HudUnit, map_center, rendered_entity_position};

/// The animated rig of a skinned hero model with a bound animation set.
pub(crate) fn animated_rig(models: &ModelLibrary, model: Option<HeroModel>) -> Option<RigRef<'_>> {
    let skin = model?.skin?;
    Some(RigRef {
        index: skin.rig,
        skeleton: models.rig(skin.rig)?,
        graph: models.graph(skin.rig)?,
    })
}

/// Per-frame animation inputs of the network view (ADR-0012).
pub(crate) struct AnimationFrame<'a> {
    pub(crate) driver: &'a mut AnimationDriver,
    /// The camera, for the animation LOD (`None` evaluates every hero).
    pub(crate) view: Option<LodView>,
}

/// Visual height of a unit kind, metres (VFX attach at half of it).
pub(crate) fn unit_height(kind: UnitKind) -> f32 {
    match kind {
        UnitKind::Hero => 3.6,
        UnitKind::MeleeMinion => 2.2,
        UnitKind::RangedMinion => 1.8,
        UnitKind::Tower => 9.0,
        UnitKind::Nexus => 15.0,
        _ => 2.0,
    }
}

/// Rebuilds the drawn units, their bones and the VFX anchors from the latest snapshot.
/// `projectile_cubes` keeps the pre-VFX projectile placeholders (when effects are off).
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One pass over the entities.
pub(crate) fn rebuild_network_units(
    network: &NetworkView,
    models: &ModelLibrary,
    map: &MapLibrary,
    units: &mut Vec<UnitInstance>,
    bones: &mut BonePool,
    animation: AnimationFrame<'_>,
    anchors: &mut Anchors,
    projectile_cubes: bool,
) {
    units.clear();
    bones.clear();
    anchors.clear();
    let AnimationFrame { driver, view } = animation;
    driver.begin_frame();
    let Some(latest) = network.interpolator.latest() else {
        return;
    };
    let center = map_center(network);
    // Walls and props were built from the wire map when it arrived (`MapLibrary::bind`).
    map.push_instances(units);
    let render_tick = network.render_tick();
    let tick_rate = f32::from(network.client.tick_rate().max(1));
    let now = render_tick / tick_rate;
    let pinned = network.pinned_tick().is_some();
    // Outlines (ADR-0016): a spectator sees blue as the "ally" side and has no own hero.
    let own_team = if network.spectator {
        0
    } else {
        network.client.team().index() as u8
    };
    let own_id = network.own_net_id.filter(|_| !network.spectator);
    for entity in &latest.entities {
        let Some(kind) = kind_from_u8(entity.kind) else {
            continue;
        };
        let model = (kind == UnitKind::Hero)
            .then(|| models.hero(usize::from(entity.hero_pick)))
            .flatten();
        let rig = animated_rig(models, model);
        // A dead hero is drawn only while its death clip (and linger) plays.
        if entity.is_dead()
            && !rig.is_some_and(|rig| driver.visible(entity.net_id, rig.graph, now, true))
        {
            continue;
        }
        let Some(team) = team_from_u8(entity.team) else {
            continue;
        };
        let outline = outlines::slot(outlines::OutlineUnit {
            hero: kind == UnitKind::Hero,
            own: own_id == Some(entity.net_id),
            enemy: entity.team != own_team,
            hovered: network.hover == Some(entity.net_id) && !entity.is_dead(),
            target: network.attack_target == Some(entity.net_id) && !entity.is_dead(),
        });
        let first = units.len();
        let position = rendered_entity_position(network, HudUnit::new(entity), render_tick);
        let ground = Vec3::new(
            position.x.to_f32() - center,
            0.0,
            position.y.to_f32() - center,
        );
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
            // A stationary animated hero keeps its last movement (or attack) direction; the
            // WP-4.1 pose kept facing +X.
            let (facing, gait) = if speed_sq > 0.0001 {
                (dy.atan2(dx), 1.0)
            } else {
                let rest = rig.and_then(|_| driver.rest_facing(entity.net_id));
                (rest.unwrap_or(0.0), 0.0)
            };
            let pose = HeroPose {
                position,
                team,
                health_fraction: fraction,
                center,
                time: render_tick / 30.0,
                phase_offset: f32::from(entity.net_id),
                facing,
                gait,
            };
            // Speed for the locomotion blend: both ends interpolated (the own hero's drawn
            // position is predicted ahead), so it depends only on the render tick.
            anchors.push(UnitAnchor {
                id: u32::from(entity.net_id),
                position: ground,
                height: unit_height(kind),
                model: rig
                    .zip(model)
                    .map(|(rig, model)| (rig.index, hero_transform(&model, &pose))),
            });
            let animated = match (rig, model) {
                (Some(rig), Some(model)) => {
                    let here = network
                        .interpolator
                        .position(entity.net_id, render_tick)
                        .unwrap_or(position);
                    let speed = here.distance(previous).to_f32() * tick_rate / 2.0;
                    driver.animate(
                        entity.net_id,
                        rig,
                        now,
                        if entity.is_dead() { 0.0 } else { speed },
                        hero_transform(&model, &pose),
                        view.as_ref(),
                        pinned,
                    )
                }
                _ => None,
            };
            push_hero(units, bones, models, model, &pose, animated);
            outlines::mark(&mut units[first..], outline);
            continue;
        }
        anchors.push(UnitAnchor {
            id: u32::from(entity.net_id),
            position: ground,
            height: unit_height(kind),
            model: None,
        });
        push_unit(
            units,
            position,
            kind,
            team,
            fraction,
            entity.status_bits,
            center,
        );
        outlines::mark(&mut units[first..], outline);
    }
    for projectile in network.projectiles.iter().filter(|_| projectile_cubes) {
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

fn hero_color(team: Team, health_fraction: f32) -> Vec4 {
    let color = match team {
        Team::Blue => Vec4::new(0.28, 0.55, 1.0, 1.0),
        Team::Red => Vec4::new(1.0, 0.34, 0.28, 1.0),
    };
    let brightness = 0.55 + 0.45 * health_fraction.clamp(0.0, 1.0);
    Vec4::new(
        color.x * brightness,
        color.y * brightness,
        color.z * brightness,
        1.0,
    )
}

/// Presentation inputs for one hero: everything is derived from replicated state the
/// client already has (no wire fields of its own).
#[derive(Clone, Copy, Debug)]
pub(crate) struct HeroPose {
    pub(crate) position: FixVec2,
    pub(crate) team: Team,
    pub(crate) health_fraction: f32,
    /// Half the map extent: world position minus this centres the map on the origin.
    pub(crate) center: f32,
    /// Animation time in seconds (render tick / 30).
    pub(crate) time: f32,
    /// Per-unit phase so heroes do not animate in lockstep.
    pub(crate) phase_offset: f32,
    /// Movement angle in the map's (x, z) plane.
    pub(crate) facing: f32,
    /// 0 idle .. 1 walking.
    pub(crate) gait: f32,
}

/// A model's model-to-world transform for `pose` (facing the movement direction, standing
/// on the ground at the hero's position).
pub(crate) fn hero_transform(model: &HeroModel, pose: &HeroPose) -> Mat4 {
    // Models face +Z (glTF); `facing` is the movement angle in the map's (x, z) plane.
    let rotation = Quat::from_rotation_y(core::f32::consts::FRAC_PI_2 - pose.facing);
    let ground = Vec3::new(
        pose.position.x.to_f32() - pose.center,
        0.0,
        pose.position.y.to_f32() - pose.center,
    );
    Mat4::from_rotation_translation(rotation, ground) * model.normalize
}

/// Pushes a hero as its loaded model when there is one, otherwise as the procedural rig.
///
/// A skinned model uses `animated` (an animator's matrices, ADR-0012) when given, else it is
/// posed into `bones` by the WP-4.1 rule (run clip weighted by gait, rest pose when idle).
/// When the bone budget is exhausted it is drawn with its static mesh for that frame. Never
/// allocates when `units` has capacity.
pub(crate) fn push_hero(
    units: &mut Vec<UnitInstance>,
    bones: &mut BonePool,
    models: &ModelLibrary,
    model: Option<HeroModel>,
    pose: &HeroPose,
    animated: Option<&[JointMatrix]>,
) {
    let HeroPose {
        position,
        team,
        health_fraction,
        center,
        time,
        phase_offset,
        facing,
        gait,
    } = *pose;
    let Some(model) = model else {
        push_hero_rig(
            units,
            position,
            team,
            health_fraction,
            center,
            time,
            phase_offset,
            facing,
            gait,
        );
        return;
    };
    // Models face +Z (glTF); `facing` is the movement angle in the map's (x, z) plane.
    let rotation = Quat::from_rotation_y(core::f32::consts::FRAC_PI_2 - facing);
    let ground = Vec3::new(
        position.x.to_f32() - center,
        0.0,
        position.y.to_f32() - center,
    );
    let color = hero_color(team, health_fraction);
    if let Some(skin) = model.skin
        && let Some(rig) = models.rig(skin.rig)
        && let Some(range) = match animated {
            Some(matrices) => bones.push(matrices),
            None => bones.pose(rig, time, phase_offset, gait),
        }
    {
        // The clip animates the body (including the hip bob), so no procedural offset.
        let transform = hero_transform(&model, pose);
        units.push(
            UnitInstance::new(transform, color)
                .with_mesh(skin.mesh)
                .with_bones(range),
        );
        return;
    }
    // A static mesh has no joints; a small stride bob keeps moving heroes readable.
    let bob = (time * 12.0 + phase_offset).sin().abs() * 0.12 * gait.clamp(0.0, 1.0);
    let base = ground + Vec3::Y * bob;
    let transform = Mat4::from_rotation_translation(rotation, base) * model.normalize;
    units.push(UnitInstance::new(transform, color).with_mesh(model.mesh));
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
    let color = hero_color(team, health_fraction);
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
        // `UnitKind` is `#[non_exhaustive]`; an unknown kind draws as a plain team-coloured
        // minion-sized box.
        _ => (2.0, team_color),
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
    if network.overview_camera {
        return Camera::orbit(Vec3::ZERO, 240.0, -0.7, 0.92, aspect);
    }
    let predicted = network.prediction.position();
    let world_center = map_center(network);
    let center = Vec3::new(
        predicted.x.to_f32() - world_center,
        0.0,
        predicted.y.to_f32() - world_center,
    );
    Camera::orbit(center, network.follow_distance, -0.7, 0.95, aspect)
}

#[cfg(test)]
mod tests {
    use moba_core::FixVec2;
    use moba_game::Team;
    use moba_render::{MeshData, MeshId, UnitInstance};

    use super::{HeroPose, push_hero};
    use crate::app::models::{ModelBindings, ModelLibrary, Upload};
    use crate::app::skinning::BonePool;

    const SKINNED_SAMPLE: &[u8] = include_bytes!("../../../../content/models/sentinel-skinned.glb");

    /// The whole per-frame skinning path (clear, pose, push, and truncation to the static
    /// mesh) must not allocate once its buffers exist.
    #[test]
    fn skinned_hero_frames_do_not_allocate() {
        let root = std::env::temp_dir().join(format!(
            "moba-skinning-alloc-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("models")).expect("temp model dir");
        std::fs::write(root.join("models/walker.glb"), SKINNED_SAMPLE).expect("write sample");
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        library.bind_heroes([Some("walker")], |_: &MeshData, _: Upload| Ok(MeshId::CUBE));
        let model = library.hero(0);
        assert!(model.is_some_and(|model| model.skin.is_some()));
        let joints = 10;
        let budget_heroes = 16;
        let heroes = 20i32;
        let mut bones = BonePool::new(joints * budget_heroes);
        let mut units: Vec<UnitInstance> = Vec::with_capacity(64);
        let frame = |bones: &mut BonePool, units: &mut Vec<UnitInstance>, tick: u16| {
            bones.clear();
            units.clear();
            for hero in 0..heroes {
                push_hero(
                    units,
                    bones,
                    &library,
                    model,
                    &HeroPose {
                        position: FixVec2::from_ints(100 + hero, 120),
                        team: if hero % 2 == 0 { Team::Blue } else { Team::Red },
                        health_fraction: 0.8,
                        center: 128.0,
                        time: f32::from(tick) / 30.0,
                        phase_offset: hero as f32,
                        facing: 0.3 * hero as f32,
                        gait: (hero % 2) as f32,
                    },
                    None,
                );
            }
        };
        // Warm-up frame: the first truncation logs once (stderr) before counting starts.
        frame(&mut bones, &mut units, 0);
        let ((), allocations) = crate::app::allocation_probe::probe(|| {
            for tick in 1..=30 {
                frame(&mut bones, &mut units, tick);
            }
        });
        assert_eq!(allocations, 0, "per-frame skinning allocated");
        let skinned = units.iter().filter(|unit| unit.bones.count > 0).count();
        assert_eq!(skinned, budget_heroes);
        assert_eq!(units.len(), 20, "over-budget heroes draw static");
        assert_eq!(bones.truncated(), 4);
        assert_eq!(bones.bones().len(), joints * budget_heroes);
        let _ = std::fs::remove_dir_all(root);
    }
}
