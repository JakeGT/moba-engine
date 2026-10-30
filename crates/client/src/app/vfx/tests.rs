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
[light]
color = [1, 0.5, 0.25]
intensity = 4
radius = 6
anchor = "source"
fade_out = 0.1
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
[[decals]]
shape = "splat"
radius = 0.8
lifetime = 5.0
fade_out = 1.0
color = [0.05, 0.03, 0.02, 0.8]
stamp = true
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
[[decals]]
shape = "ring"
radius = 1.0
width = 0.2
scale_with_radius = true
color = [2, 0.4, 0.2, 0.3]
telegraph = true
progress = true
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

/// R3b (ADR-0017 §1): effect decals follow the telegraph rules. Enemy telegraph decals are
/// listed first, never culled (even far away) and keep full strength; an ally's are dimmed;
/// stamped scorch marks outlive their effect in a bounded ring and then expire.
#[test]
fn effect_decals_list_enemy_telegraphs_first_and_stamps_outlive_their_effect() {
    use moba_render::{DecalRole, DecalShape, DecalStyle};
    let (mut vfx, root) = system("decals");
    vfx.set_ally_opacity(0.5);
    let view = view(Some(0));
    let anchors = anchors();
    // Cosmetic attacks first (their stamps), then an ally and a far enemy telegraph.
    vfx.trigger(&event(Trigger::Attack, 1, Some(3)), &view, &anchors);
    vfx.trigger(&event(Trigger::HeroDeath, 0, None), &view, &anchors);
    let mut far = event(Trigger::HeroDeath, 1, None);
    far.position = Vec3::new(0.0, 0.0, 900.0);
    vfx.trigger(&far, &view, &anchors);
    vfx.update(1.0 / 30.0, 0.0, &view, &anchors, NO_SOCKET);
    let decals = vfx.decals();
    assert_eq!(
        decals[0].role,
        DecalRole::EnemyTelegraph,
        "enemy telegraphs first"
    );
    assert_eq!(decals[0].shape, DecalShape::Ring);
    assert_eq!(decals[0].style, DecalStyle::Glow);
    assert!(
        (decals[0].position.z - 900.0).abs() < 1e-3,
        "never distance-culled"
    );
    assert!(
        (decals[0].radius - (3.0 - 0.3)).abs() < 1e-4,
        "scaled, band centred"
    );
    assert!(
        (decals[0].fade - 1.0).abs() < 1e-6,
        "full strength until its last 15 %"
    );
    assert!(decals[0].progress.is_some_and(|progress| progress < 0.05));
    let ally = decals
        .iter()
        .find(|decal| decal.role == DecalRole::Ally)
        .expect("the ally telegraph");
    assert!(
        (ally.color.w - 0.15).abs() < 1e-5,
        "dimmed by the ally opacity"
    );
    let scorch = decals
        .iter()
        .find(|decal| decal.shape == DecalShape::Splat)
        .expect("the stamped scorch");
    assert_eq!(scorch.style, DecalStyle::Paint);
    assert_eq!(scorch.role, DecalRole::Cosmetic);
    // The attack's instance ends after 0.3 s; its stamp stays until 5 s, then expires.
    let splats = |vfx: &VfxSystem| {
        vfx.decals()
            .iter()
            .filter(|decal| decal.shape == DecalShape::Splat)
            .count()
    };
    for frame in 1..=60 {
        vfx.update(1.0 / 30.0, frame as f32, &view, &anchors, NO_SOCKET);
    }
    assert_eq!(vfx.counters().instances, 0, "every effect ended");
    assert_eq!(splats(&vfx), 1, "the stamp outlives its effect");
    for frame in 61..=180 {
        vfx.update(1.0 / 30.0, frame as f32, &view, &anchors, NO_SOCKET);
    }
    assert_eq!(splats(&vfx), 0, "and expires");
    // The ring is bounded: 200 stamps keep the newest MAX_STAMPS.
    for _ in 0..200 {
        vfx.trigger(&event(Trigger::Attack, 1, Some(3)), &view, &anchors);
        vfx.update(1.0 / 30.0, 200.0, &view, &anchors, NO_SOCKET);
    }
    assert_eq!(splats(&vfx), super::MAX_STAMPS);
    let _ = std::fs::remove_dir_all(root);
}

