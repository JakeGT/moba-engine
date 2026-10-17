//! Runtime item content is validated, published in the handshake and used by the simulation.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moba_game::{GameContent, HeroSpec, ItemSpec};
use moba_net::protocol::WireCommand;
use moba_net::{NetClient, NetServer, ServerConfig};
use moba_sim::SimConfig;

fn custom_content() -> GameContent {
    GameContent {
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
        }],
    }
}

fn spawn_server(content: GameContent) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let config = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        sim: SimConfig::default(),
        snapshot_interval: 1,
        max_clients: 10,
        countdown_ticks: 0,
        results_ticks: 0,
        client_timeout: Duration::from_secs(2),
        reconnect_grace: Duration::from_secs(2),
        pre_shared_key: None,
        record: None,
        max_spectators: 4,
        require_ready: false,
    };
    let mut server = NetServer::bind_with_content(config, Arc::new(content)).expect("bind");
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
        }) {
            if own.progress.items.contains(&0) {
                assert!(own.progress.gold < 500);
                bought = true;
                break;
            }
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
        if let Some(state) = client.snapshot() {
            if let Some(own) = state
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
    }
    assert!(bought, "custom item purchased through the server");
    let _ = client.disconnect();
    server.join().expect("server thread");
}
