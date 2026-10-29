use std::path::PathBuf;

use moba_core::math::{Mat4, Vec3};
use moba_render::{Camera, ParticleBlend, ParticleRole, ParticleShape, QualityTier};

use super::{Anchors, Trigger, UnitAnchor, VfxEvent, VfxLibrary, VfxSystem, VfxView};

const BINDINGS: &str = r#"
[defaults]
attack = "spark"
minion_death = "spark"
hero_death = "telegraph"

[[abilities]]
name = "Bolt"
cast = "flash"
projectile = "bolt"
hit = "flash"
tint = [2.0, 1.0, 1.0]

[[abilities]]
name = "Other"
cast = "spark"
hit = "telegraph"
"#;

const FLASH: &str = r#"
duration = 0.2
importance = "high"
[[emitters]]
burst = 10
lifetime = [0.5, 0.5]
size = [1, 1]
color = [1, 0.5, 0.25, 1]
blend = "alpha"
anchor = "source"
"#;

const BOLT: &str = r#"
duration = 0.1
[[emitters]]
primitive = "trail"
lifetime = [0.3, 0.3]
size = [1, 1]
width = 0.5
color = [0.5, 0.5, 1, 1]
rate = 1
[[emitters]]
rate = 60
lifetime = [0.2, 0.2]
size = [0.5, 0.5]
color = [1, 1, 1, 1]
"#;

const SPARK: &str = r#"
duration = 0.0
importance = "low"
[[emitters]]
burst = 6
lifetime = [0.3, 0.3]
size = [0.3, 0.3]
color = [1, 1, 1, 1]
"#;

const TELEGRAPH: &str = r#"
duration = 2.0
[[emitters]]
primitive = "ring"
burst = 1
lifetime = [2, 2]
size = [2, 2]
color = [1, 0.2, 0.2, 0.1]
telegraph = true
scale_with_radius = true
"#;

fn root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "moba-vfx-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("vfx")).expect("vfx dir");
    for (name, text) in [
        ("bindings", BINDINGS),
        ("flash", FLASH),
        ("bolt", BOLT),
        ("spark", SPARK),
        ("telegraph", TELEGRAPH),
    ] {
        std::fs::write(root.join(format!("vfx/{name}.toml")), text).expect("write");
    }
    root
}

fn view(team: Option<u8>) -> VfxView {
    let camera = Camera::perspective(
        Vec3::new(0.0, 30.0, 25.0),
        Vec3::ZERO,
        Vec3::Y,
        1.0,
        16.0 / 9.0,
        0.1,
        1_000.0,
    );
    VfxView::new(&camera, [1_280.0, 720.0], team, Some(1))
}

fn anchors() -> Anchors {
    let mut anchors = Anchors::new();
    for (id, x) in [(1, -4.0), (2, 4.0), (3, 0.0)] {
        anchors.push(UnitAnchor {
            id,
            position: Vec3::new(x, 0.0, 0.0),
            height: 2.0,
            model: None,
        });
    }
    anchors
}

fn event(trigger: Trigger, team: u8, source: Option<u32>) -> VfxEvent {
    VfxEvent {
        trigger,
        team,
        source,
        target: Some(2),
        position: Vec3::new(4.0, 0.0, 0.0),
        end: Vec3::new(10.0, 0.0, 0.0),
        direction: Vec3::X,
        speed: 20.0,
        radius: 3.0,
        seconds: 0.0,
        arc: 0.0,
        ability: None,
        effect: None,
    }
}

fn system(tag: &str) -> (VfxSystem, PathBuf) {
    let root = root(tag);
    let library = VfxLibrary::load(root.clone());
    (
        VfxSystem::new(library, QualityTier::High.vfx_budget(), true),
        root,
    )
}

const NO_SOCKET: &dyn Fn(u32, &str) -> Option<Vec3> = &|_, _| None;

