//! UDP transport, handshake, input messages and delta-compressed snapshots.
//!
//! Layering: this crate consumes [`moba_sim`] observations and produces wire data. The
//! simulation never depends on networking. Encryption/authentication hardening lands in
//! M7; M5 focuses on a correct, bandwidth-measured protocol.
#![forbid(unsafe_code)]

pub mod client;
pub mod crypto;
pub mod host;
pub mod interpolation;
pub mod prediction;
pub mod protocol;
pub mod server;
pub mod transport;

pub use client::{ClientEvent, ClientStats, NetClient};
pub use crypto::{CryptoError, SessionCrypto};
pub use host::{ContentWatch, HostBudget, HostReport, MatchHost, TickHistogram, WorkerStats};
pub use interpolation::Interpolator;
pub use prediction::PredictionState;
pub use protocol::{
    HEADER_LEN, Header, MAX_DATAGRAM, NetEntity, NetId, NetUpdate, PacketKind, SnapshotDelta,
    SnapshotState, Welcome, WireCommand, WireError, decode_hello, decode_packet, decode_welcome,
    encode_hello, encode_packet, encode_welcome,
};
pub use server::{NetServer, ServerConfig, ServerStats};
pub use transport::{AckTracker, ReceivedMessage, UdpTransport};
