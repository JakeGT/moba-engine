//! Presentation events as VFX triggers: from the wire (`NetEvent`, network play) and from a
//! local simulation (`SimEvent`, the preview and replay playback). Presentation only.

use moba_core::FixVec2;
use moba_core::math::Vec3;
use moba_net::protocol::{
    EVENT_ATTACK, EVENT_CAST, EVENT_PROJECTILE_HIT, EVENT_PROJECTILE_SPAWN, EVENT_RECALL,
    EVENT_STRUCTURE_DESTROYED, EVENT_UNIT_DIED, EVENT_ZONE_SPAWNED, NetEvent, RECALL_STARTED,
    dequantize_position,
};
use moba_sim::{SimEvent, UnitKind};

/// What happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trigger {
    Cast,
    ProjectileSpawn,
    ProjectileHit,
    Zone,
    HeroDeath,
    MinionDeath,
    StructureDestroyed,
    Attack,
    /// A hero started recalling: the recall effect plays at it for the channel.
    Recall,
    /// A hero's recall was cancelled or completed: its recall effect stops emitting.
    RecallEnd,
}

/// One trigger with everything an effect needs, in world space (metres, map centred on the
/// origin, +Y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VfxEvent {
    pub(crate) trigger: Trigger,
    /// Team of the caster, owner, attacker or victim (0 blue, 1 red).
    pub(crate) team: u8,
    /// Caster, owner or attacker (a net id, or a local entity index).
    pub(crate) source: Option<u32>,
    /// Hit or attacked unit.
    pub(crate) target: Option<u32>,
    /// Cast target, spawn, impact, death or zone centre, on the ground.
    pub(crate) position: Vec3,
    /// A projectile's end point.
    pub(crate) end: Vec3,
    /// A projectile's unit direction.
    pub(crate) direction: Vec3,
    /// A projectile's speed, metres per second.
    pub(crate) speed: f32,
    /// A zone's (or projectile's) radius, metres.
    pub(crate) radius: f32,
    /// A zone's lifetime, seconds.
    pub(crate) seconds: f32,
    /// A projectile's visual arc height, metres.
    pub(crate) arc: f32,
    /// The ability's binding in the library (casts; other ability events find it through
    /// the source's last cast).
    pub(crate) ability: Option<u16>,
    /// Plays this effect instead of the bindings' choice (the authoring preview).
    pub(crate) effect: Option<super::EffectId>,
}

impl VfxEvent {
    fn new(trigger: Trigger, team: u8, position: Vec3) -> Self {
        Self {
            trigger,
            team,
            source: None,
            target: None,
            position,
            end: position,
            direction: Vec3::ZERO,
            speed: 0.0,
            radius: 0.0,
            seconds: 0.0,
            arc: 0.0,
            ability: None,
            effect: None,
        }
    }
}

/// A map position in world space.
pub(crate) fn world(position: FixVec2, center: f32) -> Vec3 {
    Vec3::new(
        position.x.to_f32() - center,
        0.0,
        position.y.to_f32() - center,
    )
}

const NO_ENTITY: u16 = 0xFFFF;

