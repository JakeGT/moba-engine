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

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// #285: the visuals block is appended to the canonical feed only when a pack names a
/// visual, so every pack, replay and signature from before it keeps its exact bytes. The
/// values were recorded on `main` before the change (`ef752ab`).
#[test]
fn packs_without_visuals_keep_their_pre_285_hash_and_signing_digest() {
    use sha2::{Digest, Sha256};
    let pinned: [(&str, Option<&str>, u64, &str); 8] = [
        (
            "default",
            None,
            0x20bc_152b_e166_1fb2,
            "5c9b80c04e805a81a510dbceee5b98872a8f187d4eb301309b09bbf2d9f81319",
        ),
        (
            "1v1-mid",
            Some(include_str!("../../../../content/1v1-mid.toml")),
            0x0769_5381_662f_7867,
            "20cce1791f670e6857ee2d013ed45f25082299e23123a5d54580f1d441e3ca06",
        ),
        (
            "ability-pack",
            Some(include_str!("../../../../content/ability-pack.toml")),
            0xd31c_03ec_6711_5992,
            "c6579e0618e83c8a1a9918735cd4537f621b57da5824bff75f981d70838ddc40",
        ),
        (
            "aram",
            Some(include_str!("../../../../content/aram.toml")),
            0x6fd1_af85_3a8e_422f,
            "8bf6fd519d950b297f39b85f3f1f4cf277debe852c57fac24f9d8abef18a73bb",
        ),
        (
            "economy-override",
            Some(include_str!("../../../../content/economy-override.toml")),
            0x7d9f_7f5a_f8b7_9b03,
            "cb8de05c44d64808b6a38f86a1c4e0e8e158a8f5bbfaa13f4b14f4a54e4f4ff6",
        ),
        (
            "example-pack",
            Some(include_str!("../../../../content/example-pack.toml")),
            0xbe07_7350_aa23_e24a,
            "6cc9c7d0a531cc15fa8c13916d8a21328fd40a0b39dd621cf617297fcdd1ad00",
        ),
        (
            "items",
            Some(include_str!("../../../../content/items.toml")),
            0xe6df_97b8_4744_60e2,
            "a100bcde577ed6f68452d5d5ff996ad473f76bf6dbfc1661bd5cd1f01df5708e",
        ),
        (
            "map-pack",
            Some(include_str!("../../../../content/map-pack.toml")),
            0x62f0_6a94_5938_a96c,
            "04552fe2d22b8c4e18ebd5ce98d01194dacd639e3f5d51315e4f33b47f1bcd94",
        ),
    ];
    for (name, text, hash, digest) in pinned {
        let content = text.map_or_else(GameContent::default, |text| {
            load_toml(text).expect("shipped pack loads")
        });
        assert_eq!(content.hash(), hash, "{name}: content hash changed");
        assert_eq!(
            hex(&Sha256::digest(content.signing_message())),
            digest,
            "{name}: signing message changed"
        );
    }
}

const ASSET_PACK: &str = r#"
[[assets.units]]
name = "knight"
path = "models/knight.glb"
fallback = "sentinel"

[[assets.units]]
name = "sentinel"
path = "models/sentinel.glb"

[[assets.maps]]
name = "lava"
path = "maps/lava.glb"

[map]
theme = "stone"
visuals = "lava"
blue_spawn = [32, 128]
red_spawn = [224, 128]

[[map.lanes]]
name = "Mid"
path = [[32, 128], [224, 128]]

[[heroes]]
name = "Knight"
max_health = 600
max_mana = 300
attack_damage = 50
attack_range = 2.0
attack_interval_ticks = 30
move_speed = 7.0
vision_range = 14.0
abilities = [0, 1, 2]
model = "knight"

[[heroes]]
name = "Plain"
max_health = 600
max_mana = 300
attack_damage = 50
attack_range = 2.0
attack_interval_ticks = 30
move_speed = 7.0
vision_range = 14.0
abilities = [0, 1, 2]
"#;

