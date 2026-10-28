//! Deterministic wire-decoder fuzzer.
//!
//! Feeds random bytes and mutated valid packets into every protocol decoder. Any panic or
//! hang fails the run. Defaults to one hour; `--seconds N` is useful for CI.
//!
//! This complements `cargo-fuzz`/sanitizers (M7): it is deterministic, dependency-free and
//! exercises the same entry points with structured mutations rather than only noise.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use moba_core::Rng;
use moba_net::protocol::{
    EVENT_ATTACK, Header, Hello, InputMessage, MAX_SNAPSHOT_EVENTS, NetEntity, NetEvent,
    PacketKind, SnapshotDelta, Welcome, WireCatalog, WireCommand, decode_content_update,
    decode_hello, decode_input, decode_packet, decode_snapshot, decode_welcome,
    encode_content_update, encode_hello, encode_input, encode_packet, encode_snapshot,
    encode_welcome,
};

fn main() {
    let seconds: u64 = std::env::args()
        .skip(1)
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|pair| {
            (pair[0] == "--seconds")
                .then(|| pair[1].parse().ok())
                .flatten()
        })
        .unwrap_or(3_600);
    let mut rng = Rng::from_seed(0xF022_5EED);
    let start = Instant::now();
    let mut iterations = 0u64;
    // Build the built-in catalogue and verify it once; constructing it (or re-hashing it) per
    // iteration would dominate the run and shrink the coverage the gate actually provides.
    // Visual names (v40) are set so the decoder and the hash's trailing visuals block see
    // populated name bytes.
    let mut content = moba_game::GameContent::default();
    content.heroes[0].model = Some(String::from("sentinel"));
    content.map.visuals = Some(String::from("map-lava"));
    let catalog = WireCatalog::from(&content);
    verify_catalogue_round_trip(&catalog);
    while start.elapsed() < Duration::from_secs(seconds) {
        for _ in 0..1_000 {
            fuzz_once(&mut rng, &catalog);
            iterations += 1;
        }
    }
    eprintln!("fuzz-wire: {iterations} iterations clean in {seconds}s");
}

/// One full, valid catalogue (every hashed rules field, hero model and map visuals name) must
/// round-trip exactly and keep its content hash. Mutated bytes are only required to be
/// panic-free.
fn verify_catalogue_round_trip(catalog: &WireCatalog) {
    let content_hash = catalog.content_hash().expect("default catalogue hashes");
    let mut content_update = Vec::new();
    encode_content_update(catalog, content_hash, &mut content_update);
    match decode_content_update(&content_update) {
        Ok((decoded, decoded_hash)) => {
            assert_eq!(decoded, *catalog, "clean catalogue round trip");
            assert_eq!(decoded_hash, content_hash, "clean catalogue hash");
        }
        Err(error) => panic!("freshly encoded catalogue must decode: {error:?}"),
    }
}

fn fuzz_once(rng: &mut Rng, catalog: &WireCatalog) {
    let mut noise = vec![0u8; rng.gen_range_u32(1_400) as usize];
    rng.fill_bytes(&mut noise);
    let _ = decode_packet(&noise);
    let _ = decode_snapshot(&noise);
    let _ = decode_input(&noise);
    let _ = decode_hello(&noise);
    let _ = decode_welcome(&noise);
    let _ = decode_content_update(&noise);

    let mut snapshot = valid_snapshot(rng);
    mutate(rng, &mut snapshot);
    if let Ok(decoded) = decode_snapshot(&snapshot) {
        // Whatever decodes re-encodes to the same snapshot (compact attacks included).
        let mut payload = Vec::new();
        encode_snapshot(&decoded, &mut payload).expect("a decoded snapshot re-encodes");
        assert_eq!(decode_snapshot(&payload), Ok(decoded), "snapshot re-encode");
    }
    let mut counted = valid_snapshot(rng);
    let extreme = [u16::MAX, 0x7FFF, 0x00FF, 1, 0][rng.gen_range_u32(5) as usize];
    counted[28..30].copy_from_slice(&extreme.to_le_bytes());
    counted[30..32].copy_from_slice(&extreme.to_le_bytes());
    counted[32..34].copy_from_slice(&extreme.to_le_bytes());
    let _ = decode_snapshot(&counted);
    let mut packet = Vec::new();
    if encode_packet(&header(PacketKind::Snapshot), &snapshot, &mut packet).is_ok()
        && let Ok((_, payload)) = decode_packet(&packet)
    {
        let _ = decode_snapshot(payload);
    }

    // Keep the structured mutation path on a zeroed catalogue (cheap to encode); the Welcome
    // below carries the full built-in catalogue, so its decoder sees realistic field values.
    let mut content_update = Vec::new();
    encode_content_update(&WireCatalog::default(), rng.next_u64(), &mut content_update);
    mutate(rng, &mut content_update);
    let _ = decode_content_update(&content_update);

    let message = InputMessage {
        client_tick: rng.next_u32(),
        input_seq: rng.next_u32(),
        command: random_command(rng),
    };
    let mut input = encode_input(&message);
    mutate(rng, &mut input);
    let _ = decode_input(&input);

    let mut welcome = encode_welcome(&Welcome {
        slot: rng.gen_range_u32(10) as u8,
        tick_rate: 30,
        snapshot_interval: 1,
        tick: rng.next_u32(),
        seed: rng.next_u64(),
        hero_pick: rng.gen_range_u32(5) as u8,
        nonce: rng.next_u64(),
        server_nonce: rng.next_u64(),
        auth: [0; 32],
        spectator: rng.next_bool(),
        catalog: *catalog,
        content_hash: 0,
        identity: false,
        server_public_key: [0; 32],
        server_signature: [0; 64],
    });
    mutate(rng, &mut welcome);
    let _ = decode_welcome(&welcome);

    let mut hello = encode_hello(&Hello {
        protocol: 1,
        nonce: rng.next_u64(),
        reconnect: rng.next_u32(),
        auth: [0; 32],
        spectate: rng.next_bool(),
        identity: rng.next_bool(),
        client_public: [0; 32],
        client_signature: [0; 64],
        server_challenge: rng.next_u64(),
    });
    mutate(rng, &mut hello);
    let _ = decode_hello(&hello);
}

