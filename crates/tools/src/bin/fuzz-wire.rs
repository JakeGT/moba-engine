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
    Header, Hello, InputMessage, NetEntity, PacketKind, SnapshotDelta, Welcome, WireCommand,
    decode_hello, decode_input, decode_packet, decode_snapshot, decode_welcome, encode_hello,
    encode_input, encode_packet, encode_snapshot, encode_welcome,
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
    while start.elapsed() < Duration::from_secs(seconds) {
        for _ in 0..1_000 {
            fuzz_once(&mut rng);
            iterations += 1;
        }
    }
    eprintln!("fuzz-wire: {iterations} iterations clean in {seconds}s");
}

fn fuzz_once(rng: &mut Rng) {
    let mut noise = vec![0u8; rng.gen_range_u32(1_400) as usize];
    rng.fill_bytes(&mut noise);
    let _ = decode_packet(&noise);
    let _ = decode_snapshot(&noise);
    let _ = decode_input(&noise);
    let _ = decode_hello(&noise);
    let _ = decode_welcome(&noise);

    let mut snapshot = valid_snapshot(rng);
    mutate(rng, &mut snapshot);
    let mut packet = Vec::new();
    if encode_packet(&header(PacketKind::Snapshot), &snapshot, &mut packet).is_ok() {
        if let Ok((_, payload)) = decode_packet(&packet) {
            let _ = decode_snapshot(payload);
        }
    }

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
        item_count: 0,
        items: [moba_net::protocol::WireItem::default(); moba_net::protocol::MAX_WIRE_ITEMS],
    });
    mutate(rng, &mut welcome);
    let _ = decode_welcome(&welcome);

    let mut hello = encode_hello(&Hello {
        protocol: 1,
        nonce: rng.next_u64(),
        reconnect: rng.next_u32(),
        auth: [0; 32],
        spectate: rng.next_bool(),
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
            },
        });
    }
    if !delta.spawns.is_empty() && rng.next_bool() {
        delta
            .despawns
            .push(rng.gen_range_u32(u32::from(delta.spawns[0].net_id) + 1) as u16);
    }
    let mut payload = Vec::new();
    encode_snapshot(&delta, &mut payload).expect("valid snapshot");
    payload
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