/// Maps a replicated event. `ability` resolves a cast's `(source net id, slot)` to its
/// library binding. Unit kinds follow the wire (`0` hero, `1`/`2` minions); towers and the
/// nexus come through `EVENT_STRUCTURE_DESTROYED`.
pub(crate) fn from_net(
    event: &NetEvent,
    center: f32,
    tick_rate: f32,
    ability: impl FnOnce(u16, u8) -> Option<u16>,
) -> Option<VfxEvent> {
    let at = |x, y| world(dequantize_position(x, y), center);
    let id = |net_id: u16| (net_id != NO_ENTITY).then_some(u32::from(net_id));
    let mut out = match event.kind {
        EVENT_CAST => {
            let mut out = VfxEvent::new(Trigger::Cast, event.team, at(event.x, event.y));
            out.ability = id(event.source)
                .and_then(|_| ability(event.source, u8::try_from(event.value).unwrap_or(u8::MAX)));
            out
        }
        EVENT_PROJECTILE_SPAWN => {
            let mut out = VfxEvent::new(Trigger::ProjectileSpawn, event.team, at(event.x, event.y));
            out.end = at(event.x2, event.y2);
            out.direction = Vec3::new(
                f32::from(event.dx) / 32_767.0,
                0.0,
                f32::from(event.dy) / 32_767.0,
            )
            .normalize_or_zero();
            out.speed = f32::from(event.value);
            out.radius = f32::from(event.aux);
            out.arc = f32::from(event.arc) / 16.0;
            out
        }
        EVENT_PROJECTILE_HIT => {
            VfxEvent::new(Trigger::ProjectileHit, event.team, at(event.x, event.y))
        }
        EVENT_UNIT_DIED => {
            let trigger = match event.value {
                0 => Trigger::HeroDeath,
                1 | 2 => Trigger::MinionDeath,
                _ => return None,
            };
            VfxEvent::new(trigger, event.team, at(event.x, event.y))
        }
        EVENT_STRUCTURE_DESTROYED => VfxEvent::new(
            Trigger::StructureDestroyed,
            event.team,
            at(event.x, event.y),
        ),
        EVENT_ZONE_SPAWNED => {
            let mut out = VfxEvent::new(Trigger::Zone, event.team, at(event.x, event.y));
            out.radius = f32::from(event.value);
            out.seconds = f32::from(event.aux) / tick_rate.max(1.0);
            out
        }
        // Basic attacks carry only the two units; the hit lands at the target.
        EVENT_ATTACK => VfxEvent::new(Trigger::Attack, event.team, Vec3::ZERO),
        EVENT_RECALL if event.value == RECALL_STARTED => {
            let mut out = VfxEvent::new(Trigger::Recall, event.team, at(event.x, event.y));
            out.seconds = f32::from(event.aux) / tick_rate.max(1.0);
            out
        }
        EVENT_RECALL => VfxEvent::new(Trigger::RecallEnd, event.team, at(event.x, event.y)),
        _ => return None,
    };
    out.source = id(event.source);
    out.target = id(event.target);
    Some(out)
}

