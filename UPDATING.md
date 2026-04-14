# Updating Handy from Source (macOS / Nix)

## Quick Update

```bash
cd ~/git/handy
git pull
nix build .#handy
cp -r result/Applications/Handy.app /Applications/
```

## Notes

- If `nix build` fails after an update, try `nix flake update` first — upstream dependency changes may require newer nixpkgs.
- The VAD model (`src-tauri/resources/models/silero_vad_v4.onnx`) is bundled into the app at build time. If the upstream project changes it, re-download:
  ```bash
  curl -o src-tauri/resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx
  ```
- If JS dependencies change (new entries in `bun.lock`), regenerate the Nix lockfile:
  ```bash
  bunx bun2nix -o .nix/bun.nix
  ```
- If Cargo dependencies change (new git deps in `Cargo.lock`), the flake handles this automatically via `allowBuiltinFetchGit`.
- Quit Handy before copying to `/Applications/` to avoid file-in-use issues.
