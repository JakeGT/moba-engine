use super::catalog_codec::{decode_catalog, encode_catalog};
use super::*;
use moba_ecs::Entity;
use moba_game::GameContent;
use moba_game::Team;
use moba_sim::ObservedUnit;
use moba_sim::UnitKind;

fn header(kind: PacketKind, seq: u32) -> Header {
    Header {
        kind,
        flags: 0,
        session: 0xDEAD_BEEF,
        seq,
        ack: 7,
        ack_bits: 0b1010,
    }
}

#[test]
fn welcome_round_trip_carries_verified_catalogue() {
    let content = GameContent::default();
    let welcome = Welcome {
        slot: 3,
        tick_rate: 30,
        snapshot_interval: 1,
        tick: 42,
        seed: 0x1234,
        hero_pick: 2,
        nonce: 7,
        server_nonce: 9,
        auth: [0; 32],
        spectator: false,
        catalog: WireCatalog::from(&content),
        content_hash: content.hash(),
        identity: false,
        server_public_key: [0; 32],
        server_signature: [0; 64],
    };
    let encoded = encode_welcome(&welcome);
    assert!(
        encoded.len() > MAX_DATAGRAM,
        "the Welcome (catalogue included) must still exercise transport fragmentation"
    );
    let decoded = decode_welcome(&encoded).expect("welcome decodes");
    assert_eq!(decoded, welcome);
    assert_eq!(
        decoded.catalog.content_hash().expect("catalogue hash"),
        decoded.content_hash
    );
}

#[test]
fn catalogue_round_trip_preserves_content_hash() {
    let content = GameContent::default();
    let wire = WireCatalog::from(&content);
    assert_eq!(wire.content_hash().expect("catalogue hash"), content.hash());
    let mut bytes = Vec::new();
    encode_catalog(&wire, &mut bytes);
    let mut reader = Reader::new(&bytes);
    let decoded = decode_catalog(&mut reader).expect("decode catalogue");
    assert_eq!(reader.remaining(), 0);
    assert_eq!(decoded, wire);
    assert_eq!(
        decoded.to_content().expect("content").hash(),
        content.hash()
    );

    let mut changed = GameContent::default();
    changed.heroes[0].attack_damage += 1;
    assert_ne!(
        WireCatalog::from(&changed).content_hash(),
        wire.content_hash()
    );
    let mut changed = GameContent::default();
    if let moba_game::Targeting::Direction { range } = changed.abilities[0].targeting {
        changed.abilities[0].targeting = moba_game::Targeting::Direction {
            range: range + Fix::ONE,
        };
    }
    assert_ne!(
        WireCatalog::from(&changed).content_hash(),
        wire.content_hash()
    );
}

#[test]
fn oversized_snapshot_counts_are_rejected() {
    let delta = SnapshotDelta {
        tick: 1,
        acked_input: 0,
        baseline_seq: 0,
        blue_score: 0,
        red_score: 0,
        outcome: 0,
        phase: 0,
        countdown: 0,
        ready_mask: 0,
        player_mask: 0,
        spawns: Vec::new(),
        updates: Vec::new(),
        despawns: Vec::new(),
        events: Vec::new(),
    };
    let mut payload = Vec::new();
    encode_snapshot(&delta, &mut payload).expect("encode");
    let mut corrupted = payload.clone();
    corrupted[28..30].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(decode_snapshot(&corrupted), Err(WireError::BadValue));
    let mut corrupted = payload.clone();
    corrupted[30..32].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(decode_snapshot(&corrupted), Err(WireError::BadValue));
    let mut corrupted = payload;
    corrupted[32..34].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(decode_snapshot(&corrupted), Err(WireError::BadValue));
}

#[test]
fn progress_only_changes_are_delta_encoded() {
    let baseline = vec![entity(1, 10, 20, 600)];
    let mut current = baseline.clone();
    current[0].progress.gold = 350;
    current[0].progress.items[0] = 2;
    let delta = build_delta(&current, Some(&baseline), 2, 0, 1);
    assert_eq!(delta.updates.len(), 1);
    let progress = delta.updates[0].progress.expect("progress update");
    assert_eq!(progress.gold, 350);
    assert_eq!(progress.items[0], 2);
}

#[test]
fn packet_round_trip_and_bounds() {
    let mut buffer = Vec::new();
    let payload = [1u8, 2, 3, 4, 5];
    let written = encode_packet(&header(PacketKind::Input, 9), &payload, &mut buffer).unwrap();
    assert_eq!(written, HEADER_LEN + payload.len());
    let (decoded, body) = decode_packet(&buffer).unwrap();
    assert_eq!(decoded, header(PacketKind::Input, 9));
    assert_eq!(body, payload);

    assert_eq!(decode_packet(&[]), Err(WireError::Truncated));
    let mut corrupt = buffer.clone();
    corrupt[0] = 0;
    assert_eq!(decode_packet(&corrupt), Err(WireError::BadMagic));
    let long = vec![0u8; MAX_PAYLOAD + 1];
    assert_eq!(
        encode_packet(&header(PacketKind::Snapshot, 1), &long, &mut buffer),
        Err(WireError::TooLarge)
    );
}

#[test]
fn handshake_round_trip() {
    let hello = Hello {
        protocol: 1,
        nonce: 0xDEAD_BEEF_1234,
        reconnect: 0xABCD_1234,
        auth: [3; 32],
        spectate: true,
        identity: false,
        client_public: [0; 32],
        client_signature: [0; 64],
        server_challenge: 0,
    };
    assert_eq!(decode_hello(&encode_hello(&hello)), Ok(hello));
}

