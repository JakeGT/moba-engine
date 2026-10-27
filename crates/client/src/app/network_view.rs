//! Network match view and event handling.

use std::collections::VecDeque;

use moba_core::math::Vec3;
use moba_core::{Fix, FixVec2};
use moba_net::protocol::{
    EVENT_CAST, EVENT_MATCH_ENDED, EVENT_PROJECTILE_SPAWN, EVENT_STRUCTURE_DESTROYED,
    EVENT_UNIT_DIED, MATCH_ENDED_DRAW, MatchPhase, NetEvent, dequantize_position,
};
use moba_net::{Interpolator, NetClient, PredictionState};

#[allow(clippy::struct_excessive_bools)] // Mode flags are independent UI/network state.
pub(crate) struct KillFeedEntry {
    pub(crate) life: u32,
    pub(crate) text: String,
}

impl ProjectileFx {
    pub(crate) fn arc_height(&self) -> f32 {
        if self.total <= 0.0 || self.arc <= 0.0 {
            return 0.0;
        }
        let progress = (self.travelled / self.total).clamp(0.0, 1.0);
        self.arc * (core::f32::consts::PI * progress).sin()
    }
}

pub(crate) struct ProjectileFx {
    pub(crate) team: u8,
    pub(crate) position: FixVec2,
    pub(crate) direction: FixVec2,
    pub(crate) speed: f32,
    pub(crate) radius: f32,
    pub(crate) remaining: f32,
    pub(crate) arc: f32,
    pub(crate) total: f32,
    pub(crate) travelled: f32,
}

#[allow(clippy::struct_excessive_bools)] // Independent connection/UI flags.
pub(crate) struct NetworkView {
    pub(crate) client: NetClient,
    pub(crate) prediction: PredictionState,
    pub(crate) interpolator: Interpolator,
    pub(crate) own_slot: u8,
    pub(crate) own_net_id: Option<u16>,
    pub(crate) last_snapshot_seq: u32,
    pub(crate) phase: MatchPhase,
    pub(crate) countdown: u16,
    pub(crate) spectator: bool,
    pub(crate) ready: bool,
    pub(crate) ready_count: u8,
    pub(crate) player_count: u8,
    pub(crate) last_tick: u32,
    pub(crate) demo_move: Option<FixVec2>,
    pub(crate) demo_sent: bool,
    pub(crate) demo_buy: Option<u8>,
    pub(crate) demo_bought: bool,
    pub(crate) shop_page: usize,
    pub(crate) spectator_pan: Vec3,
    pub(crate) spectator_yaw: f32,
    pub(crate) spectator_zoom: f32,
    pub(crate) kill_feed: VecDeque<KillFeedEntry>,
    pub(crate) projectiles: Vec<ProjectileFx>,
    pub(crate) audio: crate::audio::Audio,
}

impl NetworkView {
    #[cfg(test)]
    pub(crate) fn test_stub() -> Self {
        let client = NetClient::start("127.0.0.1:9".parse().expect("addr")).expect("client");
        Self {
            client,
            prediction: PredictionState::new(Fix::from_f32(5.0)),
            interpolator: Interpolator::new(2.0, 30.0),
            own_slot: 0,
            own_net_id: None,
            last_snapshot_seq: 0,
            phase: MatchPhase::Lobby,
            countdown: 0,
            spectator: false,
            ready: false,
            ready_count: 0,
            player_count: 0,
            last_tick: 0,
            demo_move: None,
            demo_sent: false,
            demo_buy: None,
            demo_bought: false,
            shop_page: 0,
            spectator_pan: Vec3::ZERO,
            spectator_yaw: -0.7,
            spectator_zoom: 1.0,
            kill_feed: VecDeque::with_capacity(6),
            projectiles: Vec::with_capacity(32),
            audio: crate::audio::Audio::headless(),
        }
    }

    #[allow(clippy::too_many_lines)] // Event-to-HUD/audio mapping is linear.
    pub(crate) fn handle_game_event(&mut self, event: NetEvent) {
        match event.kind {
            EVENT_CAST => self.audio.play(crate::audio::Sound::Cast),
            EVENT_PROJECTILE_SPAWN => {
                self.audio.play(crate::audio::Sound::Attack);
                let direction = FixVec2::new(
                    Fix::from_f32(f32::from(event.dx) / 32_767.0),
                    Fix::from_f32(f32::from(event.dy) / 32_767.0),
                );
                let position = dequantize_position(event.x, event.y);
                let end = dequantize_position(event.x2, event.y2);
                let total = position.distance(end).to_f32();
                self.projectiles.push(ProjectileFx {
                    team: event.team,
                    position,
                    direction,
                    speed: f32::from(event.value),
                    radius: f32::from(event.aux),
                    remaining: total,
                    arc: f32::from(event.arc) / 16.0,
                    total,
                    travelled: 0.0,
                });
                if self.projectiles.len() > 32 {
                    self.projectiles.remove(0);
                }
            }
            EVENT_UNIT_DIED => {
                self.audio.play(crate::audio::Sound::Death);
                let text = if event.target_slot == u8::MAX {
                    String::from("Unit destroyed")
                } else {
                    let team = if event.team == 0 { "Blue" } else { "Red" };
                    format!(
                        "{team} hero {} eliminated",
                        u16::from(event.target_slot) + 1
                    )
                };
                self.kill_feed.push_back(KillFeedEntry { life: 600, text });
                if self.kill_feed.len() > 6 {
                    self.kill_feed.pop_front();
                }
            }
            EVENT_STRUCTURE_DESTROYED => {
                self.audio.play(crate::audio::Sound::Objective);
                self.kill_feed.push_back(KillFeedEntry {
                    life: 600,
                    text: String::from("Structure destroyed"),
                });
                if self.kill_feed.len() > 6 {
                    self.kill_feed.pop_front();
                }
            }
            EVENT_MATCH_ENDED => {
                self.audio.play(crate::audio::Sound::Ui);
                let text = if event.value == MATCH_ENDED_DRAW {
                    String::from("Match drawn")
                } else if event.team == 0 {
                    String::from("Blue team wins")
                } else {
                    String::from("Red team wins")
                };
                self.kill_feed.push_back(KillFeedEntry { life: 600, text });
            }
            _ => {}
        }
    }

    pub(crate) fn update_effects(&mut self, frame_ms: f32) {
        let dt = frame_ms.max(0.0) / 1_000.0;
        for projectile in &mut self.projectiles {
            projectile.position += projectile.direction * Fix::from_f32(projectile.speed * dt);
            projectile.remaining -= projectile.speed * dt;
            projectile.travelled += projectile.speed * dt;
        }
        self.projectiles
            .retain(|projectile| projectile.remaining > 0.0);
        for entry in &mut self.kill_feed {
            entry.life = entry.life.saturating_sub(1);
        }
        self.kill_feed.retain(|entry| entry.life > 0);
    }
}
