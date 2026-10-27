use moba_core::{Fix, FixVec2};

use super::*;
use crate::{AreaEffect, Effect, Targeting, Team};

#[test]
fn default_content_is_valid_and_stable() {
    let content = GameContent::default();
    assert_eq!(content.items.len(), ITEMS.len());
    assert_eq!(content.validate(), Ok(()));
    assert_eq!(content.hash(), GameContent::default().hash());
}

#[test]
fn map_only_pack_overlays_defaults() {
    let text = r#"
[map]
blue_spawn = [32, 128]
red_spawn = [224, 128]

[[map.lanes]]
name = "Mid"
path = [[32, 128], [128, 128], [224, 128]]

[[map.structures]]
team = "blue"
kind = "nexus"
position = [20, 128]

[[map.structures]]
team = "red"
kind = "nexus"
position = [236, 128]

[[map.walls]]
a = [80, 96]
b = [80, 160]
"#;
    let content = load_toml(text).expect("map pack");
    assert_eq!(content.items.len(), ITEMS.len());
    assert_eq!(content.map.lanes.len(), 1);
    assert_eq!(content.map.walls.len(), 1);
    assert_eq!(content.map.nexus_position(Team::Blue).x.to_i64(), 20);
}

#[test]
fn example_mode_packs_validate() {
    for text in [
        include_str!("../../../../content/1v1-mid.toml"),
        include_str!("../../../../content/aram.toml"),
    ] {
        let content = load_toml(text).expect("parse mode pack");
        content.validate().expect("valid mode pack");
    }
}

#[test]
fn map_theme_parses_and_rejects_unknown_names() {
    let text = include_str!("../../../../content/aram.toml");
    let content = load_toml(text).expect("theme pack");
    assert_eq!(content.map.theme, MapTheme::Snow);
    let bad = text.replace("theme = \"snow\"", "theme = \"lava\"");
    assert!(load_toml(&bad).is_err());
}

#[test]
fn attestation_fields_parse_without_changing_the_hash() {
    let plain = "name = \"Pack\"\nversion = \"1\"\nauthor = \"a\"\n";
    let signed = format!(
        "{plain}publisher = \"{}\"\nsignature = \"{}\"\n",
        "11".repeat(32),
        "22".repeat(64)
    );
    let unsigned = load_toml(plain).expect("plain");
    let signed = load_toml(&signed).expect("signed");
    let attestation = signed.attestation.as_ref().expect("attestation");
    assert_eq!(attestation.publisher, [0x11; 32]);
    assert_eq!(attestation.signature, [0x22; 64]);
    assert_eq!(signed.hash(), unsigned.hash());
}

#[test]
fn map_extent_parses_and_scales_positions() {
    let text = r#"
[rules]
team_size = 1
bot_fill = false
hero_respawn_ticks = 120
respawn_per_level = 10
wave_interval_ticks = 600
first_wave_ticks = 60

[map]
extent = 128
blue_spawn = [10, 10]
red_spawn = [118, 118]

[[map.lanes]]
name = "Mid"
path = [[10, 10], [64, 64], [118, 118]]

[[map.structures]]
team = "blue"
kind = "nexus"
position = [10, 10]

[[map.structures]]
team = "red"
kind = "nexus"
position = [118, 118]
"#;
    let content = load_toml(text).expect("parse");
    assert_eq!(content.map.extent, Fix::from_i32(128));
    assert_eq!(content.rules.team_size, 1);
    assert!(!content.rules.bot_fill);
    assert_eq!(content.rules.hero_respawn_ticks, 120);
    assert_eq!(content.rules.respawn_per_level, 10);
    assert_eq!(content.rules.wave_interval_ticks, 600);
    assert_eq!(content.rules.first_wave_ticks, 60);
    content.validate().expect("valid 128 map");
}

#[test]
fn example_pack_parses() {
    let content = load_toml(include_str!("../../../../content/example-pack.toml"))
        .expect("example pack is valid");
    assert_eq!(content.items.len(), 8);
    assert_eq!(content.heroes.len(), 1);
    assert_eq!(content.heroes[0].name, "Vanguard");
    assert_eq!(content.manifest.name, "Example Pack");
    assert_eq!(content.manifest.version, "1.0.0");
}

#[test]
fn ability_pack_parses_and_heroes_reference_it() {
    let text = r#"
[[items]]
name = "Stick"
cost = 1

[[abilities]]
name = "Zap"
cooldown_ticks = 60
mana_cost = 10
cast_range = 5.0
kind = "burst"
radius = 3.0
damage = 50
damage_type = "magic"

[[heroes]]
name = "Spark"
max_health = 500
max_mana = 300
attack_damage = 40
attack_range = 8.0
attack_interval_ticks = 30
move_speed = 6.0
vision_range = 14.0
abilities = [0, 0, 0]
"#;
    let content = load_toml(text).expect("ability pack");
    assert_eq!(content.abilities.len(), 1);
    assert_eq!(content.abilities[0].name, "Zap");
    assert!(matches!(
        content.abilities[0].targeting,
        Targeting::Point { .. }
    ));
    assert!(content.abilities[0].effects.iter().any(|effect| matches!(
        effect,
        Effect::Area { effects, .. }
            if effects.iter().any(|effect| matches!(
                effect,
                AreaEffect::Damage { amount: 50, .. }
            ))
    )));
}