#[test]
fn commands_round_trip() {
    let commands = [
        WireCommand::Stop,
        WireCommand::MoveTo { x: 10, y: 20 },
        WireCommand::Attack { net_id: 99 },
        WireCommand::CastAbility {
            slot: 2,
            x: 500,
            y: 600,
        },
    ];
    for command in commands {
        let message = InputMessage {
            client_tick: 123,
            input_seq: 45,
            command,
        };
        let encoded = encode_input(&message);
        assert_eq!(decode_input(&encoded), Ok(message));
    }
}

fn entity(id: NetId, x: u16, y: u16, health: u16) -> NetEntity {
    NetEntity {
        net_id: id,
        kind: UnitKind::Hero as u8,
        team: Team::Blue.index() as u8,
        hero_slot: 0,
        hero_pick: 0,
        flags: 0,
        status_bits: 0,
        max_health: 600,
        max_mana: 300,
        x,
        y,
        health,
        mana: 100,
        cooldowns: [0, 0, 0],
        progress: NetProgress::default(),
    }
}

#[test]
fn content_update_round_trips_catalogue_and_hash() {
    let content = moba_game::GameContent::default();
    let catalog = WireCatalog::from(&content);
    let mut payload = Vec::new();
    encode_content_update(&catalog, content.hash(), &mut payload);
    let (decoded, hash) = decode_content_update(&payload).expect("decode");
    assert_eq!(hash, content.hash());
    assert_eq!(decoded, catalog);
    payload[0] ^= 0xFF;
    assert!(decode_content_update(&payload).is_err());
}

#[test]
fn wire_catalogue_round_trips_map_extent() {
    use moba_game::GameContent;
    let mut content = GameContent::default();
    content.map.extent = moba_core::Fix::from_i32(128);
    content.map.name = String::from("Test Map");
    content.map.theme = moba_game::MapTheme::Snow;
    content.map.blue_spawn = moba_core::FixVec2::from_ints(10, 10);
    content.map.red_spawn = moba_core::FixVec2::from_ints(118, 118);
    content.rules.melee_minion.max_health = 77;
    content.rules.hero_respawn_ticks = 120;
    content.rules.respawn_per_level = 10;
    content.rules.wave_interval_ticks = 600;
    content.rules.first_wave_ticks = 60;
    let wire = WireCatalog::from(&content);
    assert_eq!(wire.map.extent, 128);
    assert_eq!(wire.map.name_str(), "Test Map");
    assert_eq!(wire.map.theme, 2);
    assert_eq!(wire.rules.hero_respawn_ticks, 120);
    assert_eq!(wire.rules.melee_minion.max_health, 77);
    let decoded = wire.to_content().expect("round trip");
    assert_eq!(decoded.map.extent, content.map.extent);
    assert_eq!(decoded.map.name, "Test Map");
    assert_eq!(decoded.map.theme, moba_game::MapTheme::Snow);
    assert_eq!(decoded.rules, content.rules);
    assert_eq!(decoded.hash(), content.hash());
}

#[test]
fn wire_catalogue_round_trips_fully_nondefault_content() {
    let mut content =
        moba_game::load_content_toml(include_str!("../../../../content/ability-pack.toml"))
            .expect("ability pack");
    content.manifest.name = String::from("Parity");
    content.manifest.version = String::from("9");
    content.manifest.author = String::from("qa");
    content.rules.team_size = 3;
    content.rules.wave_interval_ticks = 777;
    content.rules.first_wave_ticks = 11;
    content.rules.melee_minion.vision_range = moba_core::Fix::from_i32(12);
    content.items[0].cost += 1;
    content.items[0].health += 1;
    content.heroes[0].vision_range = moba_core::Fix::from_i32(15);
    content.map.name = String::from("Parity Map");
    content.map.theme = moba_game::MapTheme::Arcane;
    content.map.extent = moba_core::Fix::from_i32(200);
    content.heroes[0].model = Some(String::from("knight"));
    content.map.visuals = Some(String::from("lava-rock"));
    let wire = WireCatalog::from(&content);
    let decoded = wire.to_content().expect("round trip");
    assert_eq!(decoded, content);
    assert_eq!(decoded.hash(), content.hash());
}

/// Asserts a mutated catalogue is valid, changes the hash, and crosses the wire losslessly.
fn assert_visual_parity(name: &str, content: &GameContent, base: &GameContent) {
    assert_eq!(content.validate(), Ok(()), "{name}: must stay valid");
    assert_ne!(content.hash(), base.hash(), "{name}: must change the hash");
    let wire = WireCatalog::from(content);
    assert_eq!(
        wire.content_hash().expect("wire hash"),
        content.hash(),
        "{name}: hashed hero/visual field is not carried on the wire"
    );
    let decoded = wire.to_content().expect("content");
    assert_eq!(decoded.heroes, content.heroes, "{name}: heroes differ");
    assert_eq!(decoded.map, content.map, "{name}: map differs");
    let mut bytes = Vec::new();
    encode_catalog(&wire, &mut bytes);
    let mut reader = Reader::new(&bytes);
    assert_eq!(
        decode_catalog(&mut reader).expect("decode catalogue"),
        wire,
        "{name}: encode/decode round trip"
    );
    assert_eq!(reader.remaining(), 0, "{name}: trailing bytes");
}

