//! Minimal client-only procedural audio.
//!
//! One output stream plays short synthesized tones for gameplay events. Scheduling uses a
//! lock-free bounded queue, so the real-time callback never waits on a mutex. If no output
//! device is available (headless CI, container), the engine silently disables itself.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_queue::ArrayQueue;

/// Maximum queued sound requests.
const MAX_QUEUED: usize = 64;
/// Maximum simultaneously scheduled tones.
const MAX_VOICES: usize = 16;

/// One game event that maps to a sound.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sound {
    /// Basic attack landed/spawned.
    Attack,
    /// Ability cast.
    Cast,
    /// Unit or hero died.
    Death,
    /// Structure/objective destroyed.
    Objective,
    /// UI confirmation.
    Ui,
}

impl Sound {
    const fn tone(self) -> (f32, f32) {
        match self {
            Self::Attack => (220.0, 0.05),
            Self::Cast => (440.0, 0.12),
            Self::Death => (110.0, 0.22),
            Self::Objective => (660.0, 0.3),
            Self::Ui => (880.0, 0.04),
        }
    }
}

struct Tone {
    phase: f32,
    step: f32,
    remaining: u32,
    samples: u32,
}

#[derive(Default)]
struct Synth {
    tones: std::collections::VecDeque<Tone>,
    sample_rate: f32,
    volume: f32,
}

impl Synth {
    fn schedule(&mut self, sound: Sound) {
        if self.volume <= 0.0 || self.sample_rate <= 0.0 {
            return;
        }
        let (frequency, seconds) = sound.tone();
        let samples = (seconds * self.sample_rate) as u32;
        if self.tones.len() >= MAX_VOICES {
            self.tones.pop_front();
        }
        self.tones.push_back(Tone {
            phase: 0.0,
            step: std::f32::consts::TAU * frequency / self.sample_rate,
            remaining: samples,
            samples,
        });
    }

    #[cfg(test)]
    fn voice_count(&self) -> usize {
        self.tones.len()
    }

    fn next_sample(&mut self) -> f32 {
        let Some(tone) = self.tones.front_mut() else {
            return 0.0;
        };
        let envelope = if tone.samples == 0 {
            0.0
        } else {
            tone.remaining as f32 / tone.samples as f32
        };
        let sample = tone.phase.sin() * envelope * 0.18 * self.volume;
        tone.phase = (tone.phase + tone.step) % std::f32::consts::TAU;
        tone.remaining = tone.remaining.saturating_sub(1);
        if tone.remaining == 0 {
            self.tones.pop_front();
        }
        sample
    }
}

