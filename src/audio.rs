//! Audio output: noise is synthesised in the cpal callback thread of this process.
//!
//! The UI thread never touches samples. It publishes a target gain and a noise kind
//! through atomics; the callback smooths the gain per sample, so volume changes, mute,
//! pause and noise switches never click. Because the stream lives inside this process,
//! no audio can outlive `work` - not even after SIGKILL.
//!
//! On Linux cpal uses ALSA; with `pipewire-alsa` that is routed into PipeWire.
//! Only this stream's samples are scaled; the system mixer is never touched.

use crate::noise::{Generator, NoiseKind};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Gain smoothing time constant. ~5 tau (200 ms) for a full fade.
const SMOOTH_TAU_S: f32 = 0.040;
/// Below this gain the output is treated as silence.
const SILENCE: f32 = 1.0e-4;
/// Requested device buffer: ~100 ms keeps wakeups rare while controls stay responsive.
const BUFFER_S: f32 = 0.100;

struct Shared {
    target_gain: AtomicU32,
    current_gain: AtomicU32,
    kind: AtomicU8,
    error: Mutex<Option<String>>,
    errored: AtomicBool,
}

pub struct Audio {
    stream: Option<Stream>,
    shared: Arc<Shared>,
    latency: Duration,
    stream_paused: bool,
    can_pause: bool,
    quiet_since: Option<Instant>,
}

/// Perceptual volume curve: 0-100 % -> linear gain.
pub fn volume_to_gain(volume: u8) -> f32 {
    let v = f32::from(volume.min(100)) / 100.0;
    v * v
}

fn seed() -> u64 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    t ^ (u64::from(std::process::id()) << 32)
}

impl Audio {
    /// Opens the default output device and starts a (silent) stream.
    pub fn open(kind: NoiseKind) -> Result<Audio, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no audio output device found")?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("cannot query audio device: {e}"))?;
        let format = supported.sample_format();
        let mut config: StreamConfig = supported.config();

        let shared = Arc::new(Shared {
            target_gain: AtomicU32::new(0f32.to_bits()),
            current_gain: AtomicU32::new(0f32.to_bits()),
            kind: AtomicU8::new(kind.to_u8()),
            error: Mutex::new(None),
            errored: AtomicBool::new(false),
        });

