use super::{VadFrame, VoiceActivityDetector};
use anyhow::Result;
use std::collections::VecDeque;

/// Normal-speech smoothing defaults (prefill / hangover / onset frames).
pub const DEFAULT_VAD_PREFILL_FRAMES: usize = 15;
pub const DEFAULT_VAD_HANGOVER_FRAMES: usize = 15;
pub const DEFAULT_VAD_ONSET_FRAMES: usize = 2;
/// Whisper-mode preset: latch on the first voice frame (onset 1) and keep a
/// longer pre-roll (~600ms) and tail (~750ms) so soft onsets and trailing
/// whispered syllables are not clipped. Frames are 30ms each.
pub const WHISPER_VAD_PREFILL_FRAMES: usize = 20;
pub const WHISPER_VAD_HANGOVER_FRAMES: usize = 25;
pub const WHISPER_VAD_ONSET_FRAMES: usize = 1;

pub struct SmoothedVad {
    inner_vad: Box<dyn VoiceActivityDetector>,
    prefill_frames: usize,
    hangover_frames: usize,
    onset_frames: usize,
    /// Linear gain applied ONLY to the copy of each frame used for the inner
    /// VAD's speech/no-speech decision. The buffered/emitted samples stay at
    /// their original level so downstream consumers (Whisper) never see the
    /// clipping a boosted signal would introduce. 1.0 = no gain.
    detection_gain: f32,

    frame_buffer: VecDeque<Vec<f32>>,
    hangover_counter: usize,
    onset_counter: usize,
    in_speech: bool,

    temp_out: Vec<f32>,
    /// Scratch buffer reused for the gained detection copy (avoids per-frame alloc).
    detect_buf: Vec<f32>,
}

impl SmoothedVad {
    pub fn new(
        inner_vad: Box<dyn VoiceActivityDetector>,
        prefill_frames: usize,
        hangover_frames: usize,
        onset_frames: usize,
    ) -> Self {
        Self::with_detection_gain(inner_vad, prefill_frames, hangover_frames, onset_frames, 1.0)
    }

    /// Like [`new`](Self::new) but boosts the signal the inner VAD scores by
    /// `detection_gain` (linear). Use this to make quiet/whispered speech cross
    /// the inner VAD's threshold without amplifying the audio that is recorded.
    pub fn with_detection_gain(
        inner_vad: Box<dyn VoiceActivityDetector>,
        prefill_frames: usize,
        hangover_frames: usize,
        onset_frames: usize,
        detection_gain: f32,
    ) -> Self {
        Self {
            inner_vad,
            prefill_frames,
            hangover_frames,
            onset_frames,
            detection_gain,
            frame_buffer: VecDeque::new(),
            hangover_counter: 0,
            onset_counter: 0,
            in_speech: false,
            temp_out: Vec::new(),
            detect_buf: Vec::new(),
        }
    }
}

impl VoiceActivityDetector for SmoothedVad {
    fn push_frame<'a>(&'a mut self, frame: &'a [f32]) -> Result<VadFrame<'a>> {
        // 1. Buffer every incoming frame for possible pre-roll
        self.frame_buffer.push_back(frame.to_vec());
        while self.frame_buffer.len() > self.prefill_frames + 1 {
            self.frame_buffer.pop_front();
        }

        // 2. Delegate to the wrapped boolean VAD. When a detection gain is set,
        //    score a boosted COPY of the frame so quiet/whispered speech crosses
        //    the inner threshold — but keep `frame` itself (and everything we
        //    buffer/emit) at its original level to avoid clipping the recording.
        let is_voice = if self.detection_gain != 1.0 {
            self.detect_buf.clear();
            self.detect_buf.extend_from_slice(frame);
            for s in self.detect_buf.iter_mut() {
                *s = (*s * self.detection_gain).clamp(-1.0, 1.0);
            }
            self.inner_vad.is_voice(&self.detect_buf)?
        } else {
            self.inner_vad.is_voice(frame)?
        };

        match (self.in_speech, is_voice) {
            // Potential start of speech - need to accumulate onset frames
            (false, true) => {
                self.onset_counter += 1;
                if self.onset_counter >= self.onset_frames {
                    // We have enough consecutive voice frames to trigger speech
                    self.in_speech = true;
                    self.hangover_counter = self.hangover_frames;
                    self.onset_counter = 0; // Reset for next time

                    // Collect prefill + current frame
                    self.temp_out.clear();
                    for buf in &self.frame_buffer {
                        self.temp_out.extend(buf);
                    }
                    Ok(VadFrame::Speech(&self.temp_out))
                } else {
                    // Not enough frames yet, still silence
                    Ok(VadFrame::Noise)
                }
            }

            // Ongoing Speech
            (true, true) => {
                self.hangover_counter = self.hangover_frames;
                Ok(VadFrame::Speech(frame))
            }

            // End of Speech or interruption during onset phase
            (true, false) => {
                if self.hangover_counter > 0 {
                    self.hangover_counter -= 1;
                    Ok(VadFrame::Speech(frame))
                } else {
                    self.in_speech = false;
                    Ok(VadFrame::Noise)
                }
            }

            // Silence or broken onset sequence
            (false, false) => {
                self.onset_counter = 0; // Reset onset counter on silence
                Ok(VadFrame::Noise)
            }
        }
    }

    fn reset(&mut self) {
        self.frame_buffer.clear();
        self.hangover_counter = 0;
        self.onset_counter = 0;
        self.in_speech = false;
        self.temp_out.clear();
    }
}
