//! Umbrella crate for MOBA Engine.
//!
//! The workspace is intentionally layered. Most users should start here and enable only the
//! features they need:
//!
//! ```toml
//! moba-engine = { version = "0.1", default-features = false, features = ["sim"] }
//! ```
//!
//! - `sim` (default): deterministic simulation plus `core`, `ecs`, `game` and `nav`.
//! - `net` (default): UDP transport, protocol, prediction and snapshots (implies `sim`).
//! - `render`: Vulkan renderer and winit platform; pulls GPU/windowing dependencies.
//!
//! Headless servers and tools should disable `render`; the server never depends on Vulkan.
//! The individual crates remain public and can be depended on directly when a narrower
//! boundary is useful.

#![forbid(unsafe_code)]

#[cfg(feature = "sim")]
pub use moba_core as core;
#[cfg(feature = "sim")]
pub use moba_ecs as ecs;
#[cfg(feature = "sim")]
pub use moba_game as game;
#[cfg(feature = "sim")]
pub use moba_nav as nav;
#[cfg(feature = "sim")]
pub use moba_sim as sim;

#[cfg(feature = "net")]
pub use moba_net as net;

#[cfg(feature = "render")]
pub use moba_platform as platform;
#[cfg(feature = "render")]
pub use moba_render as render;

/// Commonly used simulation types.
#[cfg(feature = "sim")]
pub mod prelude {
    pub use moba_core::{Fix, FixVec2};
    pub use moba_game::Team;
    pub use moba_sim::{PlayerCommand, SimConfig, SimEvent, Simulation, TickInputs};
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "sim")]
    #[test]
    fn headless_embedding_smoke() {
        let mut simulation = crate::sim::Simulation::new(crate::sim::SimConfig {
            bot_mask: 0,
            spawn_waves: false,
            ..crate::sim::SimConfig::default()
        });
        let mut inputs = crate::sim::TickInputs::default();
        inputs.set(
            0,
            crate::sim::PlayerCommand::MoveTo(crate::core::FixVec2::from_ints(80, 128)),
        );
        for _ in 0..30 {
            simulation.tick(&inputs);
        }
        assert_eq!(simulation.current_tick().get(), 30);
        assert!(simulation.hero_position(0).is_some());
    }

    #[cfg(feature = "net")]
    #[test]
    fn protocol_is_reachable_through_the_umbrella() {
        let header = crate::net::Header {
            kind: crate::net::PacketKind::Ping,
            flags: 0,
            session: 0,
            seq: 0,
            ack: 0,
            ack_bits: 0,
        };
        let mut bytes = Vec::new();
        crate::net::encode_packet(&header, &[], &mut bytes).unwrap();
        assert_eq!(
            crate::net::decode_packet(&bytes).unwrap().0.kind,
            crate::net::PacketKind::Ping
        );
    }
}