#[test]
#[allow(clippy::too_many_lines)] // One mutation per hashed hero field.
fn wire_hero_parity_covers_every_hashed_field() {
    use moba_game::{HeroSpec, MAX_CONTENT_HEROES, MAX_VISUAL_NAME};
    type Mutation = (&'static str, Box<dyn Fn(&mut HeroSpec)>);
    let base = GameContent::default();
    // Exhaustive destructure with no `..`: adding a `HeroSpec` field fails to compile here
    // until a mutation for it is added below (#285 added `model`).
    let HeroSpec {
        name: _,
        max_health: _,
        max_mana: _,
        health_regen_per_tick: _,
        mana_regen_per_tick: _,
        attack_damage: _,
        attack_range: _,
        attack_interval_ticks: _,
        move_speed_per_sec: _,
        armor: _,
        magic_resist: _,
        vision_range: _,
        abilities: _,
        health_per_level: _,
        mana_per_level: _,
        attack_damage_per_level: _,
        armor_per_level: _,
        magic_resist_per_level: _,
        model: _,
    } = &base.heroes[0];
    assert_eq!(
        base.heroes.iter().find_map(|hero| hero.model.as_ref()),
        None
    );
    // Each mutation is relative to the hero it is applied to, so it always changes it.
    let mutations: Vec<Mutation> = vec![
        ("name", Box::new(|hero| hero.name.push('X'))),
        ("max_health", Box::new(|hero| hero.max_health += 1)),
        ("max_mana", Box::new(|hero| hero.max_mana += 1)),
        (
            "health_regen_per_tick",
            Box::new(|hero| hero.health_regen_per_tick += 1),
        ),
        (
            "mana_regen_per_tick",
            Box::new(|hero| hero.mana_regen_per_tick += 1),
        ),
        ("attack_damage", Box::new(|hero| hero.attack_damage += 1)),
        (
            "attack_range",
            Box::new(|hero| hero.attack_range += Fix::ONE),
        ),
        (
            "attack_interval_ticks",
            Box::new(|hero| hero.attack_interval_ticks += 1),
        ),
        (
            "move_speed_per_sec",
            Box::new(|hero| hero.move_speed_per_sec += Fix::ONE),
        ),
        ("armor", Box::new(|hero| hero.armor += 1)),
        ("magic_resist", Box::new(|hero| hero.magic_resist += 1)),
        (
            "vision_range",
            Box::new(|hero| hero.vision_range += Fix::ONE),
        ),
        ("abilities", Box::new(|hero| hero.abilities.swap(0, 1))),
        (
            "health_per_level",
            Box::new(|hero| hero.health_per_level += 1),
        ),
        ("mana_per_level", Box::new(|hero| hero.mana_per_level += 1)),
        (
            "attack_damage_per_level",
            Box::new(|hero| hero.attack_damage_per_level += 1),
        ),
        (
            "armor_per_level",
            Box::new(|hero| hero.armor_per_level += 1),
        ),
        (
            "magic_resist_per_level",
            Box::new(|hero| hero.magic_resist_per_level += 1),
        ),
        (
            "model",
            Box::new(|hero| hero.model = Some(String::from("knight"))),
        ),
    ];
    let last = base.heroes.len() - 1;
    for (field, mutate) in &mutations {
        for index in [0, last] {
            let mut content = base.clone();
            mutate(&mut content.heroes[index]);
            assert_visual_parity(&format!("heroes[{index}].{field}"), &content, &base);
        }
    }

    // Which hero has which name matters, not only whether any name is set.
    let mut modelled = base.clone();
    modelled.heroes[0].model = Some(String::from("knight"));
    let mut renamed = modelled.clone();
    renamed.heroes[0].model = Some(String::from("sentinel"));
    assert_visual_parity("renamed model", &renamed, &modelled);
    let mut moved = modelled.clone();
    moved.heroes[0].model = None;
    moved.heroes[last].model = Some(String::from("knight"));
    assert_visual_parity("moved model", &moved, &modelled);

    // Visual names at capacity, on every wire hero slot, plus an explicit map visuals name.
    let mut content = base.clone();
    let hero = content.heroes[0].clone();
    content.heroes.resize(MAX_CONTENT_HEROES, hero);
    for (index, hero) in content.heroes.iter_mut().enumerate() {
        hero.name = format!("Hero{index}");
        let digit = char::from(b'a' + u8::try_from(index).expect("small"));
        hero.model = Some(std::iter::repeat_n(digit, MAX_VISUAL_NAME).collect());
    }
    content.map.visuals = Some("m".repeat(MAX_VISUAL_NAME));
    assert_visual_parity("visual names at capacity", &content, &base);
    let mut map_only = base.clone();
    map_only.map.visuals = Some(String::from("lava_rock-2"));
    assert_visual_parity("map.visuals", &map_only, &base);
    let mut map_renamed = map_only.clone();
    map_renamed.map.visuals = Some(String::from("ice"));
    assert_visual_parity("map.visuals renamed", &map_renamed, &map_only);
}

#[test]
fn wire_visual_names_are_validated_by_the_client() {
    let mut content = GameContent::default();
    content.heroes[0].model = Some(String::from("knight"));
    content.map.visuals = Some(String::from("lava"));
    let wire = WireCatalog::from(&content);
    assert_eq!(wire.heroes[0].model_str(), Some("knight"));
    assert_eq!(wire.heroes[1].model_str(), None);
    assert_eq!(wire.map.visuals_str(), Some("lava"));

    // A hostile server can put path-like bytes on the wire; `to_content` carries them and
    // the client's `validate` (run on every catalogue it receives) rejects them, so a name
    // can never escape the client's asset root.
    for bad in [&b"../etc/passwd"[..], b"a/b", b"c:x", b"x.glb", b"a\\b"] {
        let mut hostile = wire;
        hostile.heroes[0].model = [0; WIRE_VISUAL_NAME];
        hostile.heroes[0].model[..bad.len()].copy_from_slice(bad);
        hostile.heroes[0].model_len = u8::try_from(bad.len()).expect("short");
        let decoded = hostile.to_content().expect("structurally valid");
        assert_eq!(
            decoded.validate(),
            Err(moba_game::ContentError::BadVisualName("heroes.model")),
            "{bad:?}"
        );
        let mut hostile = wire;
        hostile.map.visuals = [0; WIRE_VISUAL_NAME];
        hostile.map.visuals[..bad.len()].copy_from_slice(bad);
        hostile.map.visuals_len = u8::try_from(bad.len()).expect("short");
        assert_eq!(
            hostile.to_content().expect("structurally valid").validate(),
            Err(moba_game::ContentError::BadVisualName("map.visuals")),
            "{bad:?}"
        );
    }
    // Over-long lengths and invalid UTF-8 are structural errors.
    let mut hostile = wire;
    hostile.heroes[0].model_len = u8::try_from(WIRE_VISUAL_NAME + 1).expect("small");
    assert_eq!(hostile.to_content(), Err(WireError::BadValue));
    let mut hostile = wire;
    hostile.map.visuals[0] = 0xFF;
    assert_eq!(hostile.to_content(), Err(WireError::BadValue));
}

#[test]
#[allow(clippy::too_many_lines)] // One mutation per hashed rules field.
fn wire_rules_parity_covers_every_hashed_field() {
    use moba_game::RulesSpec;
    type Mutation = (&'static str, Box<dyn Fn(&mut RulesSpec)>);
    // Exhaustive destructure with no `..`: adding a `RulesSpec` field fails to compile here
    // until a mutation for it is added below, so a hashed field can't be forgotten on the
    // wire or in the hash.
    let RulesSpec {
        melee_minion,
        ranged_minion,
        tower,
        nexus,
        level_cap,
        xp_base,
        xp_per_level,
        melee_vision,
        ranged_vision,
        tower_vision,
        nexus_vision,
        team_size,
        bot_fill,
        hero_respawn_ticks,
        respawn_per_level,
        wave_interval_ticks,
        first_wave_ticks,
        starting_gold,
        gold_per_interval,
        gold_tick_interval,
        xp_radius,
        shop_radius,
        shared_xp_percent,
        rewards: rules_rewards,
        structure_heroes_first,
        nexus_win,
        score_limit,
    } = RulesSpec::default();
    // Same exhaustive-destructure guard for the nested rewards.
    let moba_game::RewardsSpec {
        melee_minion: melee_reward,
        ranged_minion: ranged_reward,
        tower: tower_reward,
        nexus: nexus_reward,
        hero: hero_reward,
    } = rules_rewards;
    let mutations: Vec<Mutation> = vec![
        (
            "melee_minion",
            Box::new(move |rules| {
                rules.melee_minion.max_health = melee_minion.max_health + 1;
            }),
        ),
        (
            "ranged_minion",
            Box::new(move |rules| {
                rules.ranged_minion.max_health = ranged_minion.max_health + 1;
            }),
        ),
        (
            "tower",
            Box::new(move |rules| {
                rules.tower.max_health = tower.max_health + 1;
            }),
        ),
        (
            "nexus",
            Box::new(move |rules| {
                rules.nexus.max_health = nexus.max_health + 1;
            }),
        ),
        (
            "level_cap",
            Box::new(move |rules| rules.level_cap = level_cap + 1),
        ),
        (
            "xp_base",
            Box::new(move |rules| rules.xp_base = xp_base + 1),
        ),
        (
            "xp_per_level",
            Box::new(move |rules| rules.xp_per_level = xp_per_level + 1),
        ),
        (
            "melee_vision",
            Box::new(move |rules| {
                rules.melee_minion.vision_range = melee_vision + moba_core::Fix::ONE;
                rules.melee_vision = rules.melee_minion.vision_range;
            }),
        ),
        (
            "ranged_vision",
            Box::new(move |rules| {
                rules.ranged_minion.vision_range = ranged_vision + moba_core::Fix::ONE;
                rules.ranged_vision = rules.ranged_minion.vision_range;
            }),
        ),
        (
            "tower_vision",
            Box::new(move |rules| {
                rules.tower.vision_range = tower_vision + moba_core::Fix::ONE;
                rules.tower_vision = rules.tower.vision_range;
            }),
        ),
        (
            "nexus_vision",
            Box::new(move |rules| {
                rules.nexus.vision_range = nexus_vision + moba_core::Fix::ONE;
                rules.nexus_vision = rules.nexus.vision_range;
            }),
        ),
        (
            "team_size",
            Box::new(move |rules| rules.team_size = team_size - 2),
        ),
        (
            "bot_fill",
            Box::new(move |rules| rules.bot_fill = !bot_fill),
        ),
        (
            "hero_respawn_ticks",
            Box::new(move |rules| rules.hero_respawn_ticks = hero_respawn_ticks + 1),
        ),
        (
            "respawn_per_level",
            Box::new(move |rules| rules.respawn_per_level = respawn_per_level + 1),
        ),
        (
            "wave_interval_ticks",
            Box::new(move |rules| rules.wave_interval_ticks = wave_interval_ticks + 1),
        ),
        (
            "first_wave_ticks",
            Box::new(move |rules| rules.first_wave_ticks = first_wave_ticks + 1),
        ),
        (
            "starting_gold",
            Box::new(move |rules| rules.starting_gold = starting_gold + 100),
        ),
        (
            "gold_per_interval",
            Box::new(move |rules| rules.gold_per_interval = gold_per_interval + 1),
        ),
        (
            "gold_tick_interval",
            Box::new(move |rules| rules.gold_tick_interval = gold_tick_interval + 1),
        ),
        (
            "xp_radius",
            Box::new(move |rules| rules.xp_radius = xp_radius + moba_core::Fix::ONE),
        ),
        (
            "shop_radius",
            Box::new(move |rules| rules.shop_radius = shop_radius + moba_core::Fix::ONE),
        ),
        (
            "shared_xp_percent",
            Box::new(move |rules| rules.shared_xp_percent = shared_xp_percent + 1),
        ),
        (
            "rewards.melee_minion.xp",
            Box::new(move |rules| {
                rules.rewards.melee_minion.xp = melee_reward.xp + 1;
            }),
        ),
        (
            "rewards.melee_minion.gold",
            Box::new(move |rules| {
                rules.rewards.melee_minion.gold = melee_reward.gold + 1;
            }),
        ),
        (
            "rewards.ranged_minion.xp",
            Box::new(move |rules| {
                rules.rewards.ranged_minion.xp = ranged_reward.xp + 1;
            }),
        ),
        (
            "rewards.ranged_minion.gold",
            Box::new(move |rules| {
                rules.rewards.ranged_minion.gold = ranged_reward.gold + 1;
            }),
        ),
        (
            "rewards.tower.xp",
            Box::new(move |rules| rules.rewards.tower.xp = tower_reward.xp + 1),
        ),
        (
            "rewards.tower.gold",
            Box::new(move |rules| rules.rewards.tower.gold = tower_reward.gold + 1),
        ),
        (
            "rewards.nexus.xp",
            Box::new(move |rules| rules.rewards.nexus.xp = nexus_reward.xp + 1),
        ),
        (
            "rewards.nexus.gold",
            Box::new(move |rules| rules.rewards.nexus.gold = nexus_reward.gold + 1),
        ),
        (
            "rewards.hero.xp",
            Box::new(move |rules| rules.rewards.hero.xp = hero_reward.xp + 1),
        ),
        (
            "rewards.hero.gold",
            Box::new(move |rules| rules.rewards.hero.gold = hero_reward.gold + 1),
        ),
        (
            "structure_heroes_first",
            Box::new(move |rules| rules.structure_heroes_first = !structure_heroes_first),
        ),
        (
            "nexus_win",
            Box::new(move |rules| rules.nexus_win = !nexus_win),
        ),
        (
            "score_limit",
            Box::new(move |rules| rules.score_limit = score_limit + 1),
        ),
    ];
    let base = GameContent::default();
    for (name, mutate) in &mutations {
        let mut content = base.clone();
        mutate(&mut content.rules);
        assert!(
            content.validate().is_ok(),
            "{name}: mutated content must stay valid: {:?}",
            content.validate()
        );
        assert_ne!(
            content.hash(),
            base.hash(),
            "{name}: mutation must change the content hash"
        );
        let wire = WireCatalog::from(&content);
        assert_eq!(
            wire.content_hash().expect("wire hash"),
            content.hash(),
            "{name}: hashed rules field is not carried on the wire"
        );
        assert_eq!(
            wire.to_content().expect("content").rules,
            content.rules,
            "{name}: rules differ after From/to_content"
        );
        let mut bytes = Vec::new();
        encode_catalog(&wire, &mut bytes);
        let mut reader = Reader::new(&bytes);
        let decoded = decode_catalog(&mut reader).expect("decode catalogue");
        assert_eq!(reader.remaining(), 0, "{name}: trailing bytes");
        assert_eq!(decoded, wire, "{name}: encode/decode round trip");
    }
}

/// Asserts `content` is valid and crosses the wire losslessly (hash, content and bytes).
fn assert_wire_parity(name: &str, content: &GameContent) {
    assert_eq!(
        content.validate(),
        Ok(()),
        "{name}: mutation must stay valid"
    );
    let wire = WireCatalog::from(content);
    assert_eq!(
        wire.content_hash().expect("wire hash"),
        content.hash(),
        "{name}: client-reconstructed hash differs"
    );
    assert_eq!(
        &wire.to_content().expect("content").map,
        &content.map,
        "{name}: map differs after From/to_content"
    );
    let mut bytes = Vec::new();
    encode_catalog(&wire, &mut bytes);
    let mut reader = Reader::new(&bytes);
    assert_eq!(
        decode_catalog(&mut reader).expect("decode catalogue"),
        wire,
        "{name}: encode/decode round trip"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One mutation table per map coordinate kind.
fn wire_map_parity_covers_every_coordinate_kind() {
    use moba_core::{Fix, FixVec2};
    use moba_game::{ContentError, LaneSpec, MapSpec, StructureSpec, WallSpec};
    type Set = Box<dyn Fn(&mut MapSpec, Fix)>;
    let mut base = GameContent::default();
    base.map.walls.push(WallSpec {
        a: FixVec2::from_ints(80, 96),
        b: FixVec2::from_ints(80, 160),
    });
    assert_wire_parity("base", &base);
    // Exhaustive destructures with no `..`: adding map geometry fails to compile here until
    // it has a coordinate mutation below (or is listed as non-geometric).
    let MapSpec {
        name: _,
        theme: _,
        extent: _,
        lanes,
        structures,
        walls,
        blue_spawn: _,
        red_spawn: _,
        // A name, not geometry: covered by `wire_hero_parity_covers_every_hashed_field`.
        visuals: _,
    } = &base.map;
    let LaneSpec { name: _, path: _ } = &lanes[0];
    let StructureSpec {
        team: _,
        position: _,
        kind: _,
        lane: _,
    } = &structures[0];
    let WallSpec { a: _, b: _ } = &walls[0];
    let axis = |set: fn(&mut FixVec2, Fix), select: fn(&mut MapSpec) -> &mut FixVec2| -> Set {
        Box::new(move |map, value| set(select(map), value))
    };
    let x: fn(&mut FixVec2, Fix) = |point, value| point.x = value;
    let y: fn(&mut FixVec2, Fix) = |point, value| point.y = value;
    let blue: fn(&mut MapSpec) -> &mut FixVec2 = |map| &mut map.blue_spawn;
    let red: fn(&mut MapSpec) -> &mut FixVec2 = |map| &mut map.red_spawn;
    let lane: fn(&mut MapSpec) -> &mut FixVec2 = |map| &mut map.lanes[0].path[1];
    let structure: fn(&mut MapSpec) -> &mut FixVec2 = |map| &mut map.structures[0].position;
    let wall_a: fn(&mut MapSpec) -> &mut FixVec2 = |map| &mut map.walls[0].a;
    let wall_b: fn(&mut MapSpec) -> &mut FixVec2 = |map| &mut map.walls[0].b;
    let point_cases: Vec<(&'static str, Set)> = vec![
        ("map.blue_spawn", axis(x, blue)),
        ("map.blue_spawn", axis(y, blue)),
        ("map.red_spawn", axis(x, red)),
        ("map.red_spawn", axis(y, red)),
        ("map.lanes.path", axis(x, lane)),
        ("map.lanes.path", axis(y, lane)),
        ("map.structures.position", axis(x, structure)),
        ("map.structures.position", axis(y, structure)),
        ("map.walls.a", axis(x, wall_a)),
        ("map.walls.a", axis(y, wall_a)),
        ("map.walls.b", axis(x, wall_b)),
        ("map.walls.b", axis(y, wall_b)),
    ];
    // (field, setter, valid whole value, values outside the wire integer type).
    let mut cases: Vec<(&'static str, Set, Fix, [Fix; 2])> = point_cases
        .into_iter()
        .map(|(field, set)| {
            (
                field,
                set,
                Fix::from_i32(101),
                [Fix::from_i32(40_000), Fix::from_i32(-40_000)],
            )
        })
        .collect();
    cases.push((
        "map.extent",
        Box::new(|map, value| map.extent = value),
        Fix::from_i32(250),
        [Fix::from_i32(70_000), Fix::from_i32(-1)],
    ));
    for (field, set, valid, out_of_wire_range) in &cases {
        let mut content = base.clone();
        set(&mut content.map, *valid);
        assert_ne!(content.hash(), base.hash(), "{field}: must change the hash");
        assert_wire_parity(field, &content);

        for fractional in [
            *valid + Fix::from_f64(0.5),
            Fix::from_bits(valid.to_bits() + 1),
        ] {
            let mut content = base.clone();
            set(&mut content.map, fractional);
            assert_eq!(
                content.validate(),
                Err(ContentError::BadMapCoordinate(field)),
                "{field}: fractional {fractional:?}"
            );
            // Why validation matters: the wire would truncate this and break the hash.
            assert_ne!(
                WireCatalog::from(&content).content_hash(),
                Ok(content.hash()),
                "{field}: fractional value must not survive the wire"
            );
        }
        for value in out_of_wire_range {
            let mut content = base.clone();
            set(&mut content.map, *value);
            assert_eq!(
                content.validate(),
                Err(ContentError::BadMapCoordinate(field)),
                "{field}: {value:?} does not fit the wire type"
            );
            assert_ne!(
                WireCatalog::from(&content).content_hash(),
                Ok(content.hash()),
                "{field}: out-of-range value must not survive the wire"
            );
        }
        // Whole and wire-representable, but outside the world: still rejected.
        let mut content = base.clone();
        set(&mut content.map, Fix::from_i32(300));
        assert!(
            content.validate().is_err(),
            "{field}: 300 is outside the 0..=256 world"
        );
    }

    // The issue's exact repro.
    let mut content = GameContent::default();
    content.map.extent = Fix::from_f64(128.5);
    assert_eq!(
        content.validate(),
        Err(ContentError::BadMapCoordinate("map.extent"))
    );
}

#[test]
fn wire_map_names_and_counts_are_validated_instead_of_truncated() {
    use moba_core::FixVec2;
    use moba_game::{
        ContentError, MAX_CONTENT_LANES, MAX_CONTENT_STRUCTURES, MAX_LANE_NAME, MAX_LANE_POINTS,
        MAX_MANIFEST_FIELD,
    };
    let base = GameContent::default();
    let check = |name: &str, mutate: &dyn Fn(&mut GameContent), expected: ContentError| {
        let mut content = base.clone();
        mutate(&mut content);
        assert_eq!(content.validate(), Err(expected), "{name}");
    };
    check(
        "map name too long",
        &|c| c.map.name = "m".repeat(MAX_MANIFEST_FIELD + 1),
        ContentError::BadMapData,
    );
    check(
        "map name non-ASCII",
        &|c| c.map.name = String::from("Kartë"),
        ContentError::BadMapData,
    );
    check(
        "lane name too long",
        &|c| c.map.lanes[0].name = "l".repeat(MAX_LANE_NAME + 1),
        ContentError::BadMapData,
    );
    check(
        "lane name empty",
        &|c| c.map.lanes[0].name.clear(),
        ContentError::BadMapData,
    );
    check(
        "too many lanes",
        &|c| {
            let lane = c.map.lanes[0].clone();
            c.map.lanes.resize(MAX_CONTENT_LANES + 1, lane);
        },
        ContentError::BadMap,
    );
    check(
        "too many lane points",
        &|c| {
            let point = c.map.lanes[0].path[0];
            c.map.lanes[0].path.resize(MAX_LANE_POINTS + 1, point);
        },
        ContentError::BadMap,
    );
    check(
        "empty lane path",
        &|c| c.map.lanes[0].path.clear(),
        ContentError::BadMap,
    );
    check(
        "too many structures",
        &|c| {
            let structure = c.map.structures[0];
            c.map
                .structures
                .resize(MAX_CONTENT_STRUCTURES + 1, structure);
        },
        ContentError::BadMap,
    );

    // The limits themselves are carried exactly.
    let mut content = base.clone();
    content.map.name = "m".repeat(MAX_MANIFEST_FIELD);
    content.map.lanes[0].name = "l".repeat(MAX_LANE_NAME);
    let point = content.map.lanes[0].path[0];
    content.map.lanes[0].path.resize(MAX_LANE_POINTS, point);
    let lane = content.map.lanes[0].clone();
    content.map.lanes.resize(MAX_CONTENT_LANES, lane);
    let structure = content.map.structures[0];
    content
        .map
        .structures
        .resize(MAX_CONTENT_STRUCTURES, structure);
    content.map.walls = (0..moba_game::MAX_CONTENT_WALLS)
        .map(|index| moba_game::WallSpec {
            a: FixVec2::from_ints(1, i32::try_from(index).expect("small")),
            b: FixVec2::from_ints(2, i32::try_from(index).expect("small")),
        })
        .collect();
    assert_wire_parity("capacity limits", &content);
}

#[test]
fn wire_catalogue_round_trips_nonzero_projectile_arc() {
    use moba_game::{Effect, GameContent};
    let mut content = GameContent::default();
    content.abilities[2].effects.effects[0] = Effect::Projectile {
        speed_per_sec: moba_core::Fix::from_i32(26),
        range: moba_core::Fix::from_i32(11),
        radius: moba_core::Fix::ONE,
        arc: moba_core::Fix::from_i32(3),
        on_hit: moba_game::AreaEffectList::one(moba_game::AreaEffect::Damage {
            amount: 10,
            damage_type: moba_game::DamageType::Physical,
        }),
    };
    let wire = WireCatalog::from(&content);
    assert_eq!(wire.abilities[2].effects[0].arc_bits, 3i64 << 32);
    let decoded = wire.to_content().expect("round trip");
    assert_eq!(decoded.hash(), content.hash());
}

#[test]
fn delta_builder_reuses_capacity() {
    let baseline = vec![entity(1, 10, 20, 600), entity(2, 30, 40, 200)];
    let mut delta = SnapshotDelta::default();
    build_delta_into(&baseline, None, 1, 0, 0, &mut delta);
    let spawn_capacity = delta.spawns.capacity();
    assert_eq!(delta.spawns.len(), 2);
    build_delta_into(&baseline, None, 2, 0, 0, &mut delta);
    assert_eq!(delta.spawns.capacity(), spawn_capacity);
    assert_eq!(delta.spawns.len(), 2);
    assert_eq!(delta.tick, 2);
}

#[test]
fn full_and_delta_snapshots_apply() {
    let baseline = vec![entity(1, 10, 20, 600), entity(2, 30, 40, 200)];
    let mut current = baseline.clone();
    current[0].x = 11;
    current[0].health = 500;
    current[0].progress = NetProgress {
        level: 5,
        xp: 120,
        gold: 750,
        items: [0, 1, 0xFF, 0xFF, 0xFF, 0xFF],
        item_cooldowns: [0; 6],
    };
    current[1].mana = 50;
    current.push(entity(3, 50, 60, 600));

    let mut full = build_delta(&baseline, None, 3, 42, 0);
    full.blue_score = 3;
    full.red_score = 2;
    full.outcome = 1;
    full.phase = 2;
    full.countdown = 0;
    let mut payload = Vec::new();
    encode_snapshot(&full, &mut payload).unwrap();
    let decoded = decode_snapshot(&payload).unwrap();
    let state = SnapshotState::from_full(3, &decoded);
    assert_eq!(state.entities.len(), 2);
    assert_eq!((state.blue_score, state.red_score), (3, 2));
    assert_eq!(state.outcome, 1);
    assert_eq!(state.phase, 2);

    let mut delta = build_delta(&current, Some(&baseline), 4, 43, 3);
    delta.blue_score = 4;
    delta.red_score = 2;
    delta.outcome = 2;
    delta.phase = 3;
    delta.countdown = 450;
    delta.events.push(NetEvent {
        kind: EVENT_PROJECTILE_SPAWN,
        team: 0,
        source: 1,
        target: 0xFFFF,
        source_slot: 0,
        target_slot: 0xFF,
        x: 10,
        y: 20,
        x2: 30,
        y2: 40,
        value: 24,
        aux: 1,
        dx: 16_383,
        dy: -16_383,
        arc: 40,
    });
    assert_eq!(delta.spawns.len(), 1);
    assert_eq!(delta.updates.len(), 2);
    encode_snapshot(&delta, &mut payload).unwrap();
    let decoded = decode_snapshot(&payload).unwrap();
    assert_eq!(decoded.events, delta.events);
    let next = state.apply(&decoded, 4).unwrap();
    assert_eq!(next.entities.len(), 3);
    assert_eq!(next.get(1).unwrap().health, 500);
    assert_eq!(next.get(1).unwrap().progress.level, 5);
    assert_eq!(next.get(1).unwrap().progress.gold, 750);
    assert_eq!(next.get(1).unwrap().progress.items[1], 1);
    assert_eq!(next.get(1).unwrap().x, 11);
    assert_eq!(next.get(2).unwrap().mana, 50);
    assert_eq!(next.get(3).unwrap().x, 50);
    assert_eq!((next.blue_score, next.red_score), (4, 2));
    assert_eq!(next.outcome, 2);
    assert_eq!(next.phase, 3);
    assert_eq!(next.countdown, 450);
}

#[test]
fn delta_with_removals_and_wrong_baseline() {
    let baseline = vec![entity(1, 10, 20, 600), entity(2, 30, 40, 200)];
    let current = vec![entity(2, 30, 40, 200)];
    let delta = build_delta(&current, Some(&baseline), 2, 0, 1);
    assert_eq!(delta.despawns, vec![1]);
    let state = SnapshotState {
        seq: 1,
        tick: 1,
        blue_score: 0,
        red_score: 0,
        outcome: 0,
        phase: 0,
        countdown: 0,
        ready_mask: 0,
        player_mask: 0,
        entities: baseline,
    };
    let applied = state.apply(&delta, 2).unwrap();
    assert_eq!(applied.entities.len(), 1);
    assert_eq!(applied.get(2).unwrap().net_id, 2);
    let wrong = SnapshotState {
        seq: 9,
        tick: 9,
        blue_score: 0,
        red_score: 0,
        outcome: 0,
        phase: 0,
        countdown: 0,
        ready_mask: 0,
        player_mask: 0,
        entities: applied.entities.clone(),
    };
    assert_eq!(wrong.apply(&delta, 3), Err(WireError::MissingBaseline));
}

#[test]
fn position_quantisation_is_bounded() {
    let position = FixVec2::from_ints(128, 64);
    let (x, y) = quantize_position(position);
    let round = dequantize_position(x, y);
    assert!(round.distance(position) <= Fix::from_f64(0.05));
    assert_eq!(
        quantize_position(FixVec2::from_ints(-10, 999)),
        (0, MAX_QUANTIZED_POSITION)
    );
}

#[test]
fn decoders_never_panic_on_noise() {
    let mut rng = moba_core::Rng::from_seed(0xF022);
    for length in 0..256usize {
        for _ in 0..20 {
            let mut data = vec![0u8; length];
            rng.fill_bytes(&mut data);
            let _ = decode_packet(&data);
            let _ = decode_snapshot(&data);
            let _ = decode_input(&data);
            let _ = decode_hello(&data);
            let _ = decode_welcome(&data);
        }
    }
    // Random payloads behind a valid header exercise the message decoders too.
    let mut payload = vec![0u8; 512];
    for _ in 0..200 {
        rng.fill_bytes(&mut payload);
        let mut packet = Vec::new();
        encode_packet(&header(PacketKind::Snapshot, 1), &payload, &mut packet).unwrap();
        let (_, body) = decode_packet(&packet).unwrap();
        let _ = decode_snapshot(body);
        let _ = decode_input(body);
    }
}

#[test]
fn serial_sequence_tolerates_wrap() {
    assert!(sequence_after(1, 0));
    assert!(!sequence_after(0, 1));
    assert!(!sequence_after(5, 5));
    assert!(sequence_after(2, u32::MAX));
    assert!(!sequence_after(u32::MAX, 2));
}

#[test]
fn observed_unit_conversion() {
    let mut unit = ObservedUnit::new(
        Entity::from_parts(3, 1),
        UnitKind::Hero,
        Team::Red,
        FixVec2::from_ints(120, 80),
    );
    unit.health = 500;
    unit.max_health = 600;
    unit.mana = 200;
    unit.max_mana = 300;
    unit.hero_slot = Some(7);
    unit.hero_pick = 2;
    unit.cooldowns = [10, 20, 30];
    unit.level = 4;
    unit.xp = 120;
    unit.gold = 950;
    unit.items = [0, 1, 2, 0xFF, 0xFF, 0xFF];
    unit.shielded = true;
    unit.burning = true;
    unit.hasted = true;
    let entity = NetEntity::from_observed(4, &unit);
    assert_eq!(entity.net_id, 4);
    assert_eq!(entity.position(), unit.position);
    assert_eq!(entity.hero_pick, 2);
    assert_eq!(entity.progress.level, 4);
    assert_eq!(entity.progress.gold, 950);
    assert!(!entity.is_dead());
    assert!(entity.flags & ENTITY_FLAG_SHIELD != 0);
    assert!(entity.flags & ENTITY_FLAG_BURN != 0);
    assert!(entity.flags & ENTITY_FLAG_HASTE != 0);
    assert!(entity.status_bits & STATUS_SHIELD != 0);
    assert!(entity.status_bits & STATUS_BURN != 0);
    assert!(entity.status_bits & STATUS_HASTE != 0);
}

#[test]
fn attack_events_round_trip_compactly_next_to_full_events() {
    let attack = NetEvent {
        kind: EVENT_ATTACK,
        team: 1,
        source: 7,
        target: 0x1234,
        source_slot: 6,
        target_slot: 0xFF,
        value: 34,
        aux: 2,
        ..NetEvent::default()
    };
    let cast = NetEvent {
        kind: EVENT_CAST,
        team: 0,
        source: 3,
        target: 0xFFFF,
        source_slot: 0,
        target_slot: 0xFF,
        x: 10,
        y: 20,
        x2: 30,
        y2: 40,
        value: 1,
        aux: 9,
        dx: -5,
        dy: 6,
        arc: 7,
    };
    let mut delta = SnapshotDelta {
        tick: 9,
        ..SnapshotDelta::default()
    };
    let mut payload = Vec::new();
    encode_snapshot(&delta, &mut payload).expect("encode");
    let empty = payload.len();
    delta.events = vec![attack, cast, attack];
    encode_snapshot(&delta, &mut payload).expect("encode");
    // Two 12-byte attacks and one 26-byte event.
    assert_eq!(payload.len() - empty, 2 * 12 + 26);
    let decoded = decode_snapshot(&payload).expect("decode");
    assert_eq!(decoded.events, delta.events);
    // The fields an attack does not carry are not sent: they decode as zero.
    let mut positioned = attack;
    positioned.x = 99;
    positioned.arc = -3;
    delta.events = vec![positioned];
    encode_snapshot(&delta, &mut payload).expect("encode");
    assert_eq!(decode_snapshot(&payload).expect("decode").events, [attack]);
    // A truncated attack record is an error, never a panic.
    for cut in 1..12 {
        delta.events = vec![attack];
        encode_snapshot(&delta, &mut payload).expect("encode");
        payload.truncate(payload.len() - cut);
        assert!(decode_snapshot(&payload).is_err(), "cut {cut}");
    }
}
