# Handoff: Configurable Whisper Pipeline

**Date:** 2026-06-26
**Branch:** `feat/darwin-flake-support` (handy repo, `~/git/handy`)
**Status:** Implementation complete, `nix build .#handy` **GREEN**, adversarially reviewed, findings fixed. **NOT yet installed, NOT committed.** The user will direct installation next.

---

## 1. Why this exists (the user's intent)

The user records **quiet whispered speech** through a high-quality headset mic over an XLR interface, and uses handy daily for real work. handy's VAD was gating out low-energy whisper frames, cutting recordings to 1–2s. A first attempt — a backend-only `whisper_mode` boolean with a fixed VAD preset — "did not work great," and there was no way to iterate on tuning without recompiling.

**The go-forward vision (this work):** turn tuning into a **configurable pipeline** exposed in the app as **a few free-text boxes** that read "like a shell invocation," so the user and assistant can iterate live on the best ffmpeg preprocessing string and whisper config. Everything sits behind **ONE master toggle**.

**The non-negotiable constraint (verbatim, "super key"):** when the master toggle is **off**, behavior must be **byte-for-byte identical to upstream** — instant bail-out for daily work. This invariant was verified by 3 independent reviewers and holds.

The long-term goal: grow the pipeline until it's good enough to leave on all the time, possibly promoting things to richer UI later. For now: maximal configurability via a couple of text boxes, not "30-40 buttons."

---

## 2. What was built

**One master toggle** `pipeline_enabled` (default OFF) gates everything. Under it, **three live free-text config stages** plus a **VAD on/off sub-toggle**:

| Stage | Setting | Applies | What it does |
|---|---|---|---|
| Master gate | `pipeline_enabled` (bool) | — | OFF = pure upstream. The bail-out. |
| 1a. VAD on/off | `pipeline_vad_enabled` (bool, default **true**) | live (recorder rebuild) | OFF = record **ungated** audio (no frames dropped) — for whispers the gate clips. |
| 1b. VAD/DSP tuning | `pipeline_vad_config` (string) | live (recorder rebuild) | `threshold=0.18 gain=12 prefill=20 hangover=25 onset=1` (gain in dB, detection-only; frame counts are 30ms each) |
| 2. ffmpeg preprocess | `pipeline_ffmpeg_args` (string) | live | middle of `ffmpeg -i IN <args> -ar 16000 -ac 1 OUT`; e.g. `-af loudnorm=I=-16:LRA=11:TP=-1.5`. The `-af` filtergraph absorbs all pre-filtering. |
| 3. Whisper decode | `pipeline_whisper_flags` (string) | live | whisper.cpp-style flags: `-mc 0 --entropy-thold 2.8` |

**Key design facts:**
- Stages 2 & 3 apply **live** (no restart) because `transcribe()` re-reads settings every call.
- Stage 1 (VAD) is cached in the recorder, so its setters call `invalidate_recorder()` to rebuild it live — no app restart, no "needs restart" pill.
- Empty config string for any stage = that stage uses defaults. So merely enabling the pipeline changes nothing until you type something.
- ffmpeg is **bundled into the .app** (`Contents/Resources/ffmpeg`) via the flake, resolved at runtime; falls back to PATH ffmpeg in `nix develop`/dev.

**Why VAD is a separate stage from ffmpeg/whisper:** VAD runs *live during capture* and decides which 30ms frames are even recorded — dropped frames are gone forever. ffmpeg/whisper run *after*, only on surviving audio. No ffmpeg string can recover gated-out audio, so the gate must be tunable (and defeatable) independently. The user's hypothesis is that the VAD gate is the main cause of whisper dropouts — hence the `pipeline_vad_enabled` kill-switch.

---

## 3. The tuning harness (`tune` bin) — the key deliverable

`src-tauri/src/audio_toolkit/bin/tune.rs`. The workflow the user wants:

> agree on a script → user records it whispering (externally, off the XLR) → harness sweeps configs against that one recording → ranks by WER → winner becomes the in-app defaults.

```sh
nix develop -c cargo run --bin tune -- <recording.wav> <script.txt> [--model <ggml.bin>]
```

It replays the WAV frame-by-frame through handy's **real** `SileroVad`/`SmoothedVad` (built from `parse_vad_config`) → optional ffmpeg → `normalize_peak` (matching the live path) → `WhisperEngine::transcribe_with` (with `parse_whisper_flags`), scores each config by word error rate vs the script, and prints a ranked table. Because it reuses the same parsers and code the app uses, **what the tuner picks is what the app does**.

- **Fixture requirement:** the recording must be the **raw, ungated** capture (recorded outside handy), because VAD tuning needs the audio the gate would otherwise discard. Any sample rate / channel count is accepted (downmixed + resampled to 16k); 8-bit WAV is rejected with a clear error.
- The sweep grid is in `build_sweep()` — deliberately small/explicit; widen it interactively once the landscape is visible.