        let wanted = (config.sample_rate as f32 * BUFFER_S) as u32;
        let fixed = match supported.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => Some(wanted.clamp(*min, *max)),
            cpal::SupportedBufferSize::Unknown => None,
        };

        // Try a fixed ~100 ms buffer first, fall back to the device default.
        let mut last_err = String::new();
        for size in [fixed, None] {
            config.buffer_size = size.map_or(cpal::BufferSize::Default, cpal::BufferSize::Fixed);
            match build_stream(&device, &config, format, kind, &shared) {
                Ok(stream) => {
                    stream
                        .play()
                        .map_err(|e| format!("cannot start audio stream: {e}"))?;
                    let frames = size.unwrap_or(wanted);
                    let latency =
                        Duration::from_secs_f32(frames as f32 / config.sample_rate as f32);
                    return Ok(Audio {
                        stream: Some(stream),
                        shared,
                        latency: latency
                            .clamp(Duration::from_millis(20), Duration::from_millis(500)),
                        stream_paused: false,
                        can_pause: true,
                        quiet_since: None,
                    });
                }
                Err(e) => last_err = e,
            }
            if fixed.is_none() {
                break;
            }
        }
        Err(last_err)
    }

    pub fn set_kind(&self, kind: NoiseKind) {
        self.shared.kind.store(kind.to_u8(), Relaxed);
    }

    /// Sets the desired output gain (0.0 = silent). Changes are smoothed by the callback.
    pub fn set_gain(&mut self, gain: f32) {
        self.shared
            .target_gain
            .store(gain.max(0.0).to_bits(), Relaxed);
        if gain > 0.0 && self.stream_paused {
            if let Some(s) = &self.stream {
                let _ = s.play();
            }
            self.stream_paused = false;
        }
        self.quiet_since = None;
    }

    fn target(&self) -> f32 {
        f32::from_bits(self.shared.target_gain.load(Relaxed))
    }

    fn current(&self) -> f32 {
        f32::from_bits(self.shared.current_gain.load(Relaxed))
    }

    /// True while a fade or a pending device suspend still needs periodic `maintain` calls.
    pub fn needs_maintenance(&self) -> bool {
        self.target() == 0.0 && !self.stream_paused && self.can_pause && self.stream.is_some()
    }

    /// Suspends the device once the output has been silent long enough for the
    /// buffered fade-out to play, so a paused session costs no audio CPU at all.
    pub fn maintain(&mut self, now: Instant) {
        if !self.needs_maintenance() {
            return;
        }
        if self.current() > 0.0 {
            self.quiet_since = None;
            return;
        }
        let since = *self.quiet_since.get_or_insert(now);
        if now.duration_since(since) >= self.latency
            && let Some(s) = &self.stream
        {
            match s.pause() {
                Ok(()) => self.stream_paused = true,
                Err(_) => self.can_pause = false, // keep streaming silence instead
            }
        }
    }

    pub fn error(&self) -> Option<String> {
        if !self.shared.errored.load(Relaxed) {
            return None;
        }
        self.shared.error.lock().ok()?.clone()
    }

    /// Fades out, lets the buffered tail play, then closes the device.
    pub fn shutdown(&mut self) {
        if self.stream.is_none() {
            return;
        }
        if !self.stream_paused {
            self.shared.target_gain.store(0f32.to_bits(), Relaxed);
            let deadline = Instant::now() + Duration::from_millis(400);
            while self.current() > 0.0
                && Instant::now() < deadline
                && !self.shared.errored.load(Relaxed)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(self.latency);
        }
        self.stream = None; // dropping the stream stops the callback thread and closes the PCM
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    kind: NoiseKind,
    shared: &Arc<Shared>,
) -> Result<Stream, String> {
    match format {
        SampleFormat::F32 => build::<f32>(device, config, kind, shared),
        SampleFormat::I16 => build::<i16>(device, config, kind, shared),
        SampleFormat::I32 => build::<i32>(device, config, kind, shared),
        SampleFormat::U16 => build::<u16>(device, config, kind, shared),
        SampleFormat::F64 => build::<f64>(device, config, kind, shared),
        other => Err(format!("unsupported audio sample format: {other}")),
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    kind: NoiseKind,
    shared: &Arc<Shared>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let alpha = 1.0 - (-1.0 / (SMOOTH_TAU_S * config.sample_rate as f32)).exp();
    let s = seed();
    // Independent generators for left and right give a wide, natural stereo image.
    let mut gens = [
        Generator::new(kind, s),
        Generator::new(kind, s.rotate_left(17) ^ 0xA5A5),
    ];
    let mut cur_kind = kind;
    let mut gain = 0.0f32;

    let cb_shared = Arc::clone(shared);
    let err_shared = Arc::clone(shared);

    device
        .build_output_stream(
            *config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let target = f32::from_bits(cb_shared.target_gain.load(Relaxed));
                let want = NoiseKind::from_u8(cb_shared.kind.load(Relaxed));
                for frame in data.chunks_mut(channels) {
                    // Switching noise colour: fade out, swap, fade back in.
                    let t = if want != cur_kind { 0.0 } else { target };
                    gain += (t - gain) * alpha;
                    if (gain - t).abs() < SILENCE {
                        gain = t;
                    }
                    if gain == 0.0 && want != cur_kind {
                        cur_kind = want;
                        gens.iter_mut().for_each(|g| g.set_kind(want));
                    }
                    if gain == 0.0 {
                        frame.iter_mut().for_each(|x| *x = T::from_sample(0.0f32));
                        continue;
                    }
                    let l = gens[0].next_sample() * gain;
                    let r = if channels > 1 {
                        gens[1].next_sample() * gain
                    } else {
                        l
                    };
                    for (i, x) in frame.iter_mut().enumerate() {
                        *x = T::from_sample(if i % 2 == 0 { l } else { r });
                    }
                }
                cb_shared.current_gain.store(gain.to_bits(), Relaxed);
            },
            move |err: cpal::Error| {
                // Underruns and missing RT priority are recoverable; the stream keeps going.
                if matches!(
                    err.kind(),
                    cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied
                ) {
                    return;
                }
                if let Ok(mut slot) = err_shared.error.lock() {
                    *slot = Some(err.to_string());
                }
                err_shared.errored.store(true, Relaxed);
            },
            None,
        )
        .map_err(|e| format!("cannot open audio stream: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_curve() {
        assert_eq!(volume_to_gain(0), 0.0);
        assert_eq!(volume_to_gain(100), 1.0);
        assert_eq!(volume_to_gain(200), 1.0);
        assert!((volume_to_gain(50) - 0.25).abs() < 1e-6);
        let mut prev = -1.0;
        for v in 0..=100 {
            let g = volume_to_gain(v);
            assert!(g > prev);
            prev = g;
        }
    }
}
