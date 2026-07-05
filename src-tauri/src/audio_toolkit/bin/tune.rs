//! Pipeline tuning harness.
//!
//! Replays a recorded WAV through handy's *real* capture + transcription code
//! for a grid of pipeline configs, scores each against a ground-truth script by
//! word error rate (WER), and prints a ranked table. The winning config's three
//! strings can be pasted straight into the app's pipeline text boxes (they use
//! the same parsers) and then baked in as defaults.
//!
//! Usage:
//!   tune <recording.wav> <script.txt> [--model <path-to-ggml.bin>]
//!
//! The recording should be the RAW, ungated capture of the script being
//! whispered (recorded outside handy, e.g. straight off the interface), because
//! VAD tuning needs the audio the gate would otherwise have discarded.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hound::WavReader;
use transcribe_rs::whisper_cpp::{WhisperEngine, WhisperInferenceParams};

use handy_app_lib::audio_toolkit::{
    audio::FrameResampler,
    constants,
    vad::{parse_vad_config, SileroVad, SmoothedVad, VadConfig, VoiceActivityDetector},
};
use handy_app_lib::pipeline::{
    normalize_peak, parse_whisper_flags, split_shell_args, WHISPER_TARGET_PEAK,
};

const FRAME_MS: u64 = 30;

/// One point in the sweep: the pipeline config. `vad_enabled` mirrors the app's
/// `pipeline_vad_enabled` master sub-toggle — when false the capture gate is
/// bypassed entirely (ungated audio), and `vad` config is irrelevant.
#[derive(Clone)]
struct Candidate {
    vad_enabled: bool,
    vad: String,
    ffmpeg: String,
    whisper: String,
}