#[test]
fn casts_projectiles_and_hits_spawn_their_bound_effects() {
    let (mut vfx, root) = system("cast");
    let bolt = vfx.library().ability("bolt");
    assert_eq!(bolt, Some(0));
    let view = view(Some(0));
    let anchors = anchors();
    let mut cast = event(Trigger::Cast, 0, Some(5));
    cast.ability = bolt;
    vfx.queue(10, cast);
    vfx.queue(10, event(Trigger::ProjectileSpawn, 0, Some(5)));
    // Not yet: the view is behind the event's tick.
    vfx.update(1.0 / 30.0, 9.0, &view, &anchors, NO_SOCKET);
    assert_eq!(vfx.frame().spawns.len(), 0);
    vfx.update(1.0 / 30.0, 10.0, &view, &anchors, NO_SOCKET);
    let frame = vfx.frame();
    // The cast burst (an ally's: alpha capped and dimmable) plus the projectile's rate.
    let burst: Vec<_> = frame
        .spawns
        .iter()
        .filter(|spawn| spawn.blend == ParticleBlend::Alpha)
        .collect();
    assert_eq!(burst.len(), 10);
    assert!(burst.iter().all(|spawn| spawn.role == ParticleRole::Ally));
    assert!(
        burst
            .iter()
            .all(|spawn| spawn.color.w <= super::ALLY_MAX_ALPHA)
    );
    // The binding's tint doubles red.
    assert!((burst[0].color.x - 2.0).abs() < 1e-5);
    assert_eq!(vfx.counters().instances, 2);
    // The projectile flies: its trail emits segments from the previous position.
    let mut segments = 0;
    for frame in 0..6 {
        vfx.update(1.0 / 30.0, 11.0 + frame as f32, &view, &anchors, NO_SOCKET);
        segments += vfx
            .frame()
            .spawns
            .iter()
            .filter(|spawn| spawn.shape == ParticleShape::Segment)
            .count();
    }
    assert_eq!(segments, 6, "one trail segment per frame of flight");
    // The hit resolves to the owner's last cast (the hit effect) and stops the flight.
    vfx.queue(17, event(Trigger::ProjectileHit, 0, Some(5)));
    vfx.update(1.0 / 30.0, 17.0, &view, &anchors, NO_SOCKET);
    let hits = vfx
        .frame()
        .spawns
        .iter()
        .filter(|spawn| spawn.blend == ParticleBlend::Alpha)
        .count();
    assert_eq!(hits, 10);
    for frame in 0..30 {
        vfx.update(1.0 / 30.0, 18.0 + frame as f32, &view, &anchors, NO_SOCKET);
    }
    assert_eq!(vfx.counters().instances, 0, "every effect ended");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn enemy_telegraphs_survive_budgets_and_low_importance_is_culled_first() {
    let (mut vfx, root) = system("budget");
    let mut budget = QualityTier::Low.vfx_budget();
    budget.max_particles = 14;
    vfx.set_budget(budget);
    let view = view(Some(0));
    let anchors = anchors();
    for _ in 0..4 {
        vfx.trigger(&event(Trigger::Attack, 1, Some(3)), &view, &anchors);
    }
    vfx.update(1.0 / 30.0, 0.0, &view, &anchors, NO_SOCKET);
    // 50 % of 14 particles admits two bursts of six (the estimate counts as it spawns).
    assert_eq!(vfx.frame().spawns.len(), 12);
    assert_eq!(vfx.counters().culled_budget, 12);
    // An enemy hero death (a telegraph ring) still spawns, flagged for the opacity floor.
    vfx.trigger(&event(Trigger::HeroDeath, 1, None), &view, &anchors);
    vfx.update(1.0 / 30.0, 1.0, &view, &anchors, NO_SOCKET);
    let ring = vfx.frame().spawns[0];
    assert_eq!(ring.role, ParticleRole::EnemyTelegraph);
    assert_eq!(ring.shape, ParticleShape::Ground);
    assert!((ring.size - 6.0).abs() < 1e-5, "scaled by the event radius");
    // The same ring of an ally is neither floored nor exempt.
    vfx.trigger(&event(Trigger::HeroDeath, 0, None), &view, &anchors);
    vfx.update(1.0 / 30.0, 2.0, &view, &anchors, NO_SOCKET);
    assert_eq!(
        vfx.frame().spawns.len(),
        0,
        "over budget: an ally ring is culled"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn far_and_off_screen_effects_spawn_nothing_and_spectators_get_no_roles() {
    let (mut vfx, root) = system("cull");
    let view = view(None);
    let mut anchors = anchors();
    anchors.push(UnitAnchor {
        id: 9,
        position: Vec3::new(0.0, 0.0, 900.0),
        height: 2.0,
        model: None,
    });
    let mut far = event(Trigger::Attack, 1, Some(3));
    far.target = Some(9);
    vfx.trigger(&far, &view, &anchors);
    vfx.trigger(&event(Trigger::Attack, 1, Some(3)), &view, &anchors);
    vfx.update(1.0 / 30.0, 0.0, &view, &anchors, NO_SOCKET);
    assert_eq!(vfx.frame().spawns.len(), 6, "only the near burst");
    assert_eq!(vfx.counters().culled_distance, 1);
    assert!(
        vfx.frame()
            .spawns
            .iter()
            .all(|spawn| spawn.role == ParticleRole::Neutral)
    );
    // An attack on a unit this client cannot see draws nothing.
    let mut hidden = event(Trigger::Attack, 1, Some(3));
    hidden.target = Some(77);
    vfx.trigger(&hidden, &view, &anchors);
    assert_eq!(vfx.counters().instances, 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sockets_move_source_anchored_emitters() {
    let root = root("socket");
    std::fs::write(
        root.join("vfx/bindings.toml"),
        "[[abilities]]\nname = \"Bolt\"\ncast = \"flash\"\ncast_socket = \"spear_tip\"\n",
    )
    .expect("bindings");
    let mut vfx = VfxSystem::new(
        VfxLibrary::load(root.clone()),
        QualityTier::High.vfx_budget(),
        true,
    );
    let view = view(Some(0));
    let anchors = anchors();
    let mut cast = event(Trigger::Cast, 0, Some(1));
    cast.ability = Some(0);
    vfx.trigger(&cast, &view, &anchors);
    let tip = Vec3::new(-4.0, 3.5, 0.5);
    let socket: &dyn Fn(u32, &str) -> Option<Vec3> =
        &|unit, name| (unit == 1 && name == "spear_tip").then_some(tip);
    vfx.update(1.0 / 30.0, 0.0, &view, &anchors, socket);
    assert!(
        vfx.frame().spawns.iter().all(|spawn| spawn.position == tip),
        "the cast burst starts at the socket"
    );
    // The viewer's own effects are never dimmed.
    assert!(
        vfx.frame()
            .spawns
            .iter()
            .all(|spawn| spawn.role == ParticleRole::Neutral)
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn steady_state_frames_do_not_allocate() {
    let (mut vfx, root) = system("alloc");
    let view = view(Some(0));
    let anchors = anchors();
    let mut cast = event(Trigger::Cast, 0, Some(5));
    cast.ability = Some(0);
    // Warm-up: fill and drain every queue once.
    for tick in 0..60u32 {
        vfx.queue(tick, cast);
        vfx.queue(tick, event(Trigger::ProjectileSpawn, 0, Some(5)));
        vfx.queue(tick, event(Trigger::Attack, 1, Some(3)));
        vfx.update(1.0 / 30.0, tick as f32, &view, &anchors, NO_SOCKET);
    }
    let ((), allocations) = crate::app::allocation_probe::probe(|| {
        for tick in 60..360u32 {
            vfx.queue_unstamped(cast);
            vfx.queue_unstamped(event(Trigger::ProjectileSpawn, 0, Some(5)));
            vfx.queue_unstamped(event(Trigger::HeroDeath, 1, None));
            vfx.stamp(tick);
            vfx.update(1.0 / 30.0, tick as f32, &view, &anchors, NO_SOCKET);
        }
    });
    assert_eq!(allocations, 0, "per-frame VFX allocated");
    assert!(vfx.counters().spawned > 0);
    let _ = std::fs::remove_dir_all(root);
    let _ = Mat4::IDENTITY;
}

/// The authoring preview replays every shipped effect: each one spawns particles.
#[test]
fn preview_plays_every_shipped_effect() {
    let content = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"));
    let bindings =
        moba_assets::VfxBindings::load_file(&content.join("vfx/bindings.toml")).expect("bindings");
    for name in bindings.effect_names() {
        let library = VfxLibrary::preview(content.clone(), name);
        let effect = library.preview_effect().expect("the effect loads");
        let mut vfx = VfxSystem::new(library, QualityTier::High.vfx_budget(), true);
        let mut preview = super::Preview::new(effect);
        let mut anchors = Anchors::new();
        let mut units = Vec::new();
        let view = view(Some(0));
        let mut total = 0;
        for _ in 0..10 {
            anchors.clear();
            units.clear();
            preview.frame(&mut vfx, &mut anchors, &mut units, 1_000.0 / 30.0);
            vfx.update(1.0 / 30.0, 0.0, &view, &anchors, NO_SOCKET);
            total += vfx.frame().spawns.len();
        }
        assert!(total > 0, "{name}: no particles ({:?})", vfx.counters());
        assert_eq!(units.len(), 2, "the stand-in caster and target");
    }
}

fn segments(vfx: &VfxSystem) -> usize {
    vfx.frame()
        .spawns
        .iter()
        .filter(|spawn| spawn.shape == ParticleShape::Segment)
        .count()
}

/// Events of one tick play in arrival order: the projectile follows the cast that fired it.
#[test]
fn same_tick_events_release_in_arrival_order() {
    let (mut vfx, root) = system("order");
    let view = view(Some(0));
    let anchors = anchors();
    let mut cast = event(Trigger::Cast, 0, Some(5));
    cast.ability = vfx.library().ability("Bolt");
    for queued in [
        event(Trigger::Attack, 1, Some(3)),
        cast,
        event(Trigger::ProjectileSpawn, 0, Some(5)),
        event(Trigger::Attack, 1, Some(3)),
    ] {
        vfx.queue(10, queued);
    }
    let mut trail = 0;
    for frame in 0..7 {
        vfx.update(1.0 / 30.0, 10.0 + frame as f32, &view, &anchors, NO_SOCKET);
        trail += segments(&vfx);
    }
    assert_eq!(
        trail, 6,
        "the bolt's trail: the spawn found the cast before it"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// A hit plays the effect of the projectile that landed, not of the owner's latest cast.
#[test]
fn hits_play_the_landing_projectiles_ability() {
    let (mut vfx, root) = system("hit-ability");
    let view = view(Some(0));
    let anchors = anchors();
    let mut bolt = event(Trigger::Cast, 0, Some(5));
    bolt.ability = vfx.library().ability("Bolt");
    let mut other = event(Trigger::Cast, 0, Some(5));
    other.ability = vfx.library().ability("Other");
    vfx.queue(1, bolt);
    vfx.queue(1, event(Trigger::ProjectileSpawn, 0, Some(5)));
    vfx.update(1.0 / 30.0, 1.0, &view, &anchors, NO_SOCKET);
    vfx.queue(2, other);
    vfx.update(1.0 / 30.0, 2.0, &view, &anchors, NO_SOCKET);
    vfx.queue(3, event(Trigger::ProjectileHit, 0, Some(5)));
    vfx.update(1.0 / 30.0, 3.0, &view, &anchors, NO_SOCKET);
    let spawns = vfx.frame().spawns;
    let alpha = spawns
        .iter()
        .filter(|spawn| spawn.blend == ParticleBlend::Alpha)
        .count();
    assert_eq!(alpha, 10, "Bolt's hit (flash)");
    assert!(
        spawns
            .iter()
            .all(|spawn| spawn.shape != ParticleShape::Ground),
        "not Other's hit (a ring)"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// (a) A missing or invalid bindings file turns effects off, so projectiles keep their cubes.
#[test]
fn missing_or_invalid_bindings_disable_effects() {
    let root = root("no-bindings");
    std::fs::remove_file(root.join("vfx/bindings.toml")).expect("remove");
    let budget = QualityTier::High.vfx_budget();
    assert!(!VfxSystem::new(VfxLibrary::load(root.clone()), budget, true).enabled());
    std::fs::write(root.join("vfx/bindings.toml"), "[defaults]\ncast = 3\n").expect("write");
    assert!(!VfxSystem::new(VfxLibrary::load(root.clone()), budget, true).enabled());
    std::fs::write(root.join("vfx/bindings.toml"), BINDINGS).expect("write");
    assert!(VfxSystem::new(VfxLibrary::load(root.clone()), budget, true).enabled());
    let preview = VfxLibrary::preview(root.clone(), "missing");
    assert!(!VfxSystem::new(preview, budget, true).enabled());
    let _ = std::fs::remove_dir_all(root);
}

/// (b) A zoomed-out spectator camera (240 m) still sees effects near its focus, and every
/// projectile: distances are measured from the focus and scale with the zoom.
#[test]
fn far_spectator_cameras_cull_from_their_focus() {
    let (mut vfx, root) = system("spectator");
    let camera = Camera::orbit(Vec3::ZERO, 240.0, -0.7, 0.92, 16.0 / 9.0);
    let view = VfxView::new(&camera, [1_280.0, 720.0], None, None);
    assert!(view.focus.length() < 0.5, "the focus is the orbit target");
    assert!(view.eye.distance(Vec3::ZERO) > QualityTier::High.vfx_budget().max_distance);
    let mut anchors = anchors();
    anchors.push(UnitAnchor {
        id: 9,
        position: Vec3::new(60.0, 0.0, 0.0),
        height: 2.0,
        model: None,
    });
    let mut attack = event(Trigger::Attack, 1, Some(3));
    attack.target = Some(9);
    vfx.trigger(&attack, &view, &anchors);
    let mut cast = event(Trigger::Cast, 1, Some(5));
    cast.ability = vfx.library().ability("Bolt");
    vfx.trigger(&cast, &view, &anchors);
    vfx.trigger(
        &event(Trigger::ProjectileSpawn, 1, Some(5)),
        &view,
        &anchors,
    );
    vfx.update(1.0 / 30.0, 0.0, &view, &anchors, NO_SOCKET);
    assert!(vfx.frame().spawns.len() >= 6, "the attack sparks spawned");
    assert_eq!(vfx.counters().culled_distance, 0);
    vfx.update(1.0 / 30.0, 1.0, &view, &anchors, NO_SOCKET);
    assert_eq!(segments(&vfx), 1, "the projectile's trail");
    assert_eq!(vfx.placeholders().len(), 0);
    let _ = std::fs::remove_dir_all(root);
}

/// (c) Budget pressure never hides an enemy projectile; an ally's falls back to its cube,
/// as does a projectile whose effect did not resolve.
#[test]
fn projectiles_survive_budgets_or_fall_back_to_cubes() {
    let (mut vfx, root) = system("projectile-budget");
    let mut budget = QualityTier::Low.vfx_budget();
    budget.max_particles = 14;
    vfx.set_budget(budget);
    let view = view(Some(0));
    let anchors = anchors();
    // Fill the particle budget with low-importance sparks.
    for _ in 0..4 {
        vfx.trigger(&event(Trigger::Attack, 1, Some(3)), &view, &anchors);
    }
    vfx.update(1.0 / 30.0, 0.0, &view, &anchors, NO_SOCKET);
    for (team, source) in [(1u8, 5u32), (0, 6)] {
        let mut cast = event(Trigger::Cast, team, Some(source));
        cast.ability = vfx.library().ability("Bolt");
        vfx.trigger(&cast, &view, &anchors);
        vfx.trigger(
            &event(Trigger::ProjectileSpawn, team, Some(source)),
            &view,
            &anchors,
        );
    }
    // No cast, and the test bindings have no default projectile: unresolved.
    vfx.trigger(
        &event(Trigger::ProjectileSpawn, 1, Some(7)),
        &view,
        &anchors,
    );
    let mut enemy_trail = 0;
    let mut placeholders = 0;
    for frame in 0..6 {
        vfx.update(1.0 / 30.0, 1.0 + frame as f32, &view, &anchors, NO_SOCKET);
        enemy_trail += segments(&vfx);
        placeholders = vfx.placeholders().len();
    }
    assert_eq!(enemy_trail, 5, "the enemy bolt's trail ignores the budget");
    assert_eq!(
        placeholders, 2,
        "the culled ally bolt and the unresolved projectile"
    );
    assert!(
        vfx.frame()
            .spawns
            .iter()
            .all(|spawn| spawn.role != ParticleRole::Ally),
        "nothing of the ally's bolt spawned"
    );
    let _ = std::fs::remove_dir_all(root);
}
