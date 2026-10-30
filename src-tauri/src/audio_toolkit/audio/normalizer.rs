use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;

/// Peak target for low-level recordings before batch inference (-1.5 dBFS).
pub const TARGET_PEAK: f32 = 0.841;
const TARGET_STREAM_RMS: f32 = 0.08;
const MAX_STREAM_GAIN: f32 = 32.0;
const MIN_RMS: f32 = 1e-6;
const RELEASE_SECONDS: f32 = 0.25;
const MAX_PEAK: f32 = 0.98;

/// Raises quiet batches to the peak target; silence and already-loud audio are unchanged.
pub fn normalize_peak(samples: &mut [f32]) {
    let peak = samples
        .iter()
        .fold(0.0_f32, |max, sample| max.max(sample.abs()));
    if peak <= MIN_RMS || peak >= TARGET_PEAK {
        return;
    }

    let gain = TARGET_PEAK / peak;
    for sample in samples {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }
}

/// Causal level normalization for microphone frames sent to live ASR streams.
///
/// Keeps a short release envelope so frame-to-frame volume changes do not pump.
/// The gain is bounded, and a peak ceiling prevents amplification from clipping.
pub struct QuietSpeechNormalizer {
    rms_envelope: f32,
    scratch: Vec<f32>,
}

impl QuietSpeechNormalizer {
    pub fn new(frame_samples: usize) -> Self {
        Self {
            rms_envelope: 0.0,
            scratch: Vec::with_capacity(frame_samples),
        }
    }

    /// Normalizes into reusable scratch storage; the input recording stays intact.
    pub fn normalize(&mut self, input: &[f32]) -> &[f32] {
        self.scratch.clear();
        self.scratch.extend_from_slice(input);
        Self::apply_level_gain(&mut self.rms_envelope, &mut self.scratch);
        &self.scratch
    }

    /// Applies the same stream leveler to one block of a batch recording.
    pub fn normalize_in_place(&mut self, samples: &mut [f32]) {
        Self::apply_level_gain(&mut self.rms_envelope, samples);
    }

    fn apply_level_gain(rms_envelope: &mut f32, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }

        let mean_square =
            samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32;
        let rms = mean_square.sqrt();
        let duration = samples.len() as f32 / WHISPER_SAMPLE_RATE as f32;
        let released = *rms_envelope * (-duration / RELEASE_SECONDS).exp();
        *rms_envelope = rms.max(released);
        if !rms_envelope.is_finite() || *rms_envelope <= MIN_RMS {
            return;
        }

        let peak = samples
            .iter()
            .fold(0.0_f32, |max, sample| max.max(sample.abs()));
        let target_gain = (TARGET_STREAM_RMS / *rms_envelope).min(MAX_STREAM_GAIN);
        let peak_gain = if peak > 0.0 {
            MAX_PEAK / peak
        } else {
            MAX_STREAM_GAIN
        };
        let gain = target_gain.min(peak_gain);
        if gain == 1.0 {
            return;
        }
        for sample in samples {
            *sample = (*sample * gain).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_peak_normalization_raises_quiet_audio_without_changing_loud_audio_or_silence() {
        let mut quiet = vec![0.01; 32];
        normalize_peak(&mut quiet);
        assert!((quiet[0] - TARGET_PEAK).abs() < 1e-6);

        let mut loud = vec![0.9; 32];
        normalize_peak(&mut loud);
        assert_eq!(loud, vec![0.9; 32]);

        let mut silence = vec![0.0; 32];
        normalize_peak(&mut silence);
        assert_eq!(silence, vec![0.0; 32]);
    }

    #[test]
    fn stream_normalization_amplifies_quiet_frames_without_clipping() {
        let mut normalizer = QuietSpeechNormalizer::new(512);
        let output = normalizer.normalize(&[0.01; 512]);
        let peak = output
            .iter()
            .fold(0.0_f32, |max, sample| max.max(sample.abs()));
        let rms =
            (output.iter().map(|sample| sample * sample).sum::<f32>() / output.len() as f32).sqrt();

        assert!(rms >= TARGET_STREAM_RMS - 1e-6);
        assert!(peak <= MAX_PEAK);
    }
}