#[test]
fn manifest_validation_rejects_empty_fields() {
    let mut content = GameContent::default();
    content.manifest.author.clear();
    assert_eq!(content.validate(), Err(ContentError::BadManifest));
}

#[test]
fn hero_validation_rejects_bad_ability_ids() {
    let mut content = GameContent::default();
    content.heroes[0].abilities = [255, 0, 0];
    assert_eq!(content.validate(), Err(ContentError::BadAbility));
    let mut content = GameContent::default();
    content.heroes[0].max_health = 0;
    assert_eq!(content.validate(), Err(ContentError::BadHeroStats));
}

#[test]
fn strict_schema_rejects_unknown_fields_and_enums() {
    assert!(matches!(
        load_toml("bogus = 1"),
        Err(ContentLoadError::Toml(_))
    ));
    assert!(matches!(
        load_toml("[[items]]\nname = \"X\"\ncost = 1\nextra = 2\n"),
        Err(ContentLoadError::Toml(_))
    ));
    let bad_kind = r#"
[[abilities]]
name = "Bad"
cooldown_ticks = 1
kind = "teleport"
"#;
    assert!(matches!(
        load_toml(bad_kind),
        Err(ContentLoadError::InvalidField {
            field: "abilities.kind",
            ..
        })
    ));
    let bad_damage = r#"
[[abilities]]
name = "Bad"
cooldown_ticks = 1
kind = "burst"
radius = 1.0
damage = 1
damage_type = "holy"
"#;
    assert!(matches!(
        load_toml(bad_damage),
        Err(ContentLoadError::InvalidField {
            field: "abilities.damage_type",
            ..
        })
    ));
    let bad_team = r#"
[map]
blue_spawn = [32, 128]
red_spawn = [224, 128]
[[map.lanes]]
name = "Mid"
path = [[32, 128], [224, 128]]
[[map.structures]]
team = "green"
kind = "nexus"
position = [20, 128]
"#;
    assert!(matches!(
        load_toml(bad_team),
        Err(ContentLoadError::InvalidField {
            field: "map.structures.team",
            ..
        })
    ));
    let bad_structure = bad_team.replace("green", "blue").replace("nexus", "spire");
    assert!(matches!(
        load_toml(&bad_structure),
        Err(ContentLoadError::InvalidField {
            field: "map.structures.kind",
            ..
        })
    ));
    let non_finite = r#"
[[items]]
name = "X"
cost = 1
move_speed = nan
"#;
    assert!(matches!(
        load_toml(non_finite),
        Err(ContentLoadError::InvalidField {
            field: "items.move_speed",
            ..
        })
    ));
}

#[test]
fn map_themes_have_unique_asset_names() {
    let mut names = [""; 5];
    for (index, theme) in [
        MapTheme::Grass,
        MapTheme::Stone,
        MapTheme::Snow,
        MapTheme::Desert,
        MapTheme::Arcane,
    ]
    .into_iter()
    .enumerate()
    {
        names[index] = theme.asset_name();
    }
    names.sort_unstable();
    assert!(names.windows(2).all(|pair| pair[0] != pair[1]));
}

#[test]
fn stun_modifier_kind_parses() {
    assert_eq!(
        super::load::parse_modifier_kind("stun").expect("stun kind"),
        ModifierKind::Stun
    );
}

#[test]
fn modifier_definitions_are_validated_and_hashed() {
    let mut content = GameContent::default();
    let baseline = content.hash();
    content.modifiers[0].name = String::from("Barrier");
    assert_ne!(content.hash(), baseline);
    assert_eq!(content.validate(), Ok(()));

    let mut content = GameContent::default();
    content
        .modifiers
        .retain(|modifier| modifier.kind != ModifierKind::Burn);
    assert_eq!(content.validate(), Ok(()));

    let unknown = r#"
[[modifiers]]
name = "Odd"
kind = "charm"
"#;
    assert!(matches!(
        load_toml(unknown),
        Err(ContentLoadError::InvalidField {
            field: "modifiers.kind",
            ..
        })
    ));
}

