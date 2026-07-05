# Handy dev / build / install (nix, macOS)

This fork builds and runs **entirely via nix**. `cargo`/`bun`/`ffmpeg` are NOT on the host PATH;
they only exist inside `nix develop`. The whole build environment lives in `flake.nix`.

## Build

```sh
cd ~/git/handy
nix build .#handy          # produces ./result (Handy.app under result/Applications)
```

Fast iteration without a full bundle:

```sh
nix develop -c bun run tauri dev      # dev build, hot-reload UI, regenerates src/bindings.ts
nix develop -c bash -c 'cd src-tauri && cargo build'   # rust-only compile check
nix develop -c bash -c 'cd src-tauri && cargo clippy'
```

`src/bindings.ts` (tauri-specta) is regenerated automatically on a **debug** build and is checked
in — run a dev build after changing any `#[tauri::command]`/`AppSettings` field.

## Install over the existing /Applications copy

The old bundle's files lack the owner-write bit (nix store perms), so chmod before removing:

```sh
# 1. Quit Handy FIRST. Wait several seconds after launch before quitting any build —
#    quitting during ggml/Metal init can hit a teardown crash (ggml_metal_rsets_free).
osascript -e 'quit app "Handy"'   # or quit from the tray

# 2. Replace the bundle
chmod -R u+w /Applications/Handy.app 2>/dev/null || true
rm -rf /Applications/Handy.app
cp -R result/Applications/Handy.app /Applications/
chmod -R u+w /Applications/Handy.app

# 3. Re-grant Accessibility — REQUIRED after EVERY rebuild.
#    Ad-hoc/linker signing changes the CDHash each build, so macOS treats it as a new app
#    and the stale TCC grant leaves it stuck on the "Accessibility Access Required" screen.
tccutil reset Accessibility com.pais.handy
# If still stuck on first-boot: tccutil reset All com.pais.handy

# 4. Launch
open /Applications/Handy.app
```

## Editing settings directly

Settings persist at `~/Library/Application Support/com.pais.handy/settings_store.json`, nested
under a top-level `"settings"` key. **Quit Handy before editing** — it rewrites the file on exit
and will clobber your changes. Back up with `cp settings_store.json settings_store.json.bak`.

## Known issue: quit-time crash

`EXC_CRASH/SIGABRT` in `ggml_metal_rsets_free` on quit is a ggml Metal residency-set teardown race
(init still running on a worker thread while the main thread frees the Metal device at exit). It is
upstream / not our code. Mitigation: don't quit within the first few seconds of launch.
