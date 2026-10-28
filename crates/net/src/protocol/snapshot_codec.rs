//! Snapshot payload encoding and decoding.

use super::WireError;
use super::bytes::{Reader, count_u16, put_u16, put_u32};
use super::snapshot::{
    EVENT_ATTACK, MAX_SNAPSHOT_EVENTS, NetEntity, NetEvent, NetProgress, NetUpdate, SnapshotDelta,
};

/// Minimum encoded bytes for one spawn record.
const MIN_SPAWN_BYTES: usize = 36;
/// Minimum encoded bytes for one update record (`net_id` + mask).
const MIN_UPDATE_BYTES: usize = 3;
/// Minimum encoded bytes for one despawn record.
const MIN_DESPAWN_BYTES: usize = 2;

const UPDATE_POSITION: u8 = 1 << 0;
const UPDATE_HEALTH: u8 = 1 << 1;
const UPDATE_MANA: u8 = 1 << 2;
const UPDATE_FLAGS: u8 = 1 << 3;
const UPDATE_COOLDOWNS: u8 = 1 << 4;
const UPDATE_PROGRESS: u8 = 1 << 5;
const UPDATE_STATUS_BITS: u8 = 1 << 6;

/// Encodes a snapshot payload.
///
/// # Errors
/// Returns [`WireError::TooMany`] when a count exceeds `u16::MAX`.
#[allow(clippy::too_many_lines)] // Field order mirrors the decoder one-for-one.
pub fn encode_snapshot(delta: &SnapshotDelta, out: &mut Vec<u8>) -> Result<(), WireError> {
    out.clear();
    out.reserve(40 + delta.spawns.len() * 36 + delta.updates.len() * 12 + delta.despawns.len() * 2);
    put_u32(out, delta.tick);
    put_u32(out, delta.acked_input);
    put_u32(out, delta.baseline_seq);
    put_u32(out, delta.blue_score);
    put_u32(out, delta.red_score);
    out.push(delta.outcome);
    out.push(delta.phase);
    put_u16(out, delta.countdown);
    put_u16(out, delta.ready_mask);
    put_u16(out, delta.player_mask);
    put_u16(out, count_u16(delta.spawns.len())?);
    put_u16(out, count_u16(delta.updates.len())?);
    put_u16(out, count_u16(delta.despawns.len())?);
    for spawn in &delta.spawns {
        put_u16(out, spawn.net_id);
        out.push(spawn.kind);
        out.push(spawn.team);
        out.push(spawn.hero_slot);
        out.push(spawn.hero_pick);
        out.push(spawn.flags);
        put_u16(out, spawn.status_bits);
        put_u16(out, spawn.max_health);
        put_u16(out, spawn.max_mana);
        put_u16(out, spawn.x);
        put_u16(out, spawn.y);
        put_u16(out, spawn.health);
        put_u16(out, spawn.mana);
        for cooldown in spawn.cooldowns {
            put_u16(out, cooldown);
        }
        out.push(spawn.progress.level);
        put_u16(out, spawn.progress.xp);
        put_u16(out, spawn.progress.gold);
        out.extend_from_slice(&spawn.progress.items);
        for cooldown in spawn.progress.item_cooldowns {
            put_u16(out, cooldown);
        }
    }
    for update in &delta.updates {
        put_u16(out, update.net_id);
        let mut mask = 0u8;
        if update.position.is_some() {
            mask |= UPDATE_POSITION;
        }
        if update.health.is_some() {
            mask |= UPDATE_HEALTH;
        }
        if update.mana.is_some() {
            mask |= UPDATE_MANA;
        }
        if update.flags.is_some() {
            mask |= UPDATE_FLAGS;
        }
        if update.status_bits.is_some() {
            mask |= UPDATE_STATUS_BITS;
        }
        if update.cooldowns.is_some() {
            mask |= UPDATE_COOLDOWNS;
        }
        if update.progress.is_some() {
            mask |= UPDATE_PROGRESS;
        }
        out.push(mask);
        if let Some((x, y)) = update.position {
            put_u16(out, x);
            put_u16(out, y);
        }
        if let Some(health) = update.health {
            put_u16(out, health);
        }
        if let Some(mana) = update.mana {
            put_u16(out, mana);
        }
        if let Some(flags) = update.flags {
            out.push(flags);
        }
        if let Some(status_bits) = update.status_bits {
            put_u16(out, status_bits);
        }
        if let Some(cooldowns) = update.cooldowns {
            for cooldown in cooldowns {
                put_u16(out, cooldown);
            }
        }
        if let Some(progress) = update.progress {
            out.push(progress.level);
            put_u16(out, progress.xp);
            put_u16(out, progress.gold);
            out.extend_from_slice(&progress.items);
            for cooldown in progress.item_cooldowns {
                put_u16(out, cooldown);
            }
        }
    }
    for net_id in &delta.despawns {
        put_u16(out, *net_id);
    }
    out.push(u8::try_from(delta.events.len()).map_err(|_| WireError::TooMany)?);
    for event in &delta.events {
        out.push(event.kind);
        out.push(event.team);
        put_u16(out, event.source);
        put_u16(out, event.target);
        out.push(event.source_slot);
        out.push(event.target_slot);
        if event.kind == EVENT_ATTACK {
            // Compact: an attack has no position, direction or arc of its own (12 bytes, not 26).
            put_u16(out, event.value);
            put_u16(out, event.aux);
            continue;
        }
        put_u16(out, event.x);
        put_u16(out, event.y);
        put_u16(out, event.x2);
        put_u16(out, event.y2);
        put_u16(out, event.value);
        put_u16(out, event.aux);
        out.extend_from_slice(&event.dx.to_le_bytes());
        out.extend_from_slice(&event.dy.to_le_bytes());
        out.extend_from_slice(&event.arc.to_le_bytes());
    }
    Ok(())
}