#[test]
fn validation_rejects_out_of_range_stats_and_map_coordinates() {
    let mut content = GameContent::default();
    content.items[0].health = MAX_ITEM_STAT + 1;
    assert_eq!(content.validate(), Err(ContentError::BadItemStats));
    let mut content = GameContent::default();
    content.heroes[0].max_health = MAX_STAT + 1;
    assert_eq!(content.validate(), Err(ContentError::BadHeroStats));
    let mut content = GameContent::default();
    content.map.blue_spawn = FixVec2::from_ints(-1, 128);
    assert_eq!(content.validate(), Err(ContentError::BadMapData));
    let mut content = GameContent::default();
    content.abilities[0].cooldown_ticks = MAX_COOLDOWN_TICKS + 1;
    assert_eq!(content.validate(), Err(ContentError::BadAbilityStats));
}

#[test]
fn map_wire_conversions_are_exact_or_none() {
    let point = |x: Fix, y: Fix| MapSpec::wire_point(FixVec2::new(x, y));
    assert_eq!(
        point(Fix::from_i32(-32_768), Fix::from_i32(32_767)),
        Some([i16::MIN, i16::MAX])
    );
    assert_eq!(point(Fix::from_i32(32_768), Fix::ZERO), None);
    assert_eq!(point(Fix::ZERO, Fix::from_i32(-32_769)), None);
    assert_eq!(point(Fix::from_f64(-0.5), Fix::ZERO), None);
    assert_eq!(point(Fix::ZERO, Fix::from_bits(1)), None);
    assert_eq!(MapSpec::wire_extent(Fix::from_i32(256)), Some(256));
    assert_eq!(MapSpec::wire_extent(Fix::from_i32(65_535)), Some(u16::MAX));
    assert_eq!(MapSpec::wire_extent(Fix::from_i32(65_536)), None);
    assert_eq!(MapSpec::wire_extent(Fix::from_i32(-1)), None);
    assert_eq!(MapSpec::wire_extent(Fix::from_f64(128.5)), None);

    // #296 repro: a fractional extent used to pass validation and truncate on the wire.
    let mut content = GameContent::default();
    content.map.extent = Fix::from_f64(128.5);
    assert_eq!(
        content.validate(),
        Err(ContentError::BadMapCoordinate("map.extent"))
    );
    assert!(
        ContentError::BadMapCoordinate("map.extent")
            .to_string()
            .contains("map.extent")
    );
    let mut content = GameContent::default();
    content.map.structures[0].position.x += Fix::from_f64(0.25);
    assert_eq!(
        content.validate(),
        Err(ContentError::BadMapCoordinate("map.structures.position"))
    );
}

#[test]
fn validation_rejects_bad_names_and_duplicates() {
    let mut content = GameContent::default();
    content.items[0].name.clear();
    assert_eq!(content.validate(), Err(ContentError::BadName));
    let mut content = GameContent::default();
    content.items[1].name = content.items[0].name.clone();
    assert_eq!(content.validate(), Err(ContentError::DuplicateName));
    let mut content = GameContent::default();
    content.items[0].cost = 0;
    assert_eq!(content.validate(), Err(ContentError::FreeItem));
}

#[test]
fn rules_vision_is_hashed_and_derives_legacy_fields() {
    let text = "[rules.melee_minion]\nvision_range = 12.0\n";
    let content = load_toml(text).expect("rules");
    assert_eq!(
        content.rules.melee_vision,
        content.rules.melee_minion.vision_range
    );
    let mut changed = content.clone();
    changed.rules.melee_minion.vision_range = Fix::from_i32(11);
    assert_ne!(changed.hash(), content.hash());
    let mut mismatched = content;
    mismatched.rules.melee_vision = Fix::from_i32(1);
    assert_eq!(
        mismatched.validate(),
        Err(ContentError::BadRules("melee_vision"))
    );
}

#[test]
fn partial_ranged_and_nexus_tables_keep_their_own_defaults() {
    let ranged = load_toml("[rules.ranged_minion]\nmax_health = 999\n").expect("ranged");
    assert_eq!(ranged.rules.ranged_minion.max_health, 999);
    assert_eq!(ranged.rules.ranged_minion.attack_damage, 20);
    assert_eq!(ranged.rules.ranged_minion.attack_range, Fix::from_i32(7));
    let nexus = load_toml("[rules.nexus]\nmax_health = 6000\n").expect("nexus");
    assert_eq!(nexus.rules.nexus.max_health, 6000);
    assert_eq!(nexus.rules.nexus.attack_damage, 85);
    assert_eq!(nexus.rules.nexus.attack_range, Fix::from_i32(9));
}

#[test]
fn invalid_rules_name_the_field_and_reject_nonfinite_floats() {
    let negative = load_toml("[rules.melee_minion]\nattack_damage = -1\n");
    assert!(matches!(
        negative,
        Err(ContentLoadError::Invalid(ContentError::BadRules(
            "melee_minion"
        )))
    ));
    let nan = load_toml("[rules.melee_minion]\nvision_range = nan\n");
    assert!(matches!(
        nan,
        Err(ContentLoadError::InvalidField {
            field: "rules.melee_minion",
            ..
        })
    ));
}