struct Scored {
    cand: Candidate,
    wer: f64,
    captured_pct: f64,
    hypothesis: String,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage: {} <recording.wav> <script.txt> [--model <ggml.bin>]",
            args.first().map(String::as_str).unwrap_or("tune")
        );
        std::process::exit(2);
    }
    let wav_path = PathBuf::from(&args[1]);
    let script_path = PathBuf::from(&args[2]);
    let model_path = parse_model_arg(&args)
        .or_else(find_default_whisper_model)
        .unwrap_or_else(|| {
            fatal("no whisper model found; pass --model <path-to-ggml.bin> (e.g. a downloaded ggml-*.bin)")
        });

    let reference = std::fs::read_to_string(&script_path)
        .unwrap_or_else(|e| fatal(&format!("failed to read script {script_path:?}: {e}")));
    let reference = normalize_text(&reference);

    println!("Loading recording {wav_path:?} ...");
    let raw_16k = load_wav_16k_mono(&wav_path)
        .unwrap_or_else(|e| fatal(&format!("failed to load WAV: {e}")));
    println!(
        "  {} samples @ 16kHz ({:.1}s)",
        raw_16k.len(),
        raw_16k.len() as f64 / constants::WHISPER_SAMPLE_RATE as f64
    );

    let vad_model = find_vad_model().unwrap_or_else(|| {
        fatal("could not locate silero_vad_v4.onnx (run from repo root or set CWD to a build with resources/models/)")
    });

    println!("Loading whisper model {model_path:?} ...");
    let mut engine = WhisperEngine::load(&model_path)
        .unwrap_or_else(|e| fatal(&format!("failed to load whisper model: {e}")));

    let candidates = build_sweep();
    println!("Sweeping {} candidate configs ...\n", candidates.len());

    let mut results: Vec<Scored> = Vec::new();
    for (i, cand) in candidates.iter().enumerate() {
        // 1. Capture/VAD stage. When vad_enabled is false, bypass the gate
        // entirely (ungated audio) exactly like the app's pipeline_vad_enabled
        // = false path; otherwise gate with this candidate's VAD config.
        let gated = if cand.vad_enabled {
            gate_with_vad(&raw_16k, &vad_model, parse_vad_config(&cand.vad))
        } else {
            raw_16k.clone()
        };
        // Ratio of emitted (kept) samples to input. Can slightly exceed 100%
        // because VAD onset latches re-emit buffered prefill frames; that's
        // expected, so we report it as "emitted%" rather than implying a cap.
        let captured_pct = if raw_16k.is_empty() {
            0.0
        } else {
            100.0 * gated.len() as f64 / raw_16k.len() as f64
        };

        // Mirror the live recorder: pad very short buffers (< 1s) up to 1.25s,
        // matching AudioRecordingManager::stop_recording so the tuner's whisper
        // input matches the app's for short clips.
        let gated = pad_short_buffer(gated);

        // 2. ffmpeg preprocess stage (optional).
        let mut processed = if cand.ffmpeg.trim().is_empty() {
            gated
        } else {
            match run_ffmpeg(&gated, &cand.ffmpeg) {
                Some(p) => p,
                None => {
                    eprintln!("  [{i}] ffmpeg failed; using gated audio");
                    gated
                }
            }
        };
        // Mirror the live transcribe() loudness floor (applied in pipeline mode
        // after ffmpeg) so tuner WER reflects what the app actually feeds whisper.
        normalize_peak(&mut processed, WHISPER_TARGET_PEAK);

        // 3. Whisper decode stage.
        let mut params = WhisperInferenceParams::default();
        parse_whisper_flags(&cand.whisper, &mut params);
        let hypothesis = match engine.transcribe_with(&processed, &params) {
            Ok(r) => normalize_text(&r.text),
            Err(e) => {
                eprintln!("  [{i}] transcription failed: {e}");
                String::new()
            }
        };

        let wer = word_error_rate(&reference, &hypothesis);
        let vad_desc = if cand.vad_enabled {
            if cand.vad.is_empty() { "on(default)".to_string() } else { format!("on[{}]", cand.vad) }
        } else {
            "OFF".to_string()
        };
        println!(
            "  [{i}] WER {:.1}%  captured {:.0}%  vad={vad_desc}  ffmpeg='{}' whisper='{}'",
            wer * 100.0,
            captured_pct,
            cand.ffmpeg,
            cand.whisper
        );
        results.push(Scored {
            cand: cand.clone(),
            wer,
            captured_pct,
            hypothesis,
        });
    }

    results.sort_by(|a, b| a.wer.partial_cmp(&b.wer).unwrap_or(std::cmp::Ordering::Equal));

    println!("\n══════════════ Ranked results (best first) ══════════════");
    for (rank, s) in results.iter().enumerate() {
        println!(
            "{:>2}. WER {:.1}%  captured {:.0}%",
            rank + 1,
            s.wer * 100.0,
            s.captured_pct
        );
        println!("    vad     : {}", if s.cand.vad_enabled {
            if s.cand.vad.is_empty() { "on (default)".to_string() } else { format!("on — {}", s.cand.vad) }
        } else { "OFF (ungated)".to_string() });
        println!("    ffmpeg  : {}", s.cand.ffmpeg);
        println!("    whisper : {}", s.cand.whisper);
        println!("    text    : {}", truncate(&s.hypothesis, 100));
    }
    if let Some(best) = results.first() {
        println!("\nBest config — set in the app's pipeline settings:");
        println!("  VAD enabled   : {}", best.cand.vad_enabled);
        println!("  VAD config    : {}", best.cand.vad);
        println!("  ffmpeg args   : {}", best.cand.ffmpeg);
        println!("  whisper flags : {}", best.cand.whisper);
    }
}

/// The sweep grid. Deliberately small/explicit; widen once the landscape is
/// visible. Sweeps VAD OFF (ungated) as well as several VAD-ON configs — the
/// central question is whether bypassing the gate beats tuning it for quiet
/// whispered speech. Each axis is a list of config-string fragments.
fn build_sweep() -> Vec<Candidate> {
    // VAD-on tuning candidates (only meaningful when the gate runs).
    let vad_configs = [
        "",                                                   // upstream default (0.3/15/15/2)
        "threshold=0.18 gain=12 prefill=20 hangover=25 onset=1",
        "threshold=0.12 gain=18 prefill=20 hangover=30 onset=1",
    ];
    let ffmpeg_configs = [
        "",                                                   // no preprocessing
        "-af loudnorm=I=-16:LRA=11:TP=-1.5",
        "-af highpass=f=80,loudnorm=I=-16:LRA=11:TP=-1.5",
    ];
    let whisper_configs = ["", "-mc 0 --entropy-thold 2.8"];

    let mut out = Vec::new();
    // VAD OFF: gate bypassed; vad config string is irrelevant (left empty).
    for f in ffmpeg_configs {
        for w in whisper_configs {
            out.push(Candidate {
                vad_enabled: false,
                vad: String::new(),
                ffmpeg: f.to_string(),
                whisper: w.to_string(),
            });
        }
    }
    // VAD ON: each tuning config × ffmpeg × whisper.
    for v in vad_configs {
        for f in ffmpeg_configs {
            for w in whisper_configs {
                out.push(Candidate {
                    vad_enabled: true,
                    vad: v.to_string(),
                    ffmpeg: f.to_string(),
                    whisper: w.to_string(),
                });
            }
        }
    }
    out
}