**✅ The harness is BUILT and RUNS.** `nix build .#handy` compiles `tune` into the bundle at `Contents/MacOS/tune` (the earlier "can't compile locally" was only about `nix develop -c cargo` — the package build was always compiling it). It has been smoke-tested end-to-end on a real recording: all 18 sweep configs execute cleanly (VAD gate → bundled ffmpeg → whisper → WER rank), zero ffmpeg failures.

**How to run it:**
```sh
nix build .#handy
./result/Applications/Handy.app/Contents/MacOS/tune <recording.wav> <script.txt> --model <ggml.bin>
# model + vad model are auto-found if present under resources/models; --model overrides.
# A whisper model exists at: ~/Library/Application Support/com.pais.handy/models/ggml-large-v3-q5_0.bin
```
ffmpeg is resolved automatically: `$HANDY_FFMPEG` env override → bundled `Contents/Resources/ffmpeg` (next to the binary) → PATH.

---

## 4. Files changed (16 files, +1284/-63 vs HEAD)

**Rust (backend):**
- `src-tauri/src/settings.rs` — added `pipeline_enabled`, `pipeline_vad_enabled` (default true), `pipeline_vad_config`, `pipeline_ffmpeg_args`, `pipeline_whisper_flags`; default fns; `get_default_settings()` entries.
- `src-tauri/src/pipeline.rs` — **NEW** pure module: `split_shell_args`, `parse_whisper_flags`, `normalize_peak` + `WHISPER_TARGET_PEAK`. Shared by the live path and the tuner. Has unit tests.
- `src-tauri/src/audio_toolkit/vad/mod.rs` — `VadConfig` struct + `parse_vad_config` + unit tests.
- `src-tauri/src/managers/transcription.rs` — `run_ffmpeg_preprocess` helper; `transcribe()` rewired: ffmpeg + normalize_peak + whisper-flags all gated on `pipeline_enabled`; imports parsers from `pipeline`.
- `src-tauri/src/managers/audio.rs` — `create_audio_recorder` builds VAD from `parse_vad_config` (or attaches none when `pipeline_vad_enabled` is false); new `invalidate_recorder()` for live VAD changes.
- `src-tauri/src/commands/audio.rs` — 5 new `#[tauri::command]` setters (VAD-touching ones call `invalidate_recorder`).
- `src-tauri/src/lib.rs` — registered the 5 commands; added `pub mod pipeline`.
- `src-tauri/src/audio_toolkit/bin/tune.rs` — **NEW** tuning harness.
- `src-tauri/Cargo.toml` — `[[bin]] name = "tune"`.
- `flake.nix` — bundle `${pkgs.ffmpeg}/bin/ffmpeg` into `Handy.app/Contents/Resources/ffmpeg` (darwin installPhase); add ffmpeg to devShell.

**Frontend:**
- `src/bindings.ts` — **hand-edited** (5 command methods + 8 AppSettings fields). See §6 — normally codegen'd.
- `src/stores/settingsStore.ts` — 5 `settingUpdaters` entries.
- `src/components/settings/PipelineSettings.tsx` — **NEW** toggle + sub-toggle + 3 text boxes (commit on blur/Enter).
- `src/components/settings/advanced/AdvancedSettings.tsx` — mounts `<PipelineSettings>` in the Experimental group.
- `src/i18n/locales/en/translation.json` — keys under `settings.advanced.pipeline.*`.

**Docs/memory:**
- `DEV_SETUP.md` — **NEW** nix build/install/tccutil/settings-edit procedure (see §5).
- Memory: `~/.claude/.../memory/handy-build-install.md`, updated `handy-whisper-mode-plan.md`.

**Grounding documents for the next agent:**
- Plan: `~/.claude/plans/mellow-dazzling-thacker.md` (full spec, all phases + the VAD-toggle addendum).
- This file.
- `DEV_SETUP.md` in the repo.

---

## 5. How to build / install (the user will trigger install)

Full detail in `~/git/handy/DEV_SETUP.md`. Summary:

```sh
cd ~/git/handy
nix build .#handy            # GREEN as of this handoff; ./result has the .app
# Install (user will direct this):
osascript -e 'quit app "Handy"'        # quit FIRST; wait a few s past launch (Metal teardown crash)
chmod -R u+w /Applications/Handy.app 2>/dev/null || true
rm -rf /Applications/Handy.app
cp -R result/Applications/Handy.app /Applications/
chmod -R u+w /Applications/Handy.app
tccutil reset Accessibility com.pais.handy    # REQUIRED every rebuild (ad-hoc signing → new CDHash)
open /Applications/Handy.app
```

**Settings live at** `~/Library/Application Support/com.pais.handy/settings_store.json` (under `"settings"` key). Quit handy before editing. The pipeline can also be configured entirely through the UI: Settings → Advanced → enable "Experimental Features" → the "Configurable Whisper Pipeline" toggle and its text boxes appear.

**Nix gotcha learned:** flakes only see **git-tracked** files. New files must be `git add`-ed before `nix build` or they're invisible in the sandbox (this bit us twice). Everything is currently staged.

---

## 6. Build/test environment notes (resolved)