/// Maps a local simulation event. `entity` turns an entity into the id the anchors use;
/// `ability` resolves a cast's `(caster, slot)` to its library binding.
#[allow(clippy::too_many_lines)] // One exhaustive SimEvent mapping.
pub(crate) fn from_sim(
    event: &SimEvent,
    center: f32,
    tick_rate: f32,
    entity: impl Fn(moba_ecs::Entity) -> u32,
    ability: impl FnOnce(moba_ecs::Entity, u8) -> Option<u16>,
) -> Option<VfxEvent> {
    let team = |team: moba_game::Team| u8::try_from(team.index()).unwrap_or(0);
    let out = match *event {
        SimEvent::AbilityCast {
            caster,
            team: owner,
            ability: slot,
            target,
        } => {
            let mut out = VfxEvent::new(Trigger::Cast, team(owner), world(target, center));
            out.source = Some(entity(caster));
            out.ability = ability(caster, slot);
            out
        }
        SimEvent::ProjectileSpawned {
            owner,
            team: owner_team,
            position,
            direction,
            speed_per_sec,
            radius,
            range,
            arc,
        } => {
            let mut out = VfxEvent::new(
                Trigger::ProjectileSpawn,
                team(owner_team),
                world(position, center),
            );
            out.source = Some(entity(owner));
            out.end = world(position + direction * range, center);
            out.direction =
                Vec3::new(direction.x.to_f32(), 0.0, direction.y.to_f32()).normalize_or_zero();
            out.speed = speed_per_sec.to_f32();
            out.radius = radius.to_f32();
            out.arc = arc.to_f32();
            out
        }
        SimEvent::ProjectileHit {
            owner,
            team: owner_team,
            target,
            position,
        } => {
            let mut out = VfxEvent::new(
                Trigger::ProjectileHit,
                team(owner_team),
                world(position, center),
            );
            out.source = Some(entity(owner));
            out.target = Some(entity(target));
            out
        }
        SimEvent::ZoneSpawned {
            owner,
            team: owner_team,
            position,
            radius,
            duration_ticks,
        } => {
            let mut out = VfxEvent::new(Trigger::Zone, team(owner_team), world(position, center));
            out.source = Some(entity(owner));
            out.radius = radius.to_f32();
            out.seconds = duration_ticks as f32 / tick_rate.max(1.0);
            out
        }
        SimEvent::UnitDied {
            entity: victim,
            kind,
            team: victim_team,
            position,
            killer,
            ..
        } => {
            let trigger = match kind {
                UnitKind::Hero => Trigger::HeroDeath,
                UnitKind::MeleeMinion | UnitKind::RangedMinion => Trigger::MinionDeath,
                _ => return None,
            };
            let mut out = VfxEvent::new(trigger, team(victim_team), world(position, center));
            out.source = killer.map(&entity);
            out.target = Some(entity(victim));
            out
        }
        SimEvent::StructureDestroyed {
            team: owner_team,
            position,
            ..
        } => VfxEvent::new(
            Trigger::StructureDestroyed,
            team(owner_team),
            world(position, center),
        ),
        SimEvent::BasicAttack {
            attacker,
            team: attacker_team,
            target,
            ..
        } => {
            let mut out = VfxEvent::new(Trigger::Attack, team(attacker_team), Vec3::ZERO);
            out.source = Some(entity(attacker));
            out.target = Some(entity(target));
            out
        }
        SimEvent::RecallStarted {
            hero,
            team: hero_team,
            position,
            duration_ticks,
        } => {
            let mut out = VfxEvent::new(Trigger::Recall, team(hero_team), world(position, center));
            out.source = Some(entity(hero));
            out.seconds = duration_ticks as f32 / tick_rate.max(1.0);
            out
        }
        SimEvent::RecallEnded {
            hero,
            team: hero_team,
            position,
            ..
        } => {
            let mut out =
                VfxEvent::new(Trigger::RecallEnd, team(hero_team), world(position, center));
            out.source = Some(entity(hero));
            out
        }
        _ => return None,
    };
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_net::protocol::quantize_position;

    #[test]
    fn wire_events_map_to_world_space_triggers() {
        let (x, y) = quantize_position(FixVec2::from_ints(138, 118));
        let (x2, y2) = quantize_position(FixVec2::from_ints(148, 118));
        let event = NetEvent {
            kind: EVENT_PROJECTILE_SPAWN,
            team: 1,
            source: 7,
            target: NO_ENTITY,
            source_slot: 2,
            target_slot: 0xFF,
            x,
            y,
            x2,
            y2,
            value: 24,
            aux: 1,
            dx: 32_767,
            dy: 0,
            arc: 0,
        };
        let mapped = from_net(&event, 128.0, 30.0, |_, _| None).expect("projectile");
        assert_eq!(mapped.trigger, Trigger::ProjectileSpawn);
        assert_eq!(mapped.source, Some(7));
        assert_eq!(mapped.target, None);
        assert!((mapped.position - Vec3::new(10.0, 0.0, -10.0)).length() < 0.1);
        assert!((mapped.end - Vec3::new(20.0, 0.0, -10.0)).length() < 0.1);
        assert_eq!(mapped.direction, Vec3::X);
        assert_eq!(mapped.speed, 24.0);
        let cast = NetEvent {
            kind: EVENT_CAST,
            value: 1,
            ..event
        };
        let mapped = from_net(&cast, 128.0, 30.0, |source, slot| {
            assert_eq!((source, slot), (7, 1));
            Some(3)
        })
        .expect("cast");
        assert_eq!(mapped.ability, Some(3));
        let zone = NetEvent {
            kind: EVENT_ZONE_SPAWNED,
            value: 4,
            aux: 90,
            ..event
        };
        let mapped = from_net(&zone, 128.0, 30.0, |_, _| None).expect("zone");
        assert_eq!((mapped.radius, mapped.seconds), (4.0, 3.0));
        let tower = NetEvent {
            kind: EVENT_UNIT_DIED,
            value: 3,
            ..event
        };
        assert!(from_net(&tower, 128.0, 30.0, |_, _| None).is_none());
        let recall = NetEvent {
            kind: EVENT_RECALL,
            value: RECALL_STARTED,
            aux: 240,
            ..event
        };
        let mapped = from_net(&recall, 128.0, 30.0, |_, _| None).expect("recall");
        assert_eq!(
            (mapped.trigger, mapped.seconds, mapped.source),
            (Trigger::Recall, 8.0, Some(7))
        );
        assert!((mapped.position - Vec3::new(10.0, 0.0, -10.0)).length() < 0.1);
        for value in [
            moba_net::protocol::RECALL_CANCELLED,
            moba_net::protocol::RECALL_COMPLETED,
        ] {
            let end = NetEvent { value, ..recall };
            let mapped = from_net(&end, 128.0, 30.0, |_, _| None).expect("recall end");
            assert_eq!(
                (mapped.trigger, mapped.source),
                (Trigger::RecallEnd, Some(7))
            );
        }
        let unknown = NetEvent { kind: 99, ..event };
        assert!(from_net(&unknown, 128.0, 30.0, |_, _| None).is_none());
    }
}
