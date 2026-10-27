//! Runtime item content is validated, published in the handshake and used by the simulation.

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use moba_game::{ContentError, GameContent, HeroSpec, ItemSpec};
use moba_net::protocol::{WireCatalog, WireCommand};
use moba_net::{
    Header, NetClient, NetServer, PacketKind, ServerConfig, UdpTransport, Welcome, decode_hello,
    encode_welcome,
};
use moba_sim::SimConfig;

fn custom_content() -> GameContent {
    GameContent {
        attestation: None,
        rules: moba_game::RulesSpec::default(),
        manifest: moba_game::ContentManifest::default(),
        items: vec![
            ItemSpec {
                name: String::from("Test Blade"),
                cost: 10,
                attack_damage: 5,
                ..ItemSpec::from(moba_game::ITEMS.first().expect("default item"))
            },
            ItemSpec {
                name: String::from("Test Vest"),
                cost: 20,
                health: 40,
                ..ItemSpec::from(moba_game::ITEMS.first().expect("default item"))
            },
        ],
        heroes: vec![HeroSpec {
            name: String::from("Test Hero"),
            max_health: 700,
            max_mana: 300,
            health_regen_per_tick: 2,
            mana_regen_per_tick: 4,
            attack_damage: 60,
            attack_range: moba_core::Fix::from_i32(2),
            attack_interval_ticks: 30,
            move_speed_per_sec: moba_core::Fix::from_i32(7),
            armor: 20,
            magic_resist: 20,
            vision_range: moba_core::Fix::from_i32(14),
            abilities: [0, 1, 2],
            health_per_level: 70,
            mana_per_level: 30,
            attack_damage_per_level: 4,
            armor_per_level: 1,
            magic_resist_per_level: 1,
        }],
        abilities: GameContent::default().abilities,
        modifiers: GameContent::default().modifiers,
        map: GameContent::default().map,
    }
}

fn spawn_server(content: GameContent) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let mut server =
        NetServer::bind_with_content(server_config(), Arc::new(content)).expect("bind");
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        server.run_for(Duration::from_secs(3)).expect("server");
    });
    (addr, handle)
}

#[test]
fn custom_item_catalogue_reaches_client_and_shop() {
    let content = custom_content();
    assert_eq!(content.validate(), Ok(()));
    let (addr, server) = spawn_server(content);
    let mut client = NetClient::connect(addr, Duration::from_secs(2)).expect("connect");
    assert_eq!(client.items().len(), 2);
    assert_eq!(client.items()[0].name_str(), "Test Blade");
    assert_eq!(client.heroes().len(), 1);
    assert_eq!(client.heroes()[0].name_str(), "Test Hero");
    assert_ne!(client.abilities().len(), 0);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) && client.snapshot().is_none() {
        client.poll().expect("poll");
        client.update().expect("update");
        std::thread::sleep(Duration::from_millis(5));
    }
    client
        .send_command(WireCommand::BuyItem { item: 0 })
        .expect("buy");
    let start = Instant::now();
    let mut bought = false;
    while start.elapsed() < Duration::from_secs(1) {
        client.poll().expect("poll");
        client.update().expect("update");
        if let Some(own) = client.snapshot().and_then(|state| {
            state
                .entities
                .iter()
                .find(|entity| entity.hero_slot == client.player_slot())
        }) && own.progress.items.contains(&0)
        {
            assert!(own.progress.gold < 500);
            bought = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if !bought {
        eprintln!(
            "slot={} spectator={} state={:?}",
            client.player_slot(),
            client.is_spectator(),
            client.snapshot().map(|state| state
                .entities
                .iter()
                .map(|entity| (entity.kind, entity.team, entity.hero_slot))
                .collect::<Vec<_>>())
        );
        if let Some(state) = client.snapshot()
            && let Some(own) = state
                .entities
                .iter()
                .find(|entity| entity.hero_slot == client.player_slot())
        {
            eprintln!(
                "own gold={} items={:?}",
                own.progress.gold, own.progress.items
            );
        }
    }
    assert!(bought, "custom item purchased through the server");
    let _ = client.disconnect();
    server.join().expect("server thread");
}

#[test]
fn economy_override_pack_reaches_a_real_udp_client() {
    let content =
        moba_game::load_content_toml(include_str!("../../../content/economy-override.toml"))
            .expect("economy pack parses");
    assert_eq!(content.validate(), Ok(()));
    assert_eq!(
        content.rules.starting_gold, 750,
        "pack must override the default economy"
    );
    let expected = WireCatalog::from(&content).rules;
    let (addr, server) = spawn_server(content.clone());
    let mut client = NetClient::connect(addr, Duration::from_secs(2)).expect("connect");
    assert_eq!(
        client.rules(),
        &expected,
        "client rules must equal the server's wire rules"
    );
    assert_eq!(
        client.content_hash(),
        content.hash(),
        "wire catalogue hash must match the server pack"
    );
    assert_eq!(client.rules().starting_gold, 750);
    assert_eq!(client.rules().gold_per_interval, 2);
    assert_eq!(client.rules().gold_tick_interval, 20);
    assert_eq!(
        client.rules().xp_radius_bits,
        moba_core::Fix::from_i32(42).to_bits()
    );
    assert_eq!(
        client.rules().shop_radius_bits,
        moba_core::Fix::from_i32(36).to_bits()
    );
    assert_eq!(client.rules().shared_xp_percent, 175);
    assert_eq!(client.rules().rewards.melee_minion.xp, 40);
    assert_eq!(client.rules().rewards.hero.gold, 350);
    assert!(client.rules().structure_heroes_first);
    assert!(!client.rules().nexus_win);
    assert_eq!(client.rules().score_limit, 25);
    let _ = client.disconnect();
    server.join().expect("server thread");
}

fn server_config() -> ServerConfig {
    ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        sim: SimConfig::default(),
        snapshot_interval: 1,
        max_clients: 10,
        countdown_ticks: 0,
        results_ticks: 0,
        client_timeout: Duration::from_secs(3),
        reconnect_grace: Duration::from_secs(2),
        pre_shared_key: None,
        record: None,
        max_spectators: 4,
        max_packets_per_second: 120,
        max_total_packets_per_second: 2_000,
        require_ready: false,
    }
}

