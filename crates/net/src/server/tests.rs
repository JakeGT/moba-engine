use super::clients::NetIdTable;
use super::match_flow::slot_team;
use super::results::result_json;
use super::*;
use crate::crypto::SessionCrypto;
use crate::protocol::{
    EVENT_ATTACK, EVENT_CAST, EVENT_MATCH_ENDED, EVENT_PROJECTILE_HIT, EVENT_UNIT_DIED, Header,
    MATCH_ENDED_DRAW, OUTCOME_BLUE, OUTCOME_DRAW, OUTCOME_NONE, OUTCOME_RED, PacketKind,
};
use crate::transport::ReceivedMessage;
use std::path::Path;
#[global_allocator]
static TEST_ALLOC: moba_test_support::CountingAllocator = moba_test_support::CountingAllocator;
use moba_sim::{Health, MatchOutcome, UnitKind};
use std::sync::{Arc, Mutex};

#[test]
fn dense_net_ids_handle_generation_reuse() {
    let mut table = NetIdTable::default();
    let first = moba_ecs::Entity::from_parts(5, 1);
    let id = table.get_or_insert(first);
    assert_eq!(table.get_or_insert(first), id);
    assert_eq!(table.entity_for(id), Some(first));
    let reused = moba_ecs::Entity::from_parts(5, 2);
    let new_id = table.get_or_insert(reused);
    assert_ne!(id, new_id);
    assert_eq!(table.entity_for(new_id), Some(reused));
    table.clear();
    assert_eq!(table.entity_for(new_id), None);
    assert_eq!(table.get_or_insert(reused), 1);
}

#[test]
fn extension_systems_run_in_order_after_tick() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(1)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(1))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(0)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let order = Arc::new(Mutex::new(Vec::new()));
    let first = Arc::clone(&order);
    let second = Arc::clone(&order);
    let mut server = NetServer::bind(config)
        .expect("bind")
        .with_system(move |simulation, phase| {
            assert_eq!(phase, ServerPhase::Playing);
            let _ = simulation.world().entity_count();
            first.lock().expect("lock").push(1u8);
        })
        .with_system(move |_, phase| {
            second
                .lock()
                .expect("lock")
                .push(if phase == ServerPhase::Playing { 2 } else { 9 });
        });
    assert_eq!(server.system_count(), 2);
    server.phase = Phase::Playing;
    server.tick().expect("tick");
    assert_eq!(*order.lock().expect("lock"), vec![1, 2]);
}

#[test]
fn extension_systems_see_every_server_phase() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(2)
        .with_results_ticks(2)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(1))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(0)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let mut server = NetServer::bind(config)
        .expect("bind")
        .with_system(move |_, phase| recorder.lock().expect("lock").push(phase));
    server.phase = Phase::Lobby;
    server.tick().expect("lobby");
    server.phase = Phase::Countdown(2);
    server.tick().expect("countdown");
    server.phase = Phase::Results(2);
    server.tick().expect("results");
    let phases = seen.lock().expect("lock");
    assert!(phases.contains(&ServerPhase::Lobby));
    assert!(phases.contains(&ServerPhase::Countdown));
    assert!(phases.contains(&ServerPhase::Results));
}

#[test]
fn client_identity_keys_rotate_and_revoke() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(1)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(1))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(0)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    let old = [3u8; 32];
    let new = [4u8; 32];
    server.require_client_identity([old]);
    assert!(server.client_key_allowed(old));
    assert!(!server.client_key_allowed(new));
    assert!(server.rotate_client_key(old, new));
    assert!(!server.client_key_allowed(old));
    assert!(server.client_key_allowed(new));
    assert!(!server.rotate_client_key(old, new));
    assert!(server.revoke_client_key(new));
    assert!(!server.client_key_allowed(new));
    assert_eq!(server.allowed_client_key_count(), 0);
    assert!(!server.revoke_client_key(new));
}

#[test]
fn rotation_selects_the_next_pack_when_the_lobby_is_empty() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(1)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(1))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(0)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let mut server = NetServer::bind(config).expect("bind");
    server.watch_rotation(vec![root.join("aram.toml"), root.join("1v1-mid.toml")]);
    server.phase = Phase::Lobby;
    server.reset_match();
    assert_eq!(server.content.map.name, "1v1 Mid");
    server.reset_match();
    assert_eq!(server.content.map.name, "ARAM");
}