/// Feed `audio` (16kHz mono) through a SmoothedVad built exactly as the live
/// recorder builds it, returning only the frames the VAD keeps. This mirrors
/// `recorder::handle_frame`: 30ms frames, push_frame, collect Speech buffers.
fn gate_with_vad(audio: &[f32], vad_model: &Path, cfg: VadConfig) -> Vec<f32> {
    let silero = match SileroVad::new(vad_model, cfg.threshold) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  VAD init failed ({e}); passing audio through ungated");
            return audio.to_vec();
        }
    };
    let mut vad = SmoothedVad::with_detection_gain(
        Box::new(silero),
        cfg.prefill,
        cfg.hangover,
        cfg.onset,
        cfg.detection_gain(),
    );

    // The recorder runs input through a FrameResampler to emit fixed 30ms
    // frames; the source here is already 16kHz (identity rate) but we reuse it
    // for identical framing. Collect the frames first, then run the VAD over
    // them sequentially (the VAD holds borrow-conflicting mutable state, so we
    // can't drive it from two separate resampler callbacks).
    let mut resampler = FrameResampler::new(
        constants::WHISPER_SAMPLE_RATE as usize,
        constants::WHISPER_SAMPLE_RATE as usize,
        Duration::from_millis(FRAME_MS),
    );
    let mut frames: Vec<Vec<f32>> = Vec::new();
    resampler.push(audio, &mut |frame: &[f32]| frames.push(frame.to_vec()));
    resampler.finish(&mut |frame: &[f32]| frames.push(frame.to_vec()));

    use handy_app_lib::audio_toolkit::vad::VadFrame;
    let mut out = Vec::new();
    for frame in &frames {
        match vad
            .push_frame(frame)
            .unwrap_or(VadFrame::Speech(frame))
        {
            VadFrame::Speech(buf) => out.extend_from_slice(buf),
            VadFrame::Noise => {}
        }
    }
    out
}

/// Run an ffmpeg preprocessing string over 16kHz mono `audio`, returning the
/// processed samples (re-resampled to 16k mono by ffmpeg). Returns None on any
/// failure.
fn run_ffmpeg(audio: &[f32], user_args: &str) -> Option<Vec<f32>> {
    use handy_app_lib::audio_toolkit::{read_wav_samples, save_wav_file};

    let tmp = std::env::temp_dir();
    let pid = std::process::id();
    let in_path = tmp.join(format!("handy-tune-in-{pid}.wav"));
    let out_path = tmp.join(format!("handy-tune-out-{pid}.wav"));
    let cleanup = || {
        let _ = std::fs::remove_file(&in_path);
        let _ = std::fs::remove_file(&out_path);
    };

    if save_wav_file(&in_path, audio).is_err() {
        cleanup();
        return None;
    }
    let mut cmd = std::process::Command::new(resolve_ffmpeg());
    cmd.arg("-y").arg("-i").arg(&in_path);
    cmd.args(split_shell_args(user_args));
    cmd.args(["-ar", "16000", "-ac", "1"]);
    cmd.arg(&out_path);
    let ok = cmd
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let result = if ok {
        read_wav_samples(&out_path).ok()
    } else {
        None
    };
    cleanup();
    result
}

/// Locate the ffmpeg binary, mirroring how the app resolves it so the tuner
/// matches production: (1) `$HANDY_FFMPEG` override, (2) the `ffmpeg` bundled
/// next to this binary at `Contents/Resources/ffmpeg` (when run from the .app),
/// (3) plain `ffmpeg` from PATH (the nix dev shell). Returns a path/name to
/// hand to Command::new.
fn resolve_ffmpeg() -> PathBuf {
    if let Ok(p) = std::env::var("HANDY_FFMPEG") {
        let p = PathBuf::from(p);
        if p.exists() {
            return p;
        }
    }
    // current_exe() is .../Handy.app/Contents/MacOS/tune; the bundled ffmpeg
    // lives at .../Handy.app/Contents/Resources/ffmpeg.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            let bundled = macos_dir
                .join("../Resources/ffmpeg")
                .canonicalize()
                .unwrap_or_else(|_| macos_dir.join("../Resources/ffmpeg"));
            if bundled.exists() {
                return bundled;
            }
        }
    }
    PathBuf::from("ffmpeg")
}