**RESOLVED — `tune` builds and runs.** The `nix build .#handy` package build compiles `tune` into the bundle (`Contents/MacOS/tune`) and it has been smoke-tested working (§3). The build.rs Apple-Intelligence Swift bridge is dead-code in the package build (its `#[cfg(aarch64)]` resolves false there), so the Swift issue never bites the authoritative build. The flake's `postFixup` now adds the onnxruntime rpath to `tune` too, so it runs directly from `./result`.

**Still open (minor): `nix develop -c cargo test` / `cargo build` fails** due to a pre-existing Swift toolchain/SDK version split in the dev shell (host CommandLineTools swiftc 6.2.4 vs nix apple-sdk 6.1.2 → build.rs's swiftc invocation fails). This only blocks in-shell cargo; it does NOT block `nix build`. The pure parser logic was validated by extracting it and running standalone via `rustc` (**ALL PARSER ASSERTIONS PASSED**). If you want `cargo test` in-shell, provide a matched swiftc+SDK pair (set `SWIFTC`/`SDKROOT`/`DEVELOPER_DIR` to a consistent toolchain, or add a swift to the devShell). Not on the critical path now that `tune` works via `nix build`.

**`src/bindings.ts` was hand-edited** because tauri-specta only regenerates it on a *debug* build, which the Swift issue blocks. The fleet verified the hand-edits exactly match the Rust command names/signatures and struct fields. If the next agent gets a debug build working, regenerate `bindings.ts` to be safe (it should be a no-op diff).

**Suggested next-step to unblock dev builds:** investigate making `nix develop` provide a swiftc matching the nix SDK (or set `SWIFTC`/`SDKROOT`/`DEVELOPER_DIR` to a matched pair), so `cargo test`/`cargo run --bin tune` work in-shell.

---

## 7. Adversarial review outcome (3 reviewers, grounded in the spec)

**Critical invariant — off-path = byte-for-byte upstream — VERIFIED by all three.** Every new path is gated behind `pipeline_enabled`; when off, VAD = `VadConfig::default()` (matches the original `DEFAULT_VAD_*` constants exactly), decode params = the fork's `-1`/`2.4` defaults, ffmpeg/normalize never run. `whisper_mode` is now dead (read by nothing).

**No CRITICAL or compile-blocking issues found.** Findings addressed:
- **[HIGH] Tuner didn't apply `normalize_peak`** → its WER would mislead for quiet audio. **Fixed:** moved `normalize_peak`/`WHISPER_TARGET_PEAK` into shared `pipeline` module; both live path and tuner call it.
- **[HIGH] Tuner didn't pad short buffers** like the live recorder. **Fixed:** added `pad_short_buffer` mirroring `stop_recording` (< 1s → 1.25s).
- **[MED] `invalidate_recorder` lazy-close race** → **Fixed:** bump `close_generation` unconditionally.
- **[MED] 8-bit WAV silent-wrong** in tuner → **Fixed:** rejected with clear error.
- **[MED] tuner default model path** wouldn't be found from repo root → **Fixed:** `find_default_whisper_model` multi-path probe.
- **[LOW] `parse_whisper_flags` could swallow next flag** on bad value → **Fixed:** only consume value when it parses.
- **[LOW] `split_shell_args` silent on unterminated quote** → **Fixed:** warns.
- **[LOW, not fixed — intentional] dead `whisper_mode`/`whisper_max_text_ctx`/`whisper_entropy_thold` fields** remain (harmless; spec allowed keeping them). Candidate for cleanup once confident.

Rebuilt after fixes: `nix build .#handy` **GREEN**, ffmpeg confirmed bundled and runnable (v8.0.1).

---

## 8. Recommended next steps (in order)

1. **Install** the green build (user will direct) per §5; confirm Accessibility + launch.
2. **Sanity-check the off-path:** with the pipeline toggle OFF, confirm normal dictation is unchanged (the daily-driver bail-out).
3. **Agree on a test script** with the user; have them record it whispering (raw, off the XLR) → get `recording.wav` + `script.txt`.
4. **Unblock dev builds** (§6) so `tune` compiles, then run the sweep to find a good starting config.
5. **Iterate live in the app:** paste candidate strings into the three boxes, test on real whispers. The prime hypothesis to test first: **turn the VAD off** (`pipeline_vad_enabled` = false) and lean on ffmpeg `loudnorm` + `-mc 0` — see if dropouts disappear.
6. **Bake the winning config** as the `default_*` values in `settings.rs` once validated.
7. Consider committing (nothing is committed yet beyond the two Phase-0 nix commits `2846ac6`, `7db0ec3`).

## 9. Pending / deferred
- Decode-flag vocabulary is small (`-mc`, `--entropy-thold`) — extending requires adding setters to the transcribe-rs fork (`iwixon/transcribe-rs@bf555a6`, branch `handy-decode-params`) then a match arm in `pipeline::parse_whisper_flags`.
- ggml Metal teardown crash on quit is pre-existing/upstream (don't quit within first few seconds).
- ffmpeg closure size in the .app (many codecs) — acceptable; could slim with a custom ffmpeg build later.