#[test]
fn event_visibility_requires_an_observed_source_or_target() {
    let source = moba_ecs::Entity::from_parts(9, 1);
    let target = moba_ecs::Entity::from_parts(10, 1);
    let raw = RawEvent {
        kind: EVENT_CAST,
        team: moba_game::Team::Red,
        source: Some(source),
        target: None,
        source_slot: 6,
        target_slot: 0xFF,
        position: moba_core::FixVec2::from_ints(100, 100),
        end: moba_core::FixVec2::ZERO,
        direction: moba_core::FixVec2::ZERO,
        value: 1,
        aux: 0,
        arc: moba_core::Fix::ZERO,
        tick: 0,
    };
    let hidden = RawEvent::visible_to(&raw, moba_game::Team::Blue, false, &[], |_| true);
    assert!(!hidden, "hidden enemy cast must not leak");
    let mut unit = moba_sim::ObservedUnit::new(
        source,
        UnitKind::Hero,
        moba_game::Team::Red,
        moba_core::FixVec2::from_ints(100, 100),
    );
    unit.health = 500;
    unit.max_health = 600;
    unit.mana = 200;
    unit.max_mana = 300;
    unit.hero_slot = Some(6);
    unit.hero_pick = 0;
    unit.level = 1;
    assert!(RawEvent::visible_to(
        &raw,
        moba_game::Team::Blue,
        false,
        &[unit],
        |_| false
    ));
    assert!(RawEvent::visible_to(
        &raw,
        moba_game::Team::Blue,
        true,
        &[],
        |_| false
    ));
    let death = RawEvent {
        kind: EVENT_UNIT_DIED,
        team: moba_game::Team::Blue,
        source: None,
        target: Some(target),
        source_slot: 0xFF,
        target_slot: 0,
        ..raw
    };
    assert!(!RawEvent::visible_to(
        &death,
        moba_game::Team::Blue,
        false,
        &[],
        |_| false
    ));
    // A death the viewer saw coming (the victim was in its last snapshot) is sent, though
    // the victim is gone now; a cast never is.
    assert!(RawEvent::visible_to(
        &death,
        moba_game::Team::Blue,
        false,
        &[],
        |entity| entity == target
    ));
}

#[test]
fn reload_team_size_reassigns_connected_slots() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    let mut content = GameContent::default();
    content.rules.team_size = 1;
    server.content = Arc::new(content);
    let addr = server.local_addr().expect("addr");
    for slot in 0..4 {
        server.clients[slot] = Some(test_client(
            addr,
            slot as u8,
            if slot < 2 {
                moba_game::Team::Blue
            } else {
                moba_game::Team::Red
            },
        ));
    }
    server.reassign_teams();
    assert_eq!(
        server.clients[0].as_ref().unwrap().team,
        moba_game::Team::Blue
    );
    assert_eq!(
        server.clients[1].as_ref().unwrap().team,
        moba_game::Team::Red
    );
    assert_eq!(
        server.clients[2].as_ref().unwrap().team,
        moba_game::Team::Red
    );
    assert_eq!(
        server.clients[3].as_ref().unwrap().team,
        moba_game::Team::Red
    );
}

#[test]
fn event_buffers_reuse_capacity_between_snapshots() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    server.phase = Phase::Playing;
    let addr = server.local_addr().expect("addr");
    server.clients[0] = Some(test_client(addr, 0, moba_game::Team::Blue));
    let event = RawEvent {
        kind: EVENT_CAST,
        team: moba_game::Team::Blue,
        source: None,
        target: None,
        source_slot: 0,
        target_slot: 0xFF,
        position: moba_core::FixVec2::ZERO,
        end: moba_core::FixVec2::ZERO,
        direction: moba_core::FixVec2::ZERO,
        value: 0,
        aux: 0,
        arc: moba_core::Fix::ZERO,
        tick: 0,
    };
    server.pending_events.push_back(event);
    server.send_snapshots();
    let events_capacity = server.delta.events.capacity();
    server.pending_events.push_back(event);
    server.send_snapshots();
    assert_eq!(server.delta.events.capacity(), events_capacity);
}

#[test]
fn match_flow_transitions_and_resets() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(2)
        .with_results_ticks(3)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    assert_eq!(server.phase(), (MatchPhase::Lobby, 0));

    // A connecting client would start the countdown; drive the same transition directly.
    server.phase = Phase::Countdown(2);
    server.tick().expect("countdown");
    assert_eq!(server.phase(), (MatchPhase::Countdown, 1));
    server.tick().expect("playing");
    assert_eq!(server.phase(), (MatchPhase::Playing, 0));

    let nexus = server
        .simulation
        .world()
        .entities()
        .find(|entity| {
            server.simulation.world().get::<UnitKind>(*entity) == Some(&UnitKind::Nexus)
                && server.simulation.world().get::<moba_game::Team>(*entity)
                    == Some(&moba_game::Team::Red)
        })
        .expect("red nexus");
    server
        .simulation
        .world_mut()
        .get_mut::<Health>(nexus)
        .expect("nexus health")
        .current = 0;
    server.tick().expect("result");
    assert_eq!(server.phase(), (MatchPhase::Results, 3));
    assert_eq!(
        server.simulation().outcome(),
        Some(MatchOutcome::Winner(moba_game::Team::Blue))
    );

    for _ in 0..3 {
        server.tick().expect("results timer");
    }
    assert_eq!(server.phase(), (MatchPhase::Lobby, 0));
    assert_eq!(server.match_number(), 1);
    assert_eq!(server.simulation().outcome(), None);
}