/// Load any WAV as 16kHz mono f32, downmixing channels and resampling as needed.
fn load_wav_16k_mono(path: &Path) -> anyhow::Result<Vec<f32>> {
    let mut reader = WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;

    // Read interleaved samples as f32 regardless of bit depth / format.
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            // 8-bit WAV is unsigned (0..255, centered at 128); the signed-scale
            // path below would mangle it. Reject rather than silently misread —
            // interface captures are 16/24-bit anyway.
            if spec.bits_per_sample <= 8 {
                anyhow::bail!(
                    "8-bit WAV not supported; re-export as 16-bit PCM ({path:?})",
                    path = path
                );
            }
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()?
        }
    };

    // Downmix to mono.
    let mono: Vec<f32> = if channels <= 1 {
        interleaved
    } else {
        interleaved
            .chunks(channels)
            .map(|f| f.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    // Resample to 16kHz if needed.
    if spec.sample_rate as usize == constants::WHISPER_SAMPLE_RATE as usize {
        return Ok(mono);
    }
    let mut resampler = FrameResampler::new(
        spec.sample_rate as usize,
        constants::WHISPER_SAMPLE_RATE as usize,
        Duration::from_millis(FRAME_MS),
    );
    let mut out = Vec::new();
    resampler.push(&mono, &mut |frame: &[f32]| out.extend_from_slice(frame));
    resampler.finish(&mut |frame: &[f32]| out.extend_from_slice(frame));
    Ok(out)
}

/// Mirror `AudioRecordingManager::stop_recording`: a non-empty buffer shorter
/// than 1s is zero-padded to 1.25s. Keeps tuner whisper input identical to the
/// app's for short clips.
fn pad_short_buffer(mut samples: Vec<f32>) -> Vec<f32> {
    let sr = constants::WHISPER_SAMPLE_RATE as usize;
    if !samples.is_empty() && samples.len() < sr {
        samples.resize(sr * 5 / 4, 0.0);
    }
    samples
}

/// Lowercase, strip punctuation, collapse whitespace — so WER measures words.
fn normalize_text(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Word error rate = word-level Levenshtein(ref, hyp) / ref word count.
fn word_error_rate(reference: &str, hypothesis: &str) -> f64 {
    let ref_words: Vec<&str> = reference.split_whitespace().collect();
    let hyp_words: Vec<&str> = hypothesis.split_whitespace().collect();
    if ref_words.is_empty() {
        return if hyp_words.is_empty() { 0.0 } else { 1.0 };
    }
    let dist = word_levenshtein(&ref_words, &hyp_words);
    dist as f64 / ref_words.len() as f64
}

/// Standard edit distance over word tokens (classic DP).
fn word_levenshtein(a: &[&str], b: &[&str]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, wa) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, wb) in b.iter().enumerate() {
            let cost = if wa == wb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn parse_model_arg(args: &[String]) -> Option<PathBuf> {
    args.iter()
        .position(|a| a == "--model")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

fn find_vad_model() -> Option<PathBuf> {
    // `$HANDY_VAD_MODEL` override wins (lets the tuner run from any CWD).
    if let Ok(p) = std::env::var("HANDY_VAD_MODEL") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let candidates = [
        "./resources/models/silero_vad_v4.onnx",
        "./src-tauri/resources/models/silero_vad_v4.onnx",
        "../resources/models/silero_vad_v4.onnx",
    ];
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

/// Probe the common dev locations for a bundled ggml whisper model, trying the
/// same path roots as the VAD probe (run-from-repo-root or run-from-src-tauri).
fn find_default_whisper_model() -> Option<PathBuf> {
    let names = ["ggml-small.bin", "ggml-base.bin", "ggml-medium.bin"];
    let roots = [
        "./resources/models",
        "./src-tauri/resources/models",
        "../resources/models",
    ];
    roots
        .iter()
        .flat_map(|r| names.iter().map(move |n| PathBuf::from(r).join(n)))
        .find(|p| p.exists())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('…');
        t
    }
}

fn fatal(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}