fn valid_snapshot(rng: &mut Rng) -> Vec<u8> {
    let count = rng.gen_range_u32(40);
    let mut delta = SnapshotDelta {
        tick: rng.next_u32(),
        acked_input: rng.next_u32(),
        baseline_seq: rng.next_u32(),
        ..SnapshotDelta::default()
    };
    for index in 0..count {
        delta.spawns.push(NetEntity {
            net_id: index as u16,
            kind: rng.gen_range_u32(5) as u8,
            team: rng.gen_range_u32(2) as u8,
            hero_slot: rng.gen_range_u32(11) as u8,
            hero_pick: rng.gen_range_u32(6) as u8,
            flags: rng.next_u32() as u8,
            status_bits: 0,
            max_health: rng.next_u32() as u16,
            max_mana: rng.next_u32() as u16,
            x: rng.next_u32() as u16,
            y: rng.next_u32() as u16,
            health: rng.next_u32() as u16,
            mana: rng.next_u32() as u16,
            cooldowns: [
                rng.next_u32() as u16,
                rng.next_u32() as u16,
                rng.next_u32() as u16,
            ],
            progress: moba_net::protocol::NetProgress {
                level: rng.gen_range_u32(16) as u8,
                xp: rng.next_u32() as u16,
                gold: rng.next_u32() as u16,
                items: [0xFF; 6],
                item_cooldowns: [0; 6],
            },
        });
    }
    if !delta.spawns.is_empty() && rng.next_bool() {
        delta
            .despawns
            .push(rng.gen_range_u32(u32::from(delta.spawns[0].net_id) + 1) as u16);
    }
    // Presentation events of every kind, the compact EVENT_ATTACK (v41) among them.
    for _ in 0..rng.gen_range_u32(MAX_SNAPSHOT_EVENTS as u32 + 1) {
        delta.events.push(random_event(rng));
    }
    let mut payload = Vec::new();
    encode_snapshot(&delta, &mut payload).expect("valid snapshot");
    // A clean snapshot round-trips exactly (only mutated bytes may fail to decode).
    match decode_snapshot(&payload) {
        Ok(decoded) => assert_eq!(decoded, delta, "clean snapshot round trip"),
        Err(error) => panic!("freshly encoded snapshot must decode: {error:?}"),
    }
    payload
}

/// A random event; an attack leaves the fields its compact form does not carry at zero.
fn random_event(rng: &mut Rng) -> NetEvent {
    let kind = if rng.next_bool() {
        EVENT_ATTACK
    } else {
        rng.gen_range_u32(10) as u8
    };
    let mut event = NetEvent {
        kind,
        team: rng.gen_range_u32(2) as u8,
        source: rng.next_u32() as u16,
        target: rng.next_u32() as u16,
        source_slot: rng.next_u32() as u8,
        target_slot: rng.next_u32() as u8,
        value: rng.next_u32() as u16,
        aux: rng.next_u32() as u16,
        ..NetEvent::default()
    };
    if kind != EVENT_ATTACK {
        event.x = rng.next_u32() as u16;
        event.y = rng.next_u32() as u16;
        event.x2 = rng.next_u32() as u16;
        event.y2 = rng.next_u32() as u16;
        event.dx = rng.next_u32() as i16;
        event.dy = rng.next_u32() as i16;
        event.arc = rng.next_u32() as i16;
    }
    event
}

fn random_command(rng: &mut Rng) -> WireCommand {
    match rng.gen_range_u32(4) {
        0 => WireCommand::Stop,
        1 => WireCommand::MoveTo {
            x: rng.next_u32() as u16,
            y: rng.next_u32() as u16,
        },
        2 => WireCommand::Attack {
            net_id: rng.next_u32() as u16,
        },
        _ => WireCommand::CastAbility {
            slot: rng.gen_range_u32(3) as u8,
            x: rng.next_u32() as u16,
            y: rng.next_u32() as u16,
        },
    }
}

fn mutate(rng: &mut Rng, data: &mut Vec<u8>) {
    if data.is_empty() {
        data.push(rng.next_u32() as u8);
    }
    let flips = rng.gen_range_u32(8) + 1;
    for _ in 0..flips {
        let index = rng.gen_range_u32(data.len() as u32) as usize;
        data[index] = data[index].wrapping_add((rng.next_u32() as u8) | 1);
    }
    if rng.chance(0.1) {
        let length = rng.gen_range_u32(data.len() as u32) as usize;
        data.truncate(length);
    }
}

fn header(kind: PacketKind) -> Header {
    Header {
        kind,
        flags: 0,
        session: 0,
        seq: 0,
        ack: 0,
        ack_bits: 0,
    }
}