#[test]
fn asset_references_load_and_are_hashed() {
    let content = load_toml(ASSET_PACK).expect("asset pack");
    assert_eq!(content.heroes[0].model.as_deref(), Some("knight"));
    assert_eq!(content.heroes[1].model, None);
    assert_eq!(content.map.visuals.as_deref(), Some("lava"));
    assert_eq!(content.map.visuals_name(), "lava");
    assert_eq!(content.assets.len(), 3);
    assert_eq!(
        content.assets.resolve("knight").map(|e| e.name.as_str()),
        Some("sentinel")
    );

    // Each visual name is part of the hash and the signed feed.
    let mut no_model = content.clone();
    no_model.heroes[0].model = None;
    let mut other_model = content.clone();
    other_model.heroes[0].model = Some(String::from("sentinel"));
    let mut no_visuals = content.clone();
    no_visuals.map.visuals = None;
    let mut moved_model = content.clone();
    moved_model.heroes[0].model = None;
    moved_model.heroes[1].model = Some(String::from("knight"));
    let hashes = [
        content.hash(),
        no_model.hash(),
        other_model.hash(),
        no_visuals.hash(),
        moved_model.hash(),
    ];
    for (index, hash) in hashes.iter().enumerate() {
        assert!(
            !hashes[..index].contains(hash),
            "variant {index} collides: {hashes:x?}"
        );
    }
    assert_ne!(content.signing_message(), no_model.signing_message());

    // The manifest itself (paths) is pack-local: not hashed, not signed.
    let mut moved_files = content.clone();
    moved_files.assets.units[1].path = String::from("elsewhere/sentinel.glb");
    assert_eq!(moved_files.hash(), content.hash());
    assert_eq!(moved_files.signing_message(), content.signing_message());

    // No names at all: the block is omitted, so the hash equals the pre-#285 feed.
    let mut bare = no_model.clone();
    bare.map.visuals = None;
    bare.assets = AssetManifest::default();
    let mut without_names = ASSET_PACK.replace("model = \"knight\"\n", "");
    without_names = without_names.replace("visuals = \"lava\"\n", "");
    let reloaded = load_toml(&without_names).expect("pack without names");
    assert_eq!(reloaded.hash(), bare.hash());
}

#[test]
fn map_visuals_default_to_the_theme_and_are_canonical() {
    let plain = load_toml(&ASSET_PACK.replace("visuals = \"lava\"\n", "")).expect("no visuals");
    assert_eq!(plain.map.visuals, None);
    assert_eq!(plain.map.visuals_name(), "map-stone");
    // Naming the theme default explicitly is the same content (and hash) as omitting it, but
    // the explicit name still has to be declared in the manifest.
    let explicit = ASSET_PACK.replace("visuals = \"lava\"", "visuals = \"map-stone\"");
    assert!(matches!(
        load_toml(&explicit),
        Err(ContentLoadError::UnknownAsset {
            field: "map.visuals",
            ..
        })
    ));
    let declared = explicit.replace("name = \"lava\"", "name = \"map-stone\"");
    let declared = load_toml(&declared).expect("declared theme visuals");
    assert_eq!(declared.map.visuals, None);
    assert_eq!(declared.hash(), plain.hash());
}