fn render<T: Copy>(data: &mut [T], channels: usize, synth: &mut Synth, convert: impl Fn(f32) -> T) {
    for frame in data.chunks_mut(channels) {
        let sample = convert(synth.next_sample());
        for output in frame {
            *output = sample;
        }
    }
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

fn to_u16(sample: f32) -> u16 {
    ((sample.clamp(-1.0, 1.0) * 0.5 + 0.5) * f32::from(u16::MAX)) as u16
}

/// Client audio engine; disabled gracefully when no device exists.
pub(crate) struct Audio {
    scheduler: Arc<ArrayQueue<Sound>>,
    stream: Option<cpal::Stream>,
    mute: bool,
    volume: Arc<AtomicU32>,
    lost: Arc<AtomicBool>,
}

impl Audio {
    /// Opens the default output device. Never panics on missing hardware.
    #[must_use]
    #[allow(clippy::too_many_lines)] // Device/format setup is one linear path.
    pub(crate) fn new(volume: f32, mute: bool) -> Self {
        let volume = Arc::new(AtomicU32::new(volume.clamp(0.0, 1.0).to_bits()));
        let scheduler = Arc::new(ArrayQueue::new(MAX_QUEUED));
        let lost = Arc::new(AtomicBool::new(false));
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return Self::disabled(scheduler, volume, mute, lost);
        };
        let Ok(supported) = device.default_output_config() else {
            return Self::disabled(scheduler, volume, mute, lost);
        };
        let format = supported.sample_format();
        let config = supported.config();
        let sample_rate = config.sample_rate.0 as f32;
        let channels = usize::from(config.channels);
        let render_f32 = {
            let queue = Arc::clone(&scheduler);
            let volume = Arc::clone(&volume);
            let lost = Arc::clone(&lost);
            device.build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    let mut synth = Synth {
                        sample_rate,
                        volume: f32::from_bits(volume.load(Ordering::Relaxed)),
                        ..Synth::default()
                    };
                    while let Some(sound) = queue.pop() {
                        synth.schedule(sound);
                    }
                    render(data, channels, &mut synth, |sample| sample);
                },
                move |_error| lost.store(true, Ordering::Relaxed),
                None,
            )
        };
        let render_i16 = {
            let queue = Arc::clone(&scheduler);
            let volume = Arc::clone(&volume);
            let lost = Arc::clone(&lost);
            device.build_output_stream(
                &config,
                move |data: &mut [i16], _| {
                    let mut synth = Synth {
                        sample_rate,
                        volume: f32::from_bits(volume.load(Ordering::Relaxed)),
                        ..Synth::default()
                    };
                    while let Some(sound) = queue.pop() {
                        synth.schedule(sound);
                    }
                    render(data, channels, &mut synth, to_i16);
                },
                move |_error| lost.store(true, Ordering::Relaxed),
                None,
            )
        };
        let render_u16 = {
            let queue = Arc::clone(&scheduler);
            let volume = Arc::clone(&volume);
            let lost = Arc::clone(&lost);
            device.build_output_stream(
                &config,
                move |data: &mut [u16], _| {
                    let mut synth = Synth {
                        sample_rate,
                        volume: f32::from_bits(volume.load(Ordering::Relaxed)),
                        ..Synth::default()
                    };
                    while let Some(sound) = queue.pop() {
                        synth.schedule(sound);
                    }
                    render(data, channels, &mut synth, to_u16);
                },
                move |_error| lost.store(true, Ordering::Relaxed),
                None,
            )
        };
        let stream = match format {
            cpal::SampleFormat::F32 => render_f32,
            cpal::SampleFormat::I16 => render_i16,
            cpal::SampleFormat::U16 => render_u16,
            _ => return Self::disabled(scheduler, volume, mute, lost),
        };
        match stream {
            Ok(stream) => {
                if stream.play().is_err() {
                    return Self::disabled(scheduler, volume, mute, lost);
                }
                Self {
                    scheduler,
                    stream: Some(stream),
                    mute,
                    volume,
                    lost,
                }
            }
            Err(_) => Self::disabled(scheduler, volume, mute, lost),
        }
    }

    #[cfg(test)]
    pub(crate) fn headless() -> Self {
        Self::disabled(
            Arc::new(ArrayQueue::new(MAX_QUEUED)),
            Arc::new(AtomicU32::new(1.0f32.to_bits())),
            true,
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn disabled(
        scheduler: Arc<ArrayQueue<Sound>>,
        volume: Arc<AtomicU32>,
        mute: bool,
        lost: Arc<AtomicBool>,
    ) -> Self {
        Self {
            scheduler,
            stream: None,
            mute,
            volume,
            lost,
        }
    }

    /// Queues a sound; no-op when muted or headless.
    pub(crate) fn play(&mut self, sound: Sound) {
        if self.mute || self.stream.is_none() {
            return;
        }
        let _ = self.scheduler.push(sound);
    }

    /// Toggles mute and returns the new state.
    pub(crate) fn toggle_mute(&mut self) -> bool {
        self.mute = !self.mute;
        self.mute
    }

    /// Reopens the output device after a runtime stream error.
    pub(crate) fn poll(&mut self) {
        if self.stream.is_some() && self.lost.swap(false, Ordering::Relaxed) {
            *self = Self::new(
                f32::from_bits(self.volume.load(Ordering::Relaxed)),
                self.mute,
            );
        }
    }

    /// Updates volume/mute at runtime.
    #[cfg(test)]
    fn configure(&mut self, volume: f32, mute: bool) {
        self.mute = mute;
        self.volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Whether an output stream is active.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.stream.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_mapping_is_distinct_and_short() {
        let mut frequencies = [0u32; 5];
        for (index, sound) in [
            Sound::Attack,
            Sound::Cast,
            Sound::Death,
            Sound::Objective,
            Sound::Ui,
        ]
        .into_iter()
        .enumerate()
        {
            let (frequency, seconds) = sound.tone();
            assert!(frequency > 0.0 && seconds > 0.0 && seconds < 1.0);
            frequencies[index] = frequency as u32;
        }
        frequencies.sort_unstable();
        assert!(frequencies.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn voice_pool_is_bounded() {
        let mut synth = Synth {
            sample_rate: 48_000.0,
            volume: 1.0,
            ..Synth::default()
        };
        for _ in 0..64 {
            synth.schedule(Sound::Attack);
        }
        assert_eq!(synth.voice_count(), MAX_VOICES);
    }

    #[test]
    fn sample_conversions_cover_the_full_range() {
        assert_eq!(to_i16(-1.0), i16::MIN + 1);
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(1.0), i16::MAX);
        assert_eq!(to_u16(-1.0), 0);
        assert_eq!(to_u16(0.0), 32_767);
        assert_eq!(to_u16(1.0), u16::MAX);
    }

    #[test]
    fn headless_engine_is_inert_and_configurable() {
        let mut audio = Audio::disabled(
            Arc::new(ArrayQueue::new(MAX_QUEUED)),
            Arc::new(AtomicU32::new(1.0f32.to_bits())),
            false,
            Arc::new(AtomicBool::new(false)),
        );
        assert!(!audio.is_active());
        audio.play(Sound::Attack);
        assert!(audio.scheduler.is_empty());
        audio.configure(0.5, true);
        assert!(!audio.is_active());
        assert!(!audio.toggle_mute());
        assert!(audio.toggle_mute());
        audio.poll();
    }
}