#[test]
fn bind_with_content_rejects_invalid_content() {
    // The #296 repro: a fractional extent used to pass `validate()` and truncate on the wire.
    let mut fractional_extent = GameContent::default();
    fractional_extent.map.extent = moba_core::Fix::from_f64(128.5);
    let mut rules_invalid = GameContent::default();
    rules_invalid.rules.level_cap = 0;
    for (content, expected) in [
        (
            fractional_extent,
            ContentError::BadMapCoordinate("map.extent"),
        ),
        (rules_invalid, ContentError::BadRules("level_cap")),
    ] {
        let error = NetServer::bind_with_content(server_config(), Arc::new(content))
            .expect_err("invalid content must not bind");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        assert_eq!(
            error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<ContentError>()),
            Some(&expected),
            "{error}"
        );
    }
    NetServer::bind_with_content(server_config(), Arc::new(GameContent::default()))
        .expect("valid content binds");
}

/// Answers every Hello with a plaintext Welcome carrying `catalog` and the content hash the
/// client reconstructs from it (so only validation can make the client refuse it), and
/// returns the client's connect result.
fn connect_to_scripted_server(catalog: &WireCatalog) -> std::io::Result<NetClient> {
    let mut transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap()).expect("bind");
    let addr = transport.local_addr().expect("addr");
    let content_hash = catalog.content_hash().expect("catalogue reconstructs");
    let catalog = *catalog;
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !server_stop.load(Ordering::Relaxed) && Instant::now() < deadline {
            let Some(message) = transport.recv().expect("recv") else {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            };
            if message.header.kind != PacketKind::Hello {
                continue;
            }
            let hello = decode_hello(&message.payload).expect("hello");
            let welcome = Welcome {
                slot: 0,
                tick_rate: 30,
                snapshot_interval: 1,
                tick: 0,
                seed: 1,
                hero_pick: 0,
                nonce: hello.nonce,
                server_nonce: 0,
                auth: [0; 32],
                spectator: false,
                catalog,
                content_hash,
                identity: false,
                server_public_key: [0; 32],
                server_signature: [0; 64],
            };
            let header = Header {
                kind: PacketKind::Welcome,
                flags: 0,
                session: 0x5EED,
                seq: 1,
                ack: 0,
                ack_bits: 0,
            };
            transport
                .send(message.addr, &header, &encode_welcome(&welcome))
                .expect("send welcome");
        }
    });
    let result = NetClient::connect(addr, Duration::from_millis(1_500));
    stop.store(true, Ordering::Relaxed);
    server.join().expect("scripted server");
    result
}

#[test]
fn client_welcome_validates_the_catalogue_like_content_update() {
    // Control: the scripted server's Welcome is accepted for valid content.
    let client = connect_to_scripted_server(&WireCatalog::from(&GameContent::default()))
        .expect("a valid catalogue is welcomed");
    assert_eq!(client.content_hash(), GameContent::default().hash());
    drop(client);

    // A rules-invalid pack whose catalogue round-trips with a matching hash.
    let mut rules_invalid = GameContent::default();
    rules_invalid.rules.level_cap = 0;
    // A map-invalid catalogue: extent 0 reconstructs cleanly but fails `validate()`.
    let rules_invalid = WireCatalog::from(&rules_invalid);
    let mut map_invalid = WireCatalog::from(&GameContent::default());
    map_invalid.map.extent = 0;
    for (name, catalog) in [("rules", &rules_invalid), ("map", &map_invalid)] {
        assert!(
            catalog
                .to_content()
                .expect("reconstructs")
                .validate()
                .is_err(),
            "{name}: fixture must be invalid"
        );
        let error = connect_to_scripted_server(catalog)
            .expect_err("a Welcome with invalid content must be refused");
        assert_eq!(error.kind(), ErrorKind::TimedOut, "{name}: {error}");
    }
}