#[test]
#[allow(clippy::too_many_lines)] // One table per invalid-reference case.
fn invalid_asset_references_are_rejected() {
    let unknown_model = ASSET_PACK.replace("model = \"knight\"", "model = \"ghost\"");
    assert!(matches!(
        load_toml(&unknown_model),
        Err(ContentLoadError::UnknownAsset { field: "heroes.model", ref name }) if name == "ghost"
    ));
    // A map name is not a unit name (separate lists), and vice versa.
    let wrong_list = ASSET_PACK.replace("model = \"knight\"", "model = \"lava\"");
    assert!(matches!(
        load_toml(&wrong_list),
        Err(ContentLoadError::UnknownAsset {
            field: "heroes.model",
            ..
        })
    ));
    let unknown_visuals = ASSET_PACK.replace("visuals = \"lava\"", "visuals = \"knight\"");
    assert!(matches!(
        load_toml(&unknown_visuals),
        Err(ContentLoadError::UnknownAsset {
            field: "map.visuals",
            ..
        })
    ));
    // No `[assets]` table at all: every name is unknown.
    let start = ASSET_PACK.find("[map]").expect("map table");
    assert!(matches!(
        load_toml(&ASSET_PACK[start..]),
        Err(ContentLoadError::UnknownAsset { .. })
    ));
    for (bad, field) in [
        ("model = \"../knight\"", "heroes.model"),
        ("model = \"models/knight.glb\"", "heroes.model"),
        ("model = \"c:knight\"", "heroes.model"),
        ("model = \"\"", "heroes.model"),
        ("model = \"seventeen-chars-x\"", "heroes.model"),
        ("visuals = \"lava.glb\"", "map.visuals"),
    ] {
        let original = if field == "map.visuals" {
            "visuals = \"lava\""
        } else {
            "model = \"knight\""
        };
        let text = ASSET_PACK.replace(original, bad);
        assert!(
            matches!(
                load_toml(&text),
                Err(ContentLoadError::Invalid(ContentError::BadVisualName(f))) if f == field
            ),
            "{bad}: {:?}",
            load_toml(&text).map(|_| ())
        );
    }
    // Manifest errors: path escapes, backslashes, drive letters, fallback cycles, unknown
    // fallbacks, duplicates and unknown fields.
    for (from, to, expected) in [
        (
            "path = \"models/knight.glb\"",
            "path = \"../knight.glb\"",
            AssetError::BadPath,
        ),
        (
            "path = \"models/knight.glb\"",
            "path = \"models\\\\knight.glb\"",
            AssetError::BadPath,
        ),
        (
            "path = \"models/knight.glb\"",
            "path = \"C:/knight.glb\"",
            AssetError::BadPath,
        ),
        (
            "path = \"models/knight.glb\"",
            "path = \"/abs/knight.glb\"",
            AssetError::BadPath,
        ),
        (
            "path = \"models/sentinel.glb\"",
            "path = \"models/sentinel.glb\"\nfallback = \"knight\"",
            AssetError::BadFallback,
        ),
        (
            "fallback = \"sentinel\"",
            "fallback = \"missing\"",
            AssetError::BadFallback,
        ),
        (
            "name = \"sentinel\"",
            "name = \"knight\"",
            AssetError::Duplicate,
        ),
    ] {
        let text = ASSET_PACK.replace(from, to);
        assert!(
            matches!(load_toml(&text), Err(ContentLoadError::Assets(error)) if error == expected),
            "{to}: {:?}",
            load_toml(&text).map(|_| ())
        );
    }
    let unknown_field = ASSET_PACK.replace(
        "path = \"maps/lava.glb\"",
        "path = \"maps/lava.glb\"\nsize = 3",
    );
    assert!(matches!(
        load_toml(&unknown_field),
        Err(ContentLoadError::Toml(_))
    ));
    let unknown_list = format!("{ASSET_PACK}\n[[assets.sounds]]\nname = \"x\"\npath = \"x.glb\"\n");
    assert!(matches!(
        load_toml(&unknown_list),
        Err(ContentLoadError::Toml(_))
    ));
}

#[test]
fn sentinel_pack_names_a_shipped_model() {
    let content =
        load_toml(include_str!("../../../../content/sentinel-pack.toml")).expect("sentinel pack");
    let defaults = GameContent::default();
    assert_eq!(content.heroes.len(), defaults.heroes.len());
    for (hero, default) in content.heroes.iter().zip(&defaults.heroes) {
        let mut stats = hero.clone();
        stats.model = None;
        assert_eq!(&stats, default, "{}: stats match the defaults", hero.name);
    }
    let modelled: Vec<_> = content
        .heroes
        .iter()
        .filter_map(|hero| Some((hero.name.as_str(), hero.model.as_deref()?)))
        .collect();
    assert_eq!(modelled, [("Guardian", "sentinel")]);
    assert_eq!(
        content
            .assets
            .unit("sentinel")
            .map(|entry| entry.path.as_str()),
        Some("models/sentinel.glb")
    );
    assert_ne!(content.hash(), defaults.hash());
}

#[test]
fn recall_ticks_defaults_parses_validates_and_is_hashed() {
    let default = GameContent::default();
    assert_eq!(default.rules.recall_ticks, 240, "8 s at 30 Hz");
    let content = load_toml("[rules]\nrecall_ticks = 90\n").expect("parse");
    assert_eq!(content.rules.recall_ticks, 90);
    assert_eq!(content.validate(), Ok(()));
    assert_ne!(content.hash(), default.hash(), "recall_ticks is hashed");
    let disabled = load_toml("[rules]\nrecall_ticks = 0\n").expect("parse");
    assert_eq!(disabled.validate(), Ok(()), "0 disables recall");
    assert_ne!(disabled.hash(), content.hash());
    // The default, written out, is the pack that leaves it out (every pre-recall pack keeps
    // its hash and signature).
    let explicit = load_toml("[rules]\nrecall_ticks = 240\n").expect("parse");
    assert_eq!(explicit.hash(), default.hash());
    let mut too_long = GameContent::default();
    too_long.rules.recall_ticks = 901;
    assert_eq!(
        too_long.validate(),
        Err(ContentError::BadRules("recall_ticks"))
    );
    assert!(load_toml("[rules]\nrecall_ticks = 901\n").is_err());
    assert!(load_toml("[rules]\nrecall_ticks = 900\n").is_ok());
}
