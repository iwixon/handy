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

/// Fully-resolved VAD/DSP tuning used to build a recorder. Produced by
/// [`parse_vad_config`] from a free-text config string in pipeline mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VadConfig {
    /// Silero speech-probability threshold (0..1). Lower = catches quieter speech.
    pub threshold: f32,
    /// Detection-only gain in dB applied to the copy the VAD scores (not the
    /// recorded audio). 0 dB = unity (no boost).
    pub gain_db: f32,
    /// Pre-roll frames kept before a detected onset (30ms each).
    pub prefill: usize,
    /// Tail frames kept after speech ends (30ms each).
    pub hangover: usize,
    /// Consecutive voice frames required to latch onset.
    pub onset: usize,
}

impl Default for VadConfig {
    /// The upstream/normal-speech preset with no detection gain. An empty
    /// pipeline VAD-config string resolves to exactly this, so merely enabling
    /// the pipeline does not change capture behavior until the user tunes it.
    fn default() -> Self {
        Self {
            threshold: DEFAULT_VAD_THRESHOLD,
            gain_db: 0.0,
            prefill: DEFAULT_VAD_PREFILL_FRAMES,
            hangover: DEFAULT_VAD_HANGOVER_FRAMES,
            onset: DEFAULT_VAD_ONSET_FRAMES,
        }
    }
}

impl VadConfig {
    /// Linear detection-gain multiplier (1.0 when `gain_db` is 0).
    #[inline]
    pub fn detection_gain(&self) -> f32 {
        db_to_linear(self.gain_db)
    }
}

/// Parse a free-text VAD config string into a [`VadConfig`], starting from the
/// defaults and overriding per recognized `key=value` token (whitespace- or
/// comma-separated). Recognized keys: `threshold`, `gain` (dB), `prefill`,
/// `hangover`, `onset`. Unknown keys / unparseable values are logged and
/// ignored. An empty string yields [`VadConfig::default`].
///
/// Example: `threshold=0.18 gain=12 prefill=20 hangover=25 onset=1`
pub fn parse_vad_config(s: &str) -> VadConfig {
    let mut cfg = VadConfig::default();
    for token in s.split([' ', '\t', '\n', ',']).filter(|t| !t.is_empty()) {
        let Some((key, value)) = token.split_once('=') else {
            log::warn!("VAD config: ignoring malformed token '{token}' (expected key=value)");
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "threshold" => match value.parse::<f32>() {
                Ok(v) if (0.0..=1.0).contains(&v) => cfg.threshold = v,
                _ => log::warn!("VAD config: invalid threshold '{value}' (expected 0.0..=1.0)"),
            },
            "gain" | "gain_db" => match value.parse::<f32>() {
                Ok(v) => cfg.gain_db = v,
                Err(_) => log::warn!("VAD config: invalid gain '{value}' (expected dB number)"),
            },
            "prefill" => match value.parse::<usize>() {
                Ok(v) => cfg.prefill = v,
                Err(_) => log::warn!("VAD config: invalid prefill '{value}' (expected integer)"),
            },
            "hangover" => match value.parse::<usize>() {
                Ok(v) => cfg.hangover = v,
                Err(_) => log::warn!("VAD config: invalid hangover '{value}' (expected integer)"),
            },
            "onset" => match value.parse::<usize>() {
                Ok(v) if v >= 1 => cfg.onset = v,
                _ => log::warn!("VAD config: invalid onset '{value}' (expected integer >= 1)"),
            },
            _ => log::warn!("VAD config: ignoring unknown key '{key}'"),
        }
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_default() {
        assert_eq!(parse_vad_config(""), VadConfig::default());
        assert_eq!(parse_vad_config("   "), VadConfig::default());
    }

    #[test]
    fn parses_all_keys() {
        let cfg = parse_vad_config("threshold=0.18 gain=12 prefill=20 hangover=25 onset=1");
        assert_eq!(cfg.threshold, 0.18);
        assert_eq!(cfg.gain_db, 12.0);
        assert_eq!(cfg.prefill, 20);
        assert_eq!(cfg.hangover, 25);
        assert_eq!(cfg.onset, 1);
        assert!((cfg.detection_gain() - 3.981).abs() < 0.01);
    }

    #[test]
    fn unknown_and_malformed_tokens_ignored() {
        // Falls back to defaults for everything; bad tokens don't panic.
        let cfg = parse_vad_config("bogus=1 threshold=oops onset=0 hangover=10 garbage");
        assert_eq!(cfg.threshold, VadConfig::default().threshold);
        assert_eq!(cfg.onset, VadConfig::default().onset); // 0 rejected
        assert_eq!(cfg.hangover, 10); // valid override applied
    }

    #[test]
    fn comma_separated_also_works() {
        let cfg = parse_vad_config("threshold=0.2,onset=3");
        assert_eq!(cfg.threshold, 0.2);
        assert_eq!(cfg.onset, 3);
    }
}