/// Decodes a snapshot payload.
#[allow(clippy::too_many_lines)] // Field order mirrors the encoder one-for-one.
pub fn decode_snapshot(data: &[u8]) -> Result<SnapshotDelta, WireError> {
    let mut reader = Reader::new(data);
    let tick = reader.u32()?;
    let acked_input = reader.u32()?;
    let baseline_seq = reader.u32()?;
    let blue_score = reader.u32()?;
    let red_score = reader.u32()?;
    let outcome = reader.u8()?;
    let phase = reader.u8()?;
    let countdown = reader.u16()?;
    let ready_mask = reader.u16()?;
    let player_mask = reader.u16()?;
    let spawn_count = usize::from(reader.u16()?);
    let update_count = usize::from(reader.u16()?);
    let despawn_count = usize::from(reader.u16()?);
    if spawn_count > reader.remaining() / MIN_SPAWN_BYTES
        || update_count > reader.remaining() / MIN_UPDATE_BYTES
        || despawn_count > reader.remaining() / MIN_DESPAWN_BYTES
    {
        return Err(WireError::BadValue);
    }
    let mut spawns = Vec::with_capacity(spawn_count);
    for _ in 0..spawn_count {
        spawns.push(NetEntity {
            net_id: reader.u16()?,
            kind: reader.u8()?,
            team: reader.u8()?,
            hero_slot: reader.u8()?,
            hero_pick: reader.u8()?,
            flags: reader.u8()?,
            status_bits: reader.u16()?,
            max_health: reader.u16()?,
            max_mana: reader.u16()?,
            x: reader.u16()?,
            y: reader.u16()?,
            health: reader.u16()?,
            mana: reader.u16()?,
            cooldowns: [reader.u16()?, reader.u16()?, reader.u16()?],
            progress: NetProgress {
                level: reader.u8()?,
                xp: reader.u16()?,
                gold: reader.u16()?,
                items: reader
                    .bytes(6)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
                item_cooldowns: [
                    reader.u16()?,
                    reader.u16()?,
                    reader.u16()?,
                    reader.u16()?,
                    reader.u16()?,
                    reader.u16()?,
                ],
            },
        });
    }
    let mut updates = Vec::with_capacity(update_count);
    for _ in 0..update_count {
        let net_id = reader.u16()?;
        let mask = reader.u8()?;
        let known = UPDATE_POSITION
            | UPDATE_HEALTH
            | UPDATE_MANA
            | UPDATE_FLAGS
            | UPDATE_COOLDOWNS
            | UPDATE_PROGRESS
            | UPDATE_STATUS_BITS;
        if mask & !known != 0 {
            return Err(WireError::BadValue);
        }
        let position = if mask & UPDATE_POSITION != 0 {
            Some((reader.u16()?, reader.u16()?))
        } else {
            None
        };
        let health = (mask & UPDATE_HEALTH != 0)
            .then(|| reader.u16())
            .transpose()?;
        let mana = (mask & UPDATE_MANA != 0)
            .then(|| reader.u16())
            .transpose()?;
        let flags = (mask & UPDATE_FLAGS != 0)
            .then(|| reader.u8())
            .transpose()?;
        let status_bits = (mask & UPDATE_STATUS_BITS != 0)
            .then(|| reader.u16())
            .transpose()?;
        let cooldowns = (mask & UPDATE_COOLDOWNS != 0)
            .then(|| Ok::<[u16; 3], WireError>([reader.u16()?, reader.u16()?, reader.u16()?]))
            .transpose()?;
        let progress = (mask & UPDATE_PROGRESS != 0)
            .then(|| {
                Ok::<NetProgress, WireError>(NetProgress {
                    level: reader.u8()?,
                    xp: reader.u16()?,
                    gold: reader.u16()?,
                    items: reader
                        .bytes(6)?
                        .try_into()
                        .map_err(|_| WireError::Truncated)?,
                    item_cooldowns: [
                        reader.u16()?,
                        reader.u16()?,
                        reader.u16()?,
                        reader.u16()?,
                        reader.u16()?,
                        reader.u16()?,
                    ],
                })
            })
            .transpose()?;
        updates.push(NetUpdate {
            net_id,
            position,
            health,
            mana,
            flags,
            status_bits,
            cooldowns,
            progress,
        });
    }
    let mut despawns = Vec::with_capacity(despawn_count);
    for _ in 0..despawn_count {
        despawns.push(reader.u16()?);
    }
    let event_count = usize::from(reader.u8()?);
    if event_count > MAX_SNAPSHOT_EVENTS {
        return Err(WireError::BadValue);
    }
    let mut events = Vec::with_capacity(event_count);
    for _ in 0..event_count {
        let kind = reader.u8()?;
        if kind == EVENT_ATTACK {
            events.push(NetEvent {
                kind,
                team: reader.u8()?,
                source: reader.u16()?,
                target: reader.u16()?,
                source_slot: reader.u8()?,
                target_slot: reader.u8()?,
                value: reader.u16()?,
                aux: reader.u16()?,
                ..NetEvent::default()
            });
            continue;
        }
        events.push(NetEvent {
            kind,
            team: reader.u8()?,
            source: reader.u16()?,
            target: reader.u16()?,
            source_slot: reader.u8()?,
            target_slot: reader.u8()?,
            x: reader.u16()?,
            y: reader.u16()?,
            x2: reader.u16()?,
            y2: reader.u16()?,
            value: reader.u16()?,
            aux: reader.u16()?,
            dx: i16::from_le_bytes(
                reader
                    .bytes(2)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            ),
            dy: i16::from_le_bytes(
                reader
                    .bytes(2)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            ),
            arc: i16::from_le_bytes(
                reader
                    .bytes(2)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            ),
        });
    }
    if reader.remaining() != 0 {
        return Err(WireError::TrailingBytes);
    }
    Ok(SnapshotDelta {
        tick,
        acked_input,
        baseline_seq,
        blue_score,
        red_score,
        outcome,
        phase,
        countdown,
        ready_mask,
        player_mask,
        spawns,
        updates,
        despawns,
        events,
    })
}