/// A stamp whose delay is the effect's longest part (a scorch left after a short flash)
/// still fires: the effect lives past the delay by `STAMP_LATCH_SECONDS`, longer than the
/// largest frame step, so it is not retired on the frame its delay passes. At 10 fps and
/// 30 fps no frame lands exactly on the delay.
#[test]
fn late_stamps_fire_before_their_effect_ends() {
    use moba_render::DecalShape;
    for (step, tag) in [(0.1, "late10"), (1.0 / 30.0, "late30")] {
        let root = root(tag);
        std::fs::write(
            root.join("vfx/late.toml"),
            "duration = 0.1\n[[emitters]]\nburst = 1\nlifetime = [0.2, 0.2]\nsize = [1, 1]\n\
             color = [1, 1, 1, 1]\n[[decals]]\nshape = \"splat\"\ndelay = 0.45\nlifetime = 4\n\
             stamp = true\ncolor = [0.1, 0.1, 0.1, 1]\n",
        )
        .expect("write");
        let library = VfxLibrary::preview(root.clone(), "late");
        let effect = library.preview_effect().expect("the late effect loads");
        let mut vfx = VfxSystem::new(library, QualityTier::High.vfx_budget(), true);
        let view = view(Some(0));
        let anchors = anchors();
        let mut late = event(Trigger::Cast, 0, Some(1));
        late.effect = Some(effect);
        vfx.trigger(&late, &view, &anchors);
        for frame in 1..=20 {
            vfx.update(step, frame as f32, &view, &anchors, NO_SOCKET);
        }
        assert_eq!(vfx.counters().instances, 0, "the effect ended");
        assert_eq!(
            vfx.decals()
                .iter()
                .filter(|decal| decal.shape == DecalShape::Splat)
                .count(),
            1,
            "its late stamp fired ({step} s frames)"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

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

#[test]
fn effect_lights_put_enemies_first_and_dim_allies() {
    let (mut vfx, root) = system("lights");
    let view = view(Some(0));
    let anchors = anchors();
    // Unit 1 is the viewer's own hero, unit 3 an ally, unit 2 an enemy.
    let mut own = event(Trigger::Cast, 0, Some(1));
    own.ability = Some(0);
    let mut ally = event(Trigger::Cast, 0, Some(3));
    ally.ability = Some(0);
    let mut enemy = event(Trigger::Cast, 1, Some(2));
    enemy.ability = Some(0);
    vfx.set_ally_opacity(0.5);
    vfx.trigger(&own, &view, &anchors);
    vfx.trigger(&ally, &view, &anchors);
    vfx.trigger(&enemy, &view, &anchors);
    vfx.update(1.0 / 30.0, 0.0, &view, &anchors, NO_SOCKET);
    let lights = vfx.lights();
    assert_eq!(lights.len(), 3);
    // Enemy first (at its source, unit 2, one metre above the anchor), then the viewer's own
    // at full strength (own effects are never dimmed, as for particles) and the ally at half.
    assert!((lights[0].position.x - 4.0).abs() < 1e-4);
    assert!((lights[0].position.y - 2.0).abs() < 1e-4);
    assert!((lights[0].color.x - 4.0).abs() < 1e-4);
    assert!((lights[1].position.x + 4.0).abs() < 1e-4);
    assert!((lights[1].color.x - 4.0).abs() < 1e-4);
    assert!(lights[2].position.x.abs() < 1e-4);
    assert!((lights[2].color.x - 2.0).abs() < 1e-4);
    assert!((lights[0].radius - 6.0).abs() < 1e-6);
    // The light fades out with the effect (0.2 s + 0.1 s fade), then is gone.
    for tick in 1..12 {
        vfx.update(1.0 / 30.0, tick as f32, &view, &anchors, NO_SOCKET);
    }
    assert_eq!(vfx.lights(), []);
    // Allied lights at zero opacity are dropped; enemy and own lights stay.
    vfx.set_ally_opacity(0.0);
    vfx.trigger(&own, &view, &anchors);
    vfx.trigger(&ally, &view, &anchors);
    vfx.trigger(&enemy, &view, &anchors);
    vfx.update(1.0 / 30.0, 20.0, &view, &anchors, NO_SOCKET);
    assert_eq!(vfx.lights().len(), 2);
    let _ = std::fs::remove_dir_all(root);
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
        let projects_decals = library
            .effect(effect)
            .is_some_and(|loaded| !loaded.effect.decals.is_empty());
        let mut vfx = VfxSystem::new(library, QualityTier::High.vfx_budget(), true);
        let mut decals = 0;
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
            decals += vfx.decals().len();
        }
        assert!(total > 0, "{name}: no particles ({:?})", vfx.counters());
        // R3b: an effect with `[[decals]]` projects them (telegraph rings, scorch, splats).
        assert_eq!(decals > 0, projects_decals, "{name}: decals");
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

const HAZE: &str = r#"
duration = 1.0
[[emitters]]
burst = 4
lifetime = [1, 1]
size = [2, 2]
blend = "distortion"
distortion = 0.6
distortion_end = 0.2
distortion_scale = 3
distortion_speed = 1.5
fallback_color = [1, 0.4, 0.1, 0.2]
[[emitters]]
burst = 3
lifetime = [1, 1]
size = [2, 2]
blend = "distortion"
distortion = 0.5
"#;

/// ADR-0019: a heat-haze emitter gives distortion particles carrying (strength, noise scale,
/// rise speed) where the tier has distortion, its faint additive fallback where not (or
/// nothing without one); a map's `[[effects]]` loop at their positions and the preview
/// plays none.
#[test]
fn heat_haze_spawns_distortion_or_its_fallback_and_map_effects_loop() {
    let root = root("haze");
    std::fs::write(root.join("vfx/haze.toml"), HAZE).expect("write");
    let view = view(Some(0));
    let anchors = anchors();
    for (tier, distortion, fallback) in [
        (QualityTier::High, 7, 0),
        (QualityTier::Ultra, 7, 0),
        (QualityTier::Medium, 0, 4),
        (QualityTier::Low, 0, 4),
    ] {
        let library = VfxLibrary::preview(root.clone(), "haze");
        let effect = library.preview_effect().expect("the haze loads");
        let mut vfx = VfxSystem::new(library, tier.vfx_budget(), true);
        let mut haze = event(Trigger::Zone, 1, None);
        haze.effect = Some(effect);
        vfx.trigger(&haze, &view, &anchors);
        vfx.update(1.0 / 30.0, 1.0, &view, &anchors, NO_SOCKET);
        let spawns = vfx.frame().spawns;
        let hazes: Vec<_> = spawns
            .iter()
            .filter(|spawn| spawn.blend == ParticleBlend::Distortion)
            .collect();
        assert_eq!(hazes.len(), distortion, "{tier}");
        assert_eq!(spawns.len(), distortion + fallback, "{tier}");
        if let Some(first) = hazes.first() {
            assert!((first.color.x - 0.6).abs() < 1e-5);
            assert!((first.color.y - 3.0).abs() < 1e-5);
            assert!((first.color.z - 1.5).abs() < 1e-5);
            assert!((first.color_end.x - 0.2).abs() < 1e-5);
            // Never a telegraph: an enemy's haze is a plain particle.
            assert!(
                hazes
                    .iter()
                    .all(|spawn| spawn.role != ParticleRole::EnemyTelegraph)
            );
        } else {
            assert!(
                spawns
                    .iter()
                    .all(|spawn| spawn.blend == ParticleBlend::Additive)
            );
            assert!((spawns[0].color.x - 1.0).abs() < 1e-5);
            assert!((spawns[0].color.w - 0.2).abs() < 1e-5);
            assert_eq!(spawns[0].color_end.w, 0.0, "the fallback fades out");
        }
    }

    // Map effects: one live instance per entry, restarted as each play's emission ends.
    let mut vfx = VfxSystem::new(
        VfxLibrary::load(root.clone()),
        QualityTier::High.vfx_budget(),
        true,
    );
    let vents = [
        crate::app::map_visuals::AmbientEffect {
            effect: String::from("haze"),
            position: Vec3::new(2.0, 0.0, 0.0),
            radius: 1.0,
        },
        crate::app::map_visuals::AmbientEffect {
            effect: String::from("missing"),
            position: Vec3::ZERO,
            radius: 1.0,
        },
    ];
    vfx.sync_ambient(1, &vents);
    vfx.update(1.0 / 30.0, 1.0, &view, &anchors, NO_SOCKET);
    assert_eq!(vfx.counters().instances, 1, "the unknown effect is skipped");
    assert_eq!(vfx.frame().spawns.len(), 7);
    assert!(
        vfx.frame()
            .spawns
            .iter()
            .all(|spawn| (spawn.position - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-4)
    );
    // Same generation: nothing changes; after the 1 s emission window a second play starts.
    vfx.sync_ambient(1, &[]);
    let mut restarted = 0;
    for frame in 0..45 {
        vfx.update(1.0 / 30.0, 2.0 + frame as f32, &view, &anchors, NO_SOCKET);
        restarted += vfx.frame().spawns.len();
    }
    assert_eq!(restarted, 7, "one more burst after the first play's window");
    // A cleared system (content reload) resolves the map's effects again.
    vfx.clear();
    vfx.sync_ambient(1, &vents);
    vfx.update(1.0 / 30.0, 50.0, &view, &anchors, NO_SOCKET);
    assert_eq!(vfx.frame().spawns.len(), 7);
    // A map without effects stops them starting.
    vfx.sync_ambient(2, &[]);
    let mut later = 0;
    for frame in 0..90 {
        vfx.update(1.0 / 30.0, 60.0 + frame as f32, &view, &anchors, NO_SOCKET);
        later += vfx.frame().spawns.len();
    }
    assert_eq!(later, 0);
    // The authoring preview plays no map effects.
    let mut preview = VfxSystem::new(
        VfxLibrary::preview(root.clone(), "haze"),
        QualityTier::High.vfx_budget(),
        true,
    );
    preview.sync_ambient(1, &vents);
    preview.update(1.0 / 30.0, 1.0, &view, &anchors, NO_SOCKET);
    assert_eq!(preview.counters().instances, 0);
    let _ = std::fs::remove_dir_all(root);
}

const FLOOD: &str = r#"
duration = 0.0
importance = "low"
[[emitters]]
burst = 1
lifetime = [10, 10]
size = [1, 1]
color = [1, 1, 1, 0.5]
"#;

/// ADR-0019 review: looping map effects can never starve gameplay events. Sixteen entries of
/// a burst whose particles outlive its (zero) duration forty times over hold at most two
/// live plays each, never one of the last free slots, and an enemy telegraph still spawns;
/// an effect with a telegraph emitter or decal is never looped (it would draw as a fake,
/// full-alpha telegraph).
#[test]
fn looping_map_effects_never_starve_events_or_fake_telegraphs() {
    let root = root("flood");
    std::fs::write(root.join("vfx/flood.toml"), FLOOD).expect("write");
    let view = view(Some(0));
    let anchors = anchors();
    let mut vfx = VfxSystem::new(
        VfxLibrary::load(root.clone()),
        QualityTier::High.vfx_budget(),
        true,
    );
    let entries: Vec<_> = (0..moba_assets::MAX_MAP_EFFECTS)
        .map(|index| crate::app::map_visuals::AmbientEffect {
            effect: String::from("flood"),
            position: Vec3::new(index as f32, 0.0, 0.0),
            radius: 1.0,
        })
        .collect();
    vfx.sync_ambient(1, &entries);
    for frame in 0..300 {
        vfx.update(0.1, frame as f32, &view, &anchors, NO_SOCKET);
        let live = vfx.counters().instances as usize;
        assert!(
            live <= moba_assets::MAX_MAP_EFFECTS * super::AMBIENT_PLAYS,
            "frame {frame}: {live}"
        );
        assert!(live <= super::MAX_INSTANCES - super::EVENT_HEADROOM);
    }
    assert_eq!(
        vfx.counters().instances as usize,
        moba_assets::MAX_MAP_EFFECTS * super::AMBIENT_PLAYS,
        "every entry keeps its two plays"
    );
    // An enemy hero death (a telegraph ring and decal) still gets its instance.
    vfx.trigger(&event(Trigger::HeroDeath, 1, None), &view, &anchors);
    vfx.update(0.1, 400.0, &view, &anchors, NO_SOCKET);
    assert!(
        vfx.frame()
            .spawns
            .iter()
            .any(|spawn| spawn.role == ParticleRole::EnemyTelegraph)
    );
    assert!(
        vfx.decals()
            .iter()
            .any(|decal| decal.role == moba_render::DecalRole::EnemyTelegraph)
    );
    assert_eq!(vfx.counters().dropped_events, 0);

    // A telegraph effect is never looped by a map.
    let mut fake = VfxSystem::new(
        VfxLibrary::load(root.clone()),
        QualityTier::High.vfx_budget(),
        true,
    );
    let telegraph = [crate::app::map_visuals::AmbientEffect {
        effect: String::from("telegraph"),
        position: Vec3::ZERO,
        radius: 1.0,
    }];
    fake.sync_ambient(1, &telegraph);
    for frame in 0..10 {
        fake.update(0.1, frame as f32, &view, &anchors, NO_SOCKET);
    }
    assert_eq!(fake.counters().instances, 0);
    assert!(fake.decals().is_empty() && fake.frame().spawns.is_empty());
    let _ = std::fs::remove_dir_all(root);
}

/// Recall (protocol v42): the shipped recall effect plays at the hero for the channel, is
/// not an area telegraph, and stops when the recall ends (a cancel or the arrival).
#[test]
fn recall_plays_for_the_channel_and_stops_when_it_ends() {
    let content = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"));
    let library = VfxLibrary::load(content);
    assert!(library.defaults().recall.is_some(), "the shipped binding");
    let mut vfx = VfxSystem::new(library, QualityTier::High.vfx_budget(), true);
    let view = view(Some(1));
    let anchors = anchors();
    let mut start = event(Trigger::Recall, 0, Some(3));
    start.target = None;
    start.position = Vec3::ZERO;
    start.seconds = 8.0;
    vfx.queue(1, start);
    let mut spawned = 0;
    for tick in 1..=60 {
        vfx.update(1.0 / 30.0, tick as f32, &view, &anchors, NO_SOCKET);
        let frame = vfx.frame();
        spawned += frame.spawns.len();
        assert!(
            frame
                .spawns
                .iter()
                .all(|spawn| spawn.role != ParticleRole::EnemyTelegraph),
            "not a telegraph"
        );
        assert!(vfx.decals().is_empty(), "no telegraph decals");
    }
    assert!(spawned > 30, "it plays: {spawned}");
    assert_eq!(vfx.counters().instances, 1, "still channelling at 2 s");
    // Another hero's recall ending changes nothing; this hero's stops it.
    let mut other = event(Trigger::RecallEnd, 0, Some(1));
    other.target = None;
    vfx.queue(61, other);
    vfx.update(1.0 / 30.0, 61.0, &view, &anchors, NO_SOCKET);
    assert_eq!(vfx.counters().instances, 1);
    let mut end = event(Trigger::RecallEnd, 0, Some(3));
    end.target = None;
    vfx.queue(62, end);
    vfx.update(1.0 / 30.0, 62.0, &view, &anchors, NO_SOCKET);
    vfx.update(1.0 / 30.0, 63.0, &view, &anchors, NO_SOCKET);
    assert!(vfx.frame().spawns.is_empty(), "stopped emitting");
    // Gone once the last mote (1.3 s) and the light's fade (0.4 s) have finished.
    for tick in 64..=110 {
        vfx.update(1.0 / 30.0, tick as f32, &view, &anchors, NO_SOCKET);
    }
    assert_eq!(vfx.counters().instances, 0, "ended long before 8 s");
}
