//! Pure parsers for the configurable whisper pipeline's free-text config
//! strings. Kept in their own public module (no app/tauri state) so the tuning
//! harness (`bin/tune.rs`) can reuse exactly the same parsing the live app uses
//! — what the tuner picks is what the app does.

use log::warn;
use transcribe_rs::whisper_cpp::WhisperInferenceParams;

/// Loudness floor applied to the buffer Whisper sees, in pipeline mode, after
/// any ffmpeg stage. -1.5 dBFS as a linear peak target.
pub const WHISPER_TARGET_PEAK: f32 = 0.841;

/// Scale the whole buffer so its loudest sample reaches `target_peak` (linear,
/// 0..1). No-op for silence or already-louder buffers — only ever amplifies.
/// Shared by the live transcription path and the tuning harness so they apply
/// identical preprocessing.
pub fn normalize_peak(samples: &mut [f32], target_peak: f32) {
    let peak = samples.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
    if peak <= 1e-6 || peak >= target_peak {
        return;
    }
    let gain = target_peak / peak;
    for s in samples.iter_mut() {
        *s = (*s * gain).clamp(-1.0, 1.0);
    }
}

/// Split a shell-like argument string into tokens, honoring single and double
/// quotes (so ffmpeg filtergraphs like `-af "loudnorm=I=-16:LRA=11"` stay one
/// token). Kept dependency-light; does not handle escapes or nested quoting,
/// which filtergraphs don't need.
pub fn split_shell_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has_token = false;
    for c in s.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    has_token = true;
                }
                c if c.is_whitespace() => {
                    if has_token {
                        out.push(std::mem::take(&mut cur));
                        has_token = false;
                    }
                }
                _ => {
                    cur.push(c);
                    has_token = true;
                }
            },
        }
    }
    if quote.is_some() {
        warn!("shell args: unterminated quote; treating the remainder as one token");
    }
    if has_token {
        out.push(cur);
    }
    out
}

/// Apply a whisper.cpp-style flag string onto `params`, in place. Recognized
/// flags map onto the fork's exposed `WhisperInferenceParams` fields:
///   `-mc N` / `--max-context N`  → `n_max_text_ctx`
///   `--entropy-thold F` / `-et`  → `entropy_thold`
/// Unknown flags are logged and ignored (never fail). Empty string is a no-op,
/// leaving whisper.cpp's own defaults in place.
///
/// To add a flag: expose the corresponding setter in the transcribe-rs fork's
/// `WhisperInferenceParams`, then add a match arm here.
pub fn parse_whisper_flags(s: &str, params: &mut WhisperInferenceParams) {
    let tokens = split_shell_args(s);
    let mut i = 0;
    while i < tokens.len() {
        let flag = tokens[i].clone();
        // The following token, if any, is this flag's candidate value. We only
        // consume it (advance by 2) when it actually parses, so a typo'd value
        // doesn't silently swallow the next flag.
        let value = tokens.get(i + 1);
        let mut consumed_value = false;
        match flag.as_str() {
            "-mc" | "--max-context" => match value.and_then(|v| v.parse::<i32>().ok()) {
                Some(v) => {
                    params.n_max_text_ctx = v;
                    consumed_value = true;
                }
                None => warn!("whisper flags: '{flag}' expected an integer value"),
            },
            "--entropy-thold" | "-et" => match value.and_then(|v| v.parse::<f32>().ok()) {
                Some(v) => {
                    params.entropy_thold = v;
                    consumed_value = true;
                }
                None => warn!("whisper flags: '{flag}' expected a float value"),
            },
            other => warn!("whisper flags: ignoring unrecognized flag '{other}'"),
        }
        i += if consumed_value { 2 } else { 1 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_keeps_quoted_filtergraph_together() {
        let args = split_shell_args(r#"-af "loudnorm=I=-16:LRA=11" -ar 16000"#);
        assert_eq!(args, vec!["-af", "loudnorm=I=-16:LRA=11", "-ar", "16000"]);
    }

    #[test]
    fn empty_string_splits_to_nothing() {
        assert!(split_shell_args("").is_empty());
        assert!(split_shell_args("   ").is_empty());
    }

    #[test]
    fn whisper_flags_set_known_fields() {
        let mut p = WhisperInferenceParams::default();
        parse_whisper_flags("-mc 0 --entropy-thold 2.8", &mut p);
        assert_eq!(p.n_max_text_ctx, 0);
        assert!((p.entropy_thold - 2.8).abs() < 1e-6);
    }

    #[test]
    fn whisper_flags_ignore_unknown_and_keep_defaults() {
        let def = WhisperInferenceParams::default();
        let mut p = WhisperInferenceParams::default();
        parse_whisper_flags("--bogus 5 -mc notanint", &mut p);
        // unknown flag ignored; bad value leaves the field at its default
        assert_eq!(p.n_max_text_ctx, def.n_max_text_ctx);
    }
}
