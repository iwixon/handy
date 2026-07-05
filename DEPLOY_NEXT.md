# Next agent: stabilize the pipeline for everyday use

**Goal:** take the green build + the tuning conclusion below and make it the user's daily-driver
config on this machine — installed, settings applied, verified, and resilient across rebuilds.

## The tuning conclusion (what to deploy)

There are TWO problems to solve, and the config below solves BOTH:
- **(A) Quiet whisper:** the VAD gate drops 42–58% of soft audio → garbled transcripts.
- **(B) Long recordings collapse:** whisper.cpp's autoregressive decoder degenerates into a
  repeated phrase at end-of-file on multi-minute audio, dropping the real ending (documented at
  `~/git/long-whisper-decode/README.md`).

We swept 4 real XLR whisper recordings (short/long × 2 gain levels) × 30 configs each with the
`tune` harness. Full data: `~/git/long-whisper-decode/tuning/RESULTS.md`. And we reproduced the
long-recording collapse + its fix on the real 11-min file (`~/Documents/handy-1782318147.wav`)
with the exact model handy uses (large-v3-q5_0). Headline:

- **handy's default VAD discards 42–58% of short whispered clips → ~50% WER.** VAD OFF recovers
  them → ~3% WER. This solves (A).
- **Default decode context collapses the 11-min file** into "...we're going to do this for a
  while." ×4, losing the real ending. **`-mc 0 --entropy-thold 2.8` removes the loop and recovers
  the ending.** This solves (B). It is **WER-neutral on the short/quiet clips** (3.2% with or
  without), so it is now part of the BASELINE config, not a fallback.
- ffmpeg `loudnorm`/`highpass` barely moved WER on this clean XLR signal — leave empty.

**Target everyday config (handles BOTH quiet whisper AND long recordings):**
```
pipeline_enabled       = true
pipeline_vad_enabled   = false                       ← (A) quiet-whisper fix: bypass capture gate
pipeline_vad_config    = ""                          (irrelevant when VAD off)
pipeline_ffmpeg_args   = ""                          (loudnorm was a no-op on clean XLR)
pipeline_whisper_flags = "-mc 0 --entropy-thold 2.8" ← (B) long-recording collapse fix
```

**Tradeoff to keep in mind:** VAD off means handy records the *entire* hold-to-talk window
including silence/noise. Cost-free on this clean XLR path (it even helped). If a noisier mic is
used later, the fallback is tuned-VAD-on: `pipeline_vad_enabled=true`,
`pipeline_vad_config="threshold=0.18 gain=12 prefill=20 hangover=25 onset=1"`,
`pipeline_ffmpeg_args="-af highpass=f=80,loudnorm=I=-16:LRA=11:TP=-1.5"` (won long_gainA at 1.4%).

## How to apply the settings (two ways)

Settings live at `~/Library/Application Support/com.pais.handy/settings_store.json`, nested under
the top-level `"settings"` key. **Quit handy before editing** (it rewrites on exit). The file
currently predates the pipeline keys (only `whisper_mode` present); serde defaults fill the rest,
so you must add the pipeline keys explicitly.

**Option A — UI (preferred, no file editing):** launch handy → Settings → Advanced → enable
"Experimental Features" → the "Configurable Whisper Pipeline" group appears → toggle the pipeline
ON, then toggle **Voice Activity Detection (VAD) OFF**. The VAD toggle calls `invalidate_recorder`
so it applies on the next recording, no restart.

In the UI also set the **whisper flags** box to `-mc 0 --entropy-thold 2.8` (the long-recording fix).

**Option B — JSON (scriptable):** quit handy, back up, then set under `"settings"`:
`"pipeline_enabled": true, "pipeline_vad_enabled": false, "pipeline_vad_config": "",
"pipeline_ffmpeg_args": "", "pipeline_whisper_flags": "-mc 0 --entropy-thold 2.8"`. Relaunch.

## Install procedure (every rebuild)

Full detail: `~/git/handy/DEV_SETUP.md`. Critical bits:
```sh
cd ~/git/handy && nix build .#handy           # GREEN as of handoff
osascript -e 'quit app "Handy"'               # quit FIRST; wait a few s (Metal teardown crash)
chmod -R u+w /Applications/Handy.app 2>/dev/null || true
rm -rf /Applications/Handy.app
cp -R result/Applications/Handy.app /Applications/
chmod -R u+w /Applications/Handy.app
tccutil reset Accessibility com.pais.handy    # REQUIRED every rebuild (ad-hoc signing → new CDHash)
open /Applications/Handy.app
```

## Verification checklist for "fit for everyday use"
1. **Off-path sanity:** with pipeline OFF, confirm normal dictation is unchanged (the bail-out).
2. **Apply target config** (VAD off) via UI or JSON; record a real quiet whisper, confirm it's
   captured in full and transcribes cleanly (no 1–2s cutoff).
3. **Long dictation:** ramble several minutes with pauses; confirm no early cutoff and no
   end-of-file repetition (the `-mc 0 --entropy-thold 2.8` flag is what prevents it — already in
   the baseline config above). Reproduced + fixed on the 11-min file via whisper.cpp CLI.
4. **Accessibility/typing** works after the rebuild (tccutil reset done).
5. **Survives a restart:** quit + relaunch handy, confirm the config persists and still works.

## State of the code (nothing committed yet)
- Branch `feat/darwin-flake-support`; all pipeline work is **staged but uncommitted** (only the
  two Phase-0 nix commits `2846ac6`, `7db0ec3` are committed). Consider committing once stable.
- `nix build .#handy` GREEN; `tune` binary built + working (smoke + real runs done).
- Open env nit: `nix develop -c cargo test` fails on a Swift toolchain/SDK split (does NOT block
  `nix build`). Parsers were validated standalone via rustc. See `PIPELINE_HANDOFF.md` §6.

## Grounding documents
- `~/git/handy/PIPELINE_HANDOFF.md` — full implementation handoff (what was built, files, review).
- `~/git/long-whisper-decode/tuning/RESULTS.md` — the tuning data + recommendation.
- `~/git/long-whisper-decode/tuning/SCRIPTS.md`, `short.txt`, `long.txt`, `*.wav`, `*.out` — fixtures + raw sweeps.
- `~/git/handy/DEV_SETUP.md` — build/install/tccutil/settings procedure.
- `~/.claude/plans/mellow-dazzling-thacker.md` — the original spec/plan.
