//! Minimal client-only procedural audio.
//!
//! One output stream plays short synthesized tones for gameplay events. If no output device
//! is available (headless CI, container), the engine silently disables itself; no audio type
//! reaches the simulation, network or server crates.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

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
    tones: VecDeque<Tone>,
    sample_rate: f32,
    volume: f32,
}

impl Synth {
    fn schedule(&mut self, sound: Sound) {
        if self.volume <= 0.0 || self.samples_for(sound) == 0 {
            return;
        }
        let (frequency, seconds) = sound.tone();
        if self.sample_rate <= 0.0 {
            return;
        }
        let samples = (seconds * self.sample_rate) as u32;
        self.tones.push_back(Tone {
            phase: 0.0,
            step: std::f32::consts::TAU * frequency / self.sample_rate,
            remaining: samples,
            samples,
        });
    }

    fn samples_for(&self, sound: Sound) -> u32 {
        let (_, seconds) = sound.tone();
        if self.sample_rate <= 0.0 {
            0
        } else {
            (seconds * self.sample_rate) as u32
        }
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

fn clear_tones(synth: &Arc<Mutex<Synth>>) {
    if let Ok(mut synth) = synth.lock() {
        synth.tones.clear();
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
    synth: Arc<Mutex<Synth>>,
    stream: Option<cpal::Stream>,
    mute: bool,
}

impl Audio {
    /// Opens the default output device. Never panics on missing hardware.
    #[must_use]
    pub(crate) fn new(volume: f32, mute: bool) -> Self {
        let synth = Arc::new(Mutex::new(Synth {
            volume: volume.clamp(0.0, 1.0),
            ..Synth::default()
        }));
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return Self::disabled(synth, mute);
        };
        let Ok(supported) = device.default_output_config() else {
            return Self::disabled(synth, mute);
        };
        let format = supported.sample_format();
        let config = supported.config();
        let sample_rate = config.sample_rate.0 as f32;
        let channels = usize::from(config.channels);
        if let Ok(mut synth) = synth.lock() {
            synth.sample_rate = sample_rate;
        }
        let callback_synth = Arc::clone(&synth);
        let error_synth = Arc::clone(&synth);
        let stream = match format {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    let mut synth = callback_synth
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    for frame in data.chunks_mut(channels) {
                        let sample = synth.next_sample();
                        for output in frame {
                            *output = sample;
                        }
                    }
                },
                move |_error| clear_tones(&error_synth),
                None,
            ),
            cpal::SampleFormat::I16 => {
                let callback_synth = Arc::clone(&synth);
                let error_synth = Arc::clone(&synth);
                device.build_output_stream(
                    &config,
                    move |data: &mut [i16], _| {
                        let mut synth = callback_synth
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        for frame in data.chunks_mut(channels) {
                            let sample = to_i16(synth.next_sample());
                            for output in frame {
                                *output = sample;
                            }
                        }
                    },
                    move |_error| clear_tones(&error_synth),
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let callback_synth = Arc::clone(&synth);
                let error_synth = Arc::clone(&synth);
                device.build_output_stream(
                    &config,
                    move |data: &mut [u16], _| {
                        let mut synth = callback_synth
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        for frame in data.chunks_mut(channels) {
                            let sample = to_u16(synth.next_sample());
                            for output in frame {
                                *output = sample;
                            }
                        }
                    },
                    move |_error| clear_tones(&error_synth),
                    None,
                )
            }
            _ => {
                return Self::disabled(synth, mute);
            }
        };
        match stream {
            Ok(stream) => {
                if stream.play().is_err() {
                    return Self::disabled(synth, mute);
                }
                Self {
                    synth,
                    stream: Some(stream),
                    mute,
                }
            }
            Err(_) => Self::disabled(synth, mute),
        }
    }

    fn disabled(synth: Arc<Mutex<Synth>>, mute: bool) -> Self {
        Self {
            synth,
            stream: None,
            mute,
        }
    }

    /// Queues a sound; no-op when muted or headless.
    pub(crate) fn play(&mut self, sound: Sound) {
        if self.mute || self.stream.is_none() {
            return;
        }
        if let Ok(mut synth) = self.synth.lock() {
            synth.schedule(sound);
        }
    }

    /// Updates volume/mute at runtime.
    #[cfg(test)]
    fn configure(&mut self, volume: f32, mute: bool) {
        self.mute = mute;
        if let Ok(mut synth) = self.synth.lock() {
            synth.volume = volume.clamp(0.0, 1.0);
        }
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
            Arc::new(Mutex::new(Synth {
                volume: 1.0,
                sample_rate: 48_000.0,
                ..Synth::default()
            })),
            false,
        );
        assert!(!audio.is_active());
        audio.play(Sound::Attack);
        if let Ok(synth) = audio.synth.lock() {
            assert!(synth.tones.is_empty());
        }
        audio.configure(0.5, true);
        assert!(!audio.is_active());
    }
}
