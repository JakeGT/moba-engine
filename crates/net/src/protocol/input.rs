//! Client intent: [`WireCommand`] and [`InputMessage`] payloads.

use super::WireError;
use super::bytes::{Reader, put_u16, put_u32};
use super::snapshot::NetId;

/// A player command as it travels on the wire.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WireCommand {
    /// Clear orders.
    Stop,
    /// Move to a quantised position.
    MoveTo {
        /// Quantised X.
        x: u16,
        /// Quantised Y.
        y: u16,
    },
    /// Attack a network entity id.
    Attack {
        /// Server-assigned id for this client.
        net_id: NetId,
    },
    /// Cast an ability at a quantised position.
    CastAbility {
        /// Q/W/E slot.
        slot: u8,
        /// Quantised X.
        x: u16,
        /// Quantised Y.
        y: u16,
    },
    /// Purchase an item from the shop.
    BuyItem {
        /// Catalogue index.
        item: u8,
    },
    /// Toggle lobby readiness.
    Ready {
        /// Requested ready state.
        ready: bool,
    },
    /// Vote to end the match for the sender's team.
    Surrender,
    /// Use the active on an inventory slot.
    UseItem {
        /// Inventory slot `0..6`.
        slot: u8,
    },
}

/// Client intent payload.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InputMessage {
    /// Client's estimate of the current server tick.
    pub client_tick: u32,
    /// Monotonic input sequence.
    pub input_seq: u32,
    /// Command.
    pub command: WireCommand,
}

/// Encodes an input message payload.
#[must_use]
pub fn encode_input(message: &InputMessage) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    put_u32(&mut out, message.client_tick);
    put_u32(&mut out, message.input_seq);
    encode_command(&mut out, message.command);
    out
}

/// Decodes an input message payload.
pub fn decode_input(data: &[u8]) -> Result<InputMessage, WireError> {
    let mut reader = Reader::new(data);
    let client_tick = reader.u32()?;
    let input_seq = reader.u32()?;
    let command = decode_command(&mut reader)?;
    if reader.remaining() != 0 {
        return Err(WireError::TrailingBytes);
    }
    Ok(InputMessage {
        client_tick,
        input_seq,
        command,
    })
}

fn encode_command(out: &mut Vec<u8>, command: WireCommand) {
    match command {
        WireCommand::Stop => out.push(0),
        WireCommand::MoveTo { x, y } => {
            out.push(1);
            put_u16(out, x);
            put_u16(out, y);
        }
        WireCommand::Attack { net_id } => {
            out.push(2);
            put_u16(out, net_id);
        }
        WireCommand::CastAbility { slot, x, y } => {
            out.push(3);
            out.push(slot);
            put_u16(out, x);
            put_u16(out, y);
        }
        WireCommand::BuyItem { item } => {
            out.push(4);
            out.push(item);
        }
        WireCommand::Ready { ready } => {
            out.push(5);
            out.push(u8::from(ready));
        }
        WireCommand::Surrender => out.push(6),
        WireCommand::UseItem { slot } => {
            out.push(7);
            out.push(slot);
        }
    }
}

fn decode_command(reader: &mut Reader<'_>) -> Result<WireCommand, WireError> {
    Ok(match reader.u8()? {
        0 => WireCommand::Stop,
        1 => WireCommand::MoveTo {
            x: reader.u16()?,
            y: reader.u16()?,
        },
        2 => WireCommand::Attack {
            net_id: reader.u16()?,
        },
        3 => WireCommand::CastAbility {
            slot: reader.u8()?,
            x: reader.u16()?,
            y: reader.u16()?,
        },
        4 => WireCommand::BuyItem { item: reader.u8()? },
        5 => WireCommand::Ready {
            ready: reader.u8()? != 0,
        },
        6 => WireCommand::Surrender,
        _ => return Err(WireError::BadValue),
    })
}
