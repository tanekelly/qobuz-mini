use std::{sync::Arc, time::Duration};

use parking_lot::RwLock;
use rodio::source::Source;
use rodio::source::SeekError;
use signalsmith_stretch::Stretch;

use crate::sink::PlaybackStretchConfig;

const BLOCK_FRAMES: usize = 2048;
const N_CHANNELS: usize = 2;

pub struct SignalsmithStretchSource<S>
where
    S: Source<Item = f32> + Send,
{
    inner: S,
    sample_rate: u32,
    original_duration: Duration,
    playback_stretch: Arc<RwLock<PlaybackStretchConfig>>,
    stretch: Stretch,
    ratio: f32,
    pitch_semitones: f32,
    input_buf: Vec<f32>,
    output_buf: Vec<f32>,
    max_output_frames: usize,
    output_index: usize,
    output_len: usize,
    exhausted: bool,
}

impl<S> SignalsmithStretchSource<S>
where
    S: Source<Item = f32> + Send,
{
    pub fn new(
        inner: S,
        sample_rate: u32,
        playback_stretch: Arc<RwLock<PlaybackStretchConfig>>,
    ) -> Self {
        let original_duration = inner.total_duration().unwrap_or_default();
        let cfg = *playback_stretch.read();
        let ratio = normalize_ratio(cfg.time_stretch_ratio);
        let pitch_semitones = pitch_semitones(cfg);
        let mut stretch = Stretch::preset_default(N_CHANNELS as u32, sample_rate);
        if pitch_semitones.abs() > 0.001 {
            stretch.set_transpose_factor_semitones(pitch_semitones, None);
        }
        let max_output_frames = (BLOCK_FRAMES as f32 / 0.5).ceil() as usize;
        let out_cap = max_output_frames * N_CHANNELS;
        Self {
            inner,
            sample_rate,
            original_duration,
            playback_stretch,
            stretch,
            ratio,
            pitch_semitones,
            input_buf: vec![0.0; BLOCK_FRAMES * N_CHANNELS],
            output_buf: vec![0.0; out_cap],
            max_output_frames,
            output_index: 0,
            output_len: 0,
            exhausted: false,
        }
    }

    fn refresh_params(&mut self) {
        let cfg = *self.playback_stretch.read();
        let ratio = normalize_ratio(cfg.time_stretch_ratio);
        let pitch_semitones = pitch_semitones(cfg);

        self.ratio = ratio;

        if (pitch_semitones - self.pitch_semitones).abs() > 0.0001 {
            self.stretch.set_transpose_factor_semitones(pitch_semitones, None);
            self.pitch_semitones = pitch_semitones;
        }
    }

    fn fill_output(&mut self) {
        self.refresh_params();

        let need_samples = BLOCK_FRAMES * N_CHANNELS;
        let mut got = 0usize;
        for i in 0..need_samples {
            self.input_buf[i] = match self.inner.next() {
                Some(s) => {
                    got += 1;
                    s
                }
                None => {
                    self.exhausted = true;
                    0.0
                }
            };
        }
        let input_frames = got / N_CHANNELS;
        if input_frames == 0 {
            if self.exhausted {
                let out_latency = self.stretch.output_latency();
                if out_latency > 0 {
                    let frames = out_latency.min(self.max_output_frames);
                    self.output_buf[..frames * N_CHANNELS].fill(0.0);
                    self.stretch.flush(&mut self.output_buf[..frames * N_CHANNELS]);
                    self.output_len = frames * N_CHANNELS;
                    self.output_index = 0;
                } else {
                    self.output_len = 0;
                }
            }
            return;
        }

        let output_frames = (input_frames as f32 / self.ratio).round() as usize;
        let output_frames = output_frames.max(1).min(self.max_output_frames);
        self.stretch.process(
            &self.input_buf[..input_frames * N_CHANNELS],
            &mut self.output_buf[..output_frames * N_CHANNELS],
        );
        self.output_len = output_frames * N_CHANNELS;
        self.output_index = 0;
    }
}

impl<S> Iterator for SignalsmithStretchSource<S>
where
    S: Source<Item = f32> + Send,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted && self.output_index >= self.output_len {
            return None;
        }
        while self.output_index >= self.output_len {
            self.fill_output();
            if self.output_len == 0 && self.exhausted {
                return None;
            }
        }
        let s = self.output_buf[self.output_index];
        self.output_index += 1;
        Some(s)
    }
}

impl<S> Source for SignalsmithStretchSource<S>
where
    S: Source<Item = f32> + Send,
{
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let ratio = normalize_ratio(self.playback_stretch.read().time_stretch_ratio);
        let content_pos = Duration::from_secs_f64(pos.as_secs_f64() * ratio as f64);

        self.inner.try_seek(content_pos)?;
        self.stretch.reset();
        self.output_index = 0;
        self.output_len = 0;
        self.exhausted = false;
        Ok(())
    }

    fn current_span_len(&self) -> Option<usize> {
        if self.exhausted && self.output_index >= self.output_len {
            return Some(0);
        }
        let remaining = self.output_len.saturating_sub(self.output_index);
        if remaining > 0 {
            Some(remaining)
        } else {
            None
        }
    }

    fn channels(&self) -> u16 {
        N_CHANNELS as u16
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        let ratio = normalize_ratio(self.playback_stretch.read().time_stretch_ratio);
        Some(Duration::from_secs_f64(
            self.original_duration.as_secs_f64() / ratio as f64,
        ))
    }
}

fn normalize_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(0.5, 2.0)
    } else {
        1.0
    }
}

fn pitch_semitones(cfg: PlaybackStretchConfig) -> f32 {
    cfg.pitch_semitones as f32 + cfg.pitch_cents as f32 / 100.0
}
