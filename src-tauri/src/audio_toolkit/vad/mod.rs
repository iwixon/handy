use anyhow::Result;

pub enum VadFrame<'a> {
    /// Speech – may aggregate several frames (prefill + current + hangover)
    Speech(&'a [f32]),
    /// Non-speech (silence, noise). Down-stream code can ignore it.
    Noise,
}

impl<'a> VadFrame<'a> {
    #[inline]
    pub fn is_speech(&self) -> bool {
        matches!(self, VadFrame::Speech(_))
    }
}

pub trait VoiceActivityDetector: Send + Sync {
    /// Primary streaming API: feed one 30-ms frame, get keep/drop decision.
    fn push_frame<'a>(&'a mut self, frame: &'a [f32]) -> Result<VadFrame<'a>>;

    fn is_voice(&mut self, frame: &[f32]) -> Result<bool> {
        Ok(self.push_frame(frame)?.is_speech())
    }

    fn reset(&mut self) {}
}

mod silero;
mod smoothed;

/// Convert a decibel value to a linear amplitude multiplier (e.g. +12 dB ≈ 3.98).
#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub use silero::{SileroVad, DEFAULT_VAD_THRESHOLD, WHISPER_VAD_THRESHOLD};
pub use smoothed::{
    SmoothedVad, DEFAULT_VAD_HANGOVER_FRAMES, DEFAULT_VAD_ONSET_FRAMES, DEFAULT_VAD_PREFILL_FRAMES,
    WHISPER_VAD_HANGOVER_FRAMES, WHISPER_VAD_ONSET_FRAMES, WHISPER_VAD_PREFILL_FRAMES,
};