#[test]
fn drawn_match_reaches_results_and_reports_a_draw() {
    let path = std::env::temp_dir().join(format!(
        "moba-results-draw-{}-{}.jsonl",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(3)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(4)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    server.set_results_log(path.clone());
    server.phase = Phase::Playing;
    let nexuses: Vec<_> = server
        .simulation
        .world()
        .entities()
        .filter(|entity| {
            server.simulation.world().get::<UnitKind>(*entity) == Some(&UnitKind::Nexus)
        })
        .collect();
    assert_eq!(nexuses.len(), 2);
    for nexus in nexuses {
        server
            .simulation
            .world_mut()
            .get_mut::<Health>(nexus)
            .expect("nexus health")
            .current = 0;
    }
    server.tick().expect("result");
    assert_eq!(server.phase(), (MatchPhase::Results, 3));
    assert_eq!(server.simulation().outcome(), Some(MatchOutcome::Draw));
    assert_eq!(
        super::snapshot::outcome_code(server.simulation().outcome()),
        OUTCOME_DRAW
    );
    let ended = super::events::RawEvent::from_sim(
        &moba_sim::SimEvent::MatchEnded { winner: None },
        &server.simulation,
    )
    .expect("match end is sent");
    assert_eq!(
        (ended.kind, ended.value),
        (EVENT_MATCH_ENDED, MATCH_ENDED_DRAW)
    );
    let text = std::fs::read_to_string(&path).expect("results posted");
    assert!(text.contains("\"outcome\":\"draw\""), "{text}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn outcome_codes_are_distinct() {
    use super::snapshot::outcome_code;
    use moba_game::Team;
    assert_eq!(outcome_code(None), OUTCOME_NONE);
    assert_eq!(
        outcome_code(Some(MatchOutcome::Winner(Team::Blue))),
        OUTCOME_BLUE
    );
    assert_eq!(
        outcome_code(Some(MatchOutcome::Winner(Team::Red))),
        OUTCOME_RED
    );
    assert_eq!(outcome_code(Some(MatchOutcome::Draw)), OUTCOME_DRAW);
}

#[test]
fn content_reloads_only_with_an_empty_lobby() {
    let path = std::env::temp_dir().join(format!(
        "moba-reload-test-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    std::fs::write(
        &path,
        r#"[[items]]
name = "First"
cost = 1
"#,
    )
    .expect("seed content");
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(4)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    server.watch_content(path.clone());
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(
        &path,
        r#"[[items]]
name = "Reloaded"
cost = 1
"#,
    )
    .expect("rewrite content");
    for _ in 0..30 {
        server.tick().expect("tick");
    }
    assert_eq!(server.simulation().content().items[0].name, "Reloaded");
    assert_eq!(
        server.simulation().content().hash(),
        moba_game::load_content_file(&path).expect("load").hash()
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn content_reload_is_deferred_while_playing() {
    let path = std::env::temp_dir().join(format!(
        "moba-reload-playing-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    std::fs::write(&path, "[[items]]\nname = \"Live\"\ncost = 1\n").expect("seed");
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(4)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let content = moba_game::load_content_file(&path).expect("load seeded content");
    let mut server = NetServer::bind_with_content(config, Arc::new(content)).expect("bind");
    server.watch_content(path.clone());
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&path, "[[items]]\nname = \"Deferred\"\ncost = 1\n").expect("rewrite");
    server.phase = Phase::Playing;
    for _ in 0..3 {
        server.tick().expect("tick");
    }
    assert_eq!(server.simulation().content().items[0].name, "Live");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn rate_limiter_enforces_burst_and_refill() {
    let addr: SocketAddr = "127.0.0.1:4000".parse().expect("addr");
    let other: SocketAddr = "127.0.0.1:4001".parse().expect("addr");
    let start = Instant::now();
    let mut limiter = RateLimiter::new(10, 0, 8);
    for _ in 0..10 {
        assert!(limiter.allow_at(addr, start));
    }
    assert!(!limiter.allow_at(addr, start));
    assert!(limiter.allow_at(other, start));
    assert!(limiter.allow_at(addr, start + Duration::from_millis(100)));
    assert!(!limiter.allow_at(addr, start + Duration::from_millis(150)));
}

#[test]
fn hello_rejects_are_never_sealed_with_the_address_owners_key() {
    let mut server =
        NetServer::bind(ServerConfig::default().with_bind("127.0.0.1:0".parse().expect("address")))
            .expect("bind");
    let victim = std::net::UdpSocket::bind("127.0.0.1:0").expect("victim socket");
    victim
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("timeout");
    let victim_addr = victim.local_addr().expect("victim addr");
    // The victim has an established, keyed session with the server.
    server
        .transport
        .set_peer_key(victim_addr, SessionCrypto::new([9; 32], 0x1234, 1));
    // A spoofed Hello from the victim's address is rejected...
    server.reject_client(victim_addr, 0, 1).expect("reject");
    let mut buffer = [0u8; 256];
    let (length, _) = victim.recv_from(&mut buffer).expect("reject datagram");
    let (header, _) = crate::protocol::decode_packet(&buffer[..length]).expect("packet");
    assert_eq!(header.kind, PacketKind::Disconnect);
    // ...in plaintext, which the victim's keyed transport drops.
    assert_eq!(header.flags & crate::protocol::FLAG_ENCRYPTED, 0);
}

#[test]
fn client_challenges_are_address_bound_and_expire() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(4)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    let first: SocketAddr = "127.0.0.1:6100".parse().expect("addr");
    let second: SocketAddr = "127.0.0.1:6101".parse().expect("addr");
    let nonce = 0x1234_5678;
    server.client_challenges.push(ClientChallenge {
        addr: first,
        nonce,
        seen: Instant::now(),
    });
    assert!(server.challenge_is_pending(first, nonce));
    assert!(!server.challenge_is_pending(second, nonce));
    assert!(!server.take_challenge(second, nonce));
    assert!(server.take_challenge(first, nonce));
    assert!(!server.challenge_is_pending(first, nonce));

    server.client_challenges.push(ClientChallenge {
        addr: first,
        nonce,
        seen: Instant::now()
            .checked_sub(Duration::from_secs(11))
            .expect("test clock"),
    });
    assert!(!server.challenge_is_pending(first, nonce));
}

#[test]
fn run_until_observes_shutdown_flag() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(4)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    let shutdown = std::sync::atomic::AtomicBool::new(true);
    server
        .run_until(Duration::from_secs(10), &shutdown)
        .expect("shutdown loop");
    assert_eq!(server.stats().ticks, 0);
}

#[test]
fn results_log_appends_once_per_match() {
    let path = std::env::temp_dir().join(format!(
        "moba-results-{}-{}.jsonl",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(4)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    server.set_results_log(path.clone());
    server.write_result_log();
    server.write_result_log();
    let text = std::fs::read_to_string(&path).expect("results posted");
    assert_eq!(text.lines().count(), 1);
    assert!(text.contains("\"match\":1"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn result_json_is_valid_and_escaped() {
    let line = result_json(
        2,
        99,
        4_500,
        "blue",
        7,
        3,
        "A\"B",
        "1.0",
        0x1234,
        0x5678,
        Some(Path::new("replay\\one.mbr")),
    );
    assert!(line.starts_with("{\"match\":2,\"seed\":99,\"ticks\":4500"));
    assert!(line.contains("\"outcome\":\"blue\""));
    assert!(line.contains("\"blue_score\":7,\"red_score\":3"));
    assert!(line.contains("\"content\":\"A\\\"B\""));
    assert!(line.contains("\"content_hash\":\"0x0000000000001234\""));
    assert!(line.contains("\"state_hash\":\"0x0000000000005678\""));
    assert!(line.contains("\"replay\":\"replay\\\\one.mbr\""));
}

#[test]
fn handshake_nonce_replay_is_address_bound() {
    let key = [9u8; 32];
    let first: SocketAddr = "127.0.0.1:5000".parse().expect("addr");
    let second: SocketAddr = "127.0.0.1:5001".parse().expect("addr");
    let start = Instant::now();
    let mut tracker = NonceTracker::new(Duration::from_secs(10), 8);
    assert!(tracker.allow(key, 7, first, start));
    assert!(tracker.allow(key, 7, first, start));
    assert!(!tracker.allow(key, 7, second, start));
    assert!(tracker.allow(key, 8, second, start));
    assert!(tracker.allow(key, 7, second, start + Duration::from_secs(11)));
}

#[test]
fn global_budget_caps_distributed_floods() {
    let start = Instant::now();
    let mut limiter = RateLimiter::new(100, 5, 64);
    let mut allowed = 0;
    for index in 0..20u16 {
        let addr: SocketAddr = format!("127.0.0.1:{}", 5000 + index).parse().expect("addr");
        if limiter.allow_at(addr, start) {
            allowed += 1;
        }
    }
    assert!(allowed <= 5, "global burst caps distinct sources");
}

#[test]
fn recorded_bot_ticks_replay_deterministically() {
    let replay_path = std::env::temp_dir().join(format!(
        "moba-replay-test-{}-{}.mbr",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_millis(100))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(Some(replay_path.clone()))
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    server.phase = Phase::Playing;
    for _ in 0..60 {
        server.tick().expect("tick");
    }
    server.finish_recording();
    let bytes = std::fs::read(&replay_path).expect("replay written");
    let replay = moba_sim::Replay::decode(&bytes).expect("replay decodes");
    let result = replay.play().expect("replay verifies");
    assert_eq!(result.ticks, 60);
    assert_eq!(result.state_hash, server.simulation().state_hash());
    let _ = std::fs::remove_file(&replay_path);
}

fn observed(entity: moba_ecs::Entity, team: moba_game::Team) -> moba_sim::ObservedUnit {
    let mut unit =
        moba_sim::ObservedUnit::new(entity, UnitKind::Hero, team, moba_core::FixVec2::ZERO);
    unit.health = 100;
    unit.max_health = 100;
    unit.hero_pick = 0;
    unit.level = 1;
    unit
}

#[test]
fn events_with_hidden_endpoints_are_filtered() {
    let source = moba_ecs::Entity::from_parts(9, 1);
    let target = moba_ecs::Entity::from_parts(10, 1);
    let event = RawEvent {
        kind: EVENT_PROJECTILE_HIT,
        team: moba_game::Team::Blue,
        source: Some(source),
        target: Some(target),
        source_slot: 0,
        target_slot: 0,
        position: moba_core::FixVec2::ZERO,
        end: moba_core::FixVec2::ZERO,
        direction: moba_core::FixVec2::ZERO,
        value: 0,
        aux: 0,
        arc: moba_core::Fix::ZERO,
        tick: 0,
    };
    let blue = [observed(source, moba_game::Team::Blue)];
    assert!(!event.visible_to(moba_game::Team::Blue, false, &blue, |_| true));
    let both = [
        observed(source, moba_game::Team::Blue),
        observed(target, moba_game::Team::Red),
    ];
    assert!(event.visible_to(moba_game::Team::Blue, false, &both, |_| false));
    assert!(event.visible_to(moba_game::Team::Blue, true, &blue, |_| false));
}

#[test]
fn slots_map_to_teams_for_every_mode_size() {
    for team_size in 1..=5 {
        for slot in 0..10 {
            let expected = if slot < team_size {
                moba_game::Team::Blue
            } else {
                moba_game::Team::Red
            };
            assert_eq!(slot_team(slot, team_size), expected);
        }
    }
}

#[test]
fn connected_clients_receive_rotation_updates() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    let addr = server.local_addr().expect("addr");
    server.clients[0] = Some(test_client(addr, 0, moba_game::Team::Blue));
    server.watch_rotation(vec![
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/1v1-mid.toml"),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/map-pack.toml"),
    ]);
    server.reset_match();
    assert!(server.clients[0].as_ref().expect("client").connected);
    assert!(server.content_update_retry.is_some());
    assert!(server.content_generation > 0);
    assert_eq!(
        server.content.map.name,
        server.simulation.content().map.name
    );
}

#[test]
fn content_ack_clears_matching_retry_only() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    let addr = server.local_addr().expect("addr");
    server.clients[0] = Some(test_client(addr, 0, moba_game::Team::Blue));
    let session = server.clients[0].as_ref().expect("client").session;
    server.content_generation = 5;
    server.content_update_retry = Some((Vec::new(), 0, 3, 5));
    let stale = ReceivedMessage {
        addr,
        header: Header {
            kind: PacketKind::ContentAck,
            flags: 0,
            session,
            seq: 4,
            ack: 0,
            ack_bits: 0,
        },
        payload: Vec::new(),
    };
    server.handle_message(stale).expect("stale ack");
    assert!(server.content_update_retry.is_some());
    let ack = ReceivedMessage {
        addr,
        header: Header {
            kind: PacketKind::ContentAck,
            flags: 0,
            session,
            seq: 5,
            ack: 0,
            ack_bits: 0,
        },
        payload: Vec::new(),
    };
    server.handle_message(ack).expect("ack");
    assert!(server.content_update_retry.is_none());
}

#[test]
fn content_update_retry_clears_on_match_reset() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    server.broadcast_content_update();
    assert!(server.content_update_retry.is_some());
    let generation = server.content_generation;
    server.reset_match();
    assert!(server.content_update_retry.is_none());
    assert_eq!(server.content_generation, generation);
}

fn test_client(addr: SocketAddr, slot: u8, team: moba_game::Team) -> ClientConn {
    ClientConn {
        addr,
        session: 7,
        slot,
        key: None,
        identity: None,
        spectator: false,
        team,
        connected: true,
        ready: false,
        last_seen: Instant::now(),
        disconnected_at: None,
        last_input_seq: 4,
        command: None,
        pending_purchase: None,
        net_ids: NetIdTable::default(),
        snapshot_seq: 3,
        acked_snapshot: 2,
        need_full: false,
        history: VecDeque::new(),
        history_pool: Vec::new(),
        welcome_nonce: 0,
        welcome_payload: Vec::new(),
        bytes_sent: 0,
    }
}

#[test]
fn broadcast_resets_and_retries_for_a_connected_client() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    let addr = server.local_addr().expect("addr");
    server.clients[0] = Some(test_client(addr, 0, moba_game::Team::Blue));
    server.broadcast_content_update();
    let client = server.clients[0].as_ref().expect("client");
    assert!(client.need_full);
    assert_eq!(client.snapshot_seq, 3);
    assert_eq!(client.acked_snapshot, 0);
    assert_eq!(client.last_input_seq, 0);
    assert!(server.content_update_retry.is_some());
    assert_eq!(server.content_generation, 1);
}

struct CountingPolicy(Arc<std::sync::atomic::AtomicUsize>);

impl moba_sim::BotPolicy for CountingPolicy {
    fn inputs(&mut self, _simulation: &mut Simulation) -> TickInputs {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        TickInputs::new()
    }
}

#[test]
fn bot_policy_is_reapplied_after_rematch() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let policy_calls = Arc::clone(&calls);
    let mut server = NetServer::bind(config)
        .expect("bind")
        .with_bot_policy(move || Box::new(CountingPolicy(Arc::clone(&policy_calls))));
    server.phase = Phase::Playing;
    server.tick().expect("tick");
    let before = calls.load(std::sync::atomic::Ordering::Relaxed);
    assert!(before > 0);
    server.reset_match();
    server.phase = Phase::Playing;
    server.tick().expect("tick");
    assert!(calls.load(std::sync::atomic::Ordering::Relaxed) > before);
}

#[test]
fn named_bot_policies_can_be_registered_and_selected() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let policy_calls = Arc::clone(&calls);
    let mut server = NetServer::bind(config).expect("bind");
    assert!(server.register_bot_policy("count", move || {
        Box::new(CountingPolicy(Arc::clone(&policy_calls)))
    }));
    assert!(!server.register_bot_policy("count", || {
        Box::new(CountingPolicy(Arc::new(
            std::sync::atomic::AtomicUsize::new(0),
        )))
    }));
    assert!(server.select_bot_policy("count"));
    assert!(!server.select_bot_policy("missing"));
    server.phase = Phase::Playing;
    server.tick().expect("tick");
    assert!(calls.load(std::sync::atomic::Ordering::Relaxed) > 0);
}

#[test]
fn snapshot_send_allocations_stay_bounded_with_events() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default())
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    server.phase = Phase::Playing;
    let addr = server.local_addr().expect("addr");
    server.clients[0] = Some(test_client(addr, 0, moba_game::Team::Blue));
    let event = RawEvent {
        kind: EVENT_CAST,
        team: moba_game::Team::Blue,
        source: None,
        target: None,
        source_slot: 0,
        target_slot: 0xFF,
        position: moba_core::FixVec2::ZERO,
        end: moba_core::FixVec2::ZERO,
        direction: moba_core::FixVec2::ZERO,
        value: 0,
        aux: 0,
        arc: moba_core::Fix::ZERO,
        tick: 0,
    };
    // Attacks fill their own queue to its bound every snapshot, hero and minion alike.
    let attack = |index: u32| RawEvent {
        kind: EVENT_ATTACK,
        source: Some(moba_ecs::Entity::from_parts(100 + index, 1)),
        target: Some(moba_ecs::Entity::from_parts(200 + index, 1)),
        source_slot: if index.is_multiple_of(3) { 0 } else { 0xFF },
        value: 30,
        ..event
    };
    let fill = |server: &mut NetServer| {
        server.pending_events.push_back(event);
        for index in 0..u32::try_from(MAX_SNAPSHOT_EVENTS * 5).expect("small") {
            super::match_flow::push_attack(&mut server.pending_attacks, attack(index));
        }
        server.send_snapshots();
    };
    for _ in 0..200 {
        fill(&mut server);
    }
    moba_test_support::reset();
    for _ in 0..100 {
        fill(&mut server);
    }
    let (allocations, bytes) = moba_test_support::counters();
    eprintln!("snapshot send: {allocations} allocations, {bytes} bytes");
    assert!(
        allocations <= 5,
        "snapshot send allocated {allocations} times"
    );
}

#[test]
fn shared_reload_retries_until_occupied_slots_fit() {
    let dir = std::env::temp_dir().join(format!(
        "moba-shared-retry-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("pack.toml");
    std::fs::write(&path, "[[items]]\nname = \"First\"\ncost = 1\n").expect("seed pack");
    let mut server =
        NetServer::bind(ServerConfig::default().with_bind("127.0.0.1:0".parse().expect("address")))
            .expect("bind");
    let watch = Arc::new(crate::host::ContentWatch::new(path.clone(), false, None));
    server.follow_content(Arc::clone(&watch));
    let addr = server.local_addr().expect("addr");
    // A player in slot 3 cannot be served by a 1v1 pack.
    server.clients[3] = Some(test_client(addr, 3, moba_game::Team::Red));
    std::thread::sleep(Duration::from_millis(20));
    let staging = dir.join("pack.tmp");
    let one_v_one = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/1v1-mid.toml");
    std::fs::copy(&one_v_one, &staging).expect("stage pack");
    std::fs::rename(&staging, &path).expect("atomic replace");
    assert!(watch.poll());
    for _ in 0..30 {
        server.tick().expect("tick");
    }
    assert_eq!(
        server.content.rules.team_size, 5,
        "rejected while slot 3 is taken"
    );
    server.clients[3] = None;
    for _ in 0..30 {
        server.tick().expect("tick");
    }
    assert_eq!(
        server.content.rules.team_size, 1,
        "retried once the slot freed"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn basic_attacks_convert_to_attack_events_with_slots_and_interval() {
    let mut sim = moba_sim::Simulation::new(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false),
    );
    let blue = sim.hero_entity(0).expect("blue hero");
    let red = sim.hero_entity(5).expect("red hero");
    let at = |x| moba_core::FixVec2::from_ints(x, 100);
    sim.world_mut()
        .get_mut::<moba_sim::Position>(blue)
        .expect("position")
        .0 = at(100);
    sim.world_mut()
        .get_mut::<moba_sim::Position>(red)
        .expect("position")
        .0 = at(101);
    let interval = sim
        .world()
        .get::<moba_sim::AttackStats>(blue)
        .expect("attack stats")
        .interval;
    let mut inputs = moba_sim::TickInputs::new();
    inputs.set(0, moba_sim::PlayerCommand::Attack(red));
    sim.tick(&inputs);
    let raw = sim
        .events()
        .iter()
        .filter_map(|event| RawEvent::from_sim(event, &sim))
        .find(|raw| raw.kind == EVENT_ATTACK && raw.source == Some(blue))
        .expect("the hit is replicated");
    assert_eq!(raw.target, Some(red));
    assert_eq!((raw.source_slot, raw.target_slot), (0, 5));
    assert_eq!(u32::from(raw.value), interval);
    assert_eq!(raw.priority(), 2, "a hero's attack");
    // The damage is visible from the state after the hit's tick.
    assert_eq!(raw.tick, sim.current_tick().get());
    assert_eq!(raw.tick, 1);
}

#[test]
fn attacks_have_the_lowest_snapshot_priority_and_carry_their_age() {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default().with_spawn_waves(false))
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    server.phase = Phase::Playing;
    let addr = server.local_addr().expect("addr");
    let mut viewer = test_client(addr, 0, moba_game::Team::Blue);
    // A spectator sees every event, so only priority decides what is sent.
    viewer.spectator = true;
    server.clients[0] = Some(viewer);
    for _ in 0..3 {
        server.simulation.tick(&moba_sim::TickInputs::new());
    }
    let snapshot_tick = server.simulation.current_tick().get();
    let entity = |index| Some(moba_ecs::Entity::from_parts(index, 1));
    let base = RawEvent {
        kind: EVENT_ATTACK,
        team: moba_game::Team::Red,
        source: entity(40),
        target: entity(41),
        source_slot: 0xFF,
        target_slot: 0xFF,
        position: moba_core::FixVec2::ZERO,
        end: moba_core::FixVec2::ZERO,
        direction: moba_core::FixVec2::ZERO,
        value: 30,
        aux: 0,
        arc: moba_core::Fix::ZERO,
        tick: snapshot_tick - 2,
    };
    // A teamfight's worth of minion attacks, then a hero's, then two deaths and a cast.
    for index in 0..MAX_SNAPSHOT_EVENTS {
        let mut attack = base;
        attack.source = entity(100 + index as u32);
        server.pending_attacks.push_back(attack);
    }
    let hero_attack = RawEvent {
        source: entity(3),
        source_slot: 6,
        ..base
    };
    server.pending_attacks.push_back(hero_attack);
    for kind in [EVENT_UNIT_DIED, EVENT_UNIT_DIED, EVENT_CAST] {
        server.pending_events.push_back(RawEvent {
            kind,
            source: entity(3),
            target: entity(41),
            source_slot: 6,
            ..base
        });
    }
    server.send_snapshots();
    let events = &server.delta.events;
    assert_eq!(events.len(), MAX_SNAPSHOT_EVENTS);
    let kinds: Vec<u8> = events.iter().map(|event| event.kind).collect();
    assert_eq!(
        &kinds[..4],
        &[EVENT_UNIT_DIED, EVENT_UNIT_DIED, EVENT_CAST, EVENT_ATTACK]
    );
    // The hero's attack goes before every minion attack; the minion attacks that do not fit
    // are dropped.
    assert_eq!(events[3].source_slot, 6);
    assert!(
        events[4..]
            .iter()
            .all(|event| event.kind == EVENT_ATTACK && event.source_slot == 0xFF)
    );
    // Compact attack form: its age, the interval, and no position.
    assert_eq!((events[3].aux, events[3].value), (2, 30));
    assert_eq!(
        (events[3].x, events[3].y, events[3].dx, events[3].arc),
        (0, 0, 0, 0)
    );
    assert!(server.pending_attacks.is_empty() && server.pending_events.is_empty());
}

/// A server with one blue player in slot 0 (not a bot), playing.
fn playing_server(spawn_waves: bool) -> (NetServer, SocketAddr) {
    let config = ServerConfig::default()
        .with_bind("127.0.0.1:0".parse().expect("address"))
        .with_sim(SimConfig::default().with_spawn_waves(spawn_waves))
        .with_snapshot_interval(1)
        .with_max_clients(10)
        .with_countdown_ticks(0)
        .with_results_ticks(10)
        .with_client_timeout(Duration::from_secs(3))
        .with_reconnect_grace(Duration::from_secs(2))
        .with_pre_shared_key(None)
        .with_record(None)
        .with_max_spectators(8)
        .with_max_packets_per_second(120)
        .with_max_total_packets_per_second(2_000)
        .with_require_ready(false);
    let mut server = NetServer::bind(config).expect("bind");
    server.phase = Phase::Playing;
    let addr = server.local_addr().expect("addr");
    server.clients[0] = Some(test_client(addr, 0, moba_game::Team::Blue));
    server.refresh_bot_mask();
    (server, addr)
}

/// Blue hero 0 lands a killing blow on `victim` (standing next to it, on 1 health) and
/// returns the events of the next snapshot.
fn killing_blow(
    server: &mut NetServer,
    victim: moba_ecs::Entity,
) -> Vec<crate::protocol::NetEvent> {
    let blue = server.simulation.hero_entity(0).expect("blue hero");
    let at = server
        .simulation
        .world()
        .get::<moba_sim::Position>(victim)
        .expect("victim position")
        .0;
    server
        .simulation
        .world_mut()
        .get_mut::<moba_sim::Position>(blue)
        .expect("position")
        .0 = at + moba_core::FixVec2::from_ints(1, 0);
    server
        .simulation
        .world_mut()
        .get_mut::<Health>(victim)
        .expect("health")
        .current = 1;
    // The victim is in this snapshot ...
    server.send_snapshots();
    let client = server.clients[0].as_mut().expect("client");
    let victim_id = client.net_id_for(victim);
    assert!(
        server
            .delta
            .spawns
            .iter()
            .any(|entity| entity.net_id == victim_id),
        "the victim is visible before the blow"
    );
    let client = server.clients[0].as_mut().expect("client");
    client.command = Some(crate::protocol::WireCommand::Attack { net_id: victim_id });
    // ... and gone (despawned, or a dead enemy hero out of vision) in the next one.
    server.tick_simulation();
    server.send_snapshots();
    server.delta.events.clone()
}

#[test]
fn a_last_hit_on_a_minion_reaches_the_killer() {
    let (mut server, _) = playing_server(true);
    let minion = loop {
        server.simulation.tick(&moba_sim::TickInputs::new());
        let world = server.simulation.world();
        if let Some(minion) = world.entities().find(|entity| {
            world.get::<UnitKind>(*entity) == Some(&UnitKind::MeleeMinion)
                && world.get::<moba_game::Team>(*entity) == Some(&moba_game::Team::Red)
        }) {
            break minion;
        }
        assert!(
            server.simulation.current_tick().get() < 200,
            "a wave spawns"
        );
    };
    let events = killing_blow(&mut server, minion);
    let client = server.clients[0].as_ref().expect("client");
    let (blue, victim) = (
        client
            .net_ids
            .get(server.simulation.hero_entity(0).expect("hero")),
        client.net_ids.get(minion),
    );
    assert!(
        !server
            .simulation
            .world()
            .entities()
            .any(|entity| entity == minion)
    );
    let attack = events
        .iter()
        .find(|event| event.kind == EVENT_ATTACK && Some(event.source) == blue)
        .expect("the killing blow is sent to the killer");
    assert_eq!(Some(attack.target), victim);
    assert!(
        events
            .iter()
            .any(|event| event.kind == EVENT_UNIT_DIED && Some(event.target) == victim),
        "the minion's death is sent: {events:?}"
    );
}

#[test]
fn killing_an_enemy_hero_reaches_the_killer() {
    let (mut server, _) = playing_server(false);
    let red = server.simulation.hero_entity(5).expect("red hero");
    let events = killing_blow(&mut server, red);
    assert!(server.simulation.world().has::<moba_sim::Dead>(red));
    let client = server.clients[0].as_ref().expect("client");
    let (blue, victim) = (
        client
            .net_ids
            .get(server.simulation.hero_entity(0).expect("hero")),
        client.net_ids.get(red),
    );
    let attack = events
        .iter()
        .find(|event| event.kind == EVENT_ATTACK && Some(event.source) == blue)
        .expect("the killing blow is sent to the killer");
    assert_eq!((Some(attack.target), attack.target_slot), (victim, 5));
    assert!(
        events
            .iter()
            .any(|event| event.kind == EVENT_UNIT_DIED && Some(event.target) == victim),
        "the hero's death is sent: {events:?}"
    );
    // A client that never saw the victim gets neither.
    let mut stranger = test_client(server.local_addr().expect("addr"), 0, moba_game::Team::Blue);
    stranger.net_ids = NetIdTable::default();
    assert!(!stranger.knew(red));
}

#[test]
fn a_full_attack_queue_evicts_minion_attacks_before_hero_attacks() {
    let mut queue = VecDeque::with_capacity(MAX_SNAPSHOT_EVENTS * 4);
    let attack = |index: u32, hero: bool| RawEvent {
        kind: EVENT_ATTACK,
        team: moba_game::Team::Blue,
        source: Some(moba_ecs::Entity::from_parts(index, 1)),
        target: None,
        source_slot: if hero { 0 } else { 0xFF },
        target_slot: 0xFF,
        position: moba_core::FixVec2::ZERO,
        end: moba_core::FixVec2::ZERO,
        direction: moba_core::FixVec2::ZERO,
        value: 30,
        aux: 0,
        arc: moba_core::Fix::ZERO,
        tick: index,
    };
    // A hero attack first, then minion attacks up to the bound.
    super::match_flow::push_attack(&mut queue, attack(0, true));
    for index in 1..u32::try_from(MAX_SNAPSHOT_EVENTS * 4).expect("small") {
        super::match_flow::push_attack(&mut queue, attack(index, false));
    }
    assert_eq!(queue.len(), MAX_SNAPSHOT_EVENTS * 4);
    // Another hero attack evicts the oldest minion attack, not the older hero attack.
    super::match_flow::push_attack(&mut queue, attack(1_000, true));
    assert_eq!(queue.len(), MAX_SNAPSHOT_EVENTS * 4);
    assert_eq!(queue.front().map(|event| event.tick), Some(0));
    assert!(!queue.iter().any(|event| event.tick == 1));
    // Once only hero attacks are queued, a minion attack is dropped and a hero attack
    // evicts the oldest.
    let mut heroes: VecDeque<RawEvent> = (0..u32::try_from(MAX_SNAPSHOT_EVENTS * 4)
        .expect("small"))
        .map(|index| attack(index, true))
        .collect();
    super::match_flow::push_attack(&mut heroes, attack(2_000, false));
    assert!(heroes.iter().all(|event| event.priority() == 2));
    super::match_flow::push_attack(&mut heroes, attack(3_000, true));
    assert_eq!(heroes.front().map(|event| event.tick), Some(1));
    assert_eq!(heroes.back().map(|event| event.tick), Some(3_000));
}
