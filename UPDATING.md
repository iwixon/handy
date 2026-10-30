# Updating Handy from Source (macOS / Nix)

## Quick Update

```bash
cd ~/git/handy
git pull
nix build .#handy --out-link /tmp/handy-test
```

This keeps the new bundle separate from an installed Handy build. The bundle is at `/tmp/handy-test/Applications/Handy.app`.

## Notes

- If `nix build` fails after an update, try `nix flake update` first — upstream dependency changes may require newer nixpkgs.
- If JS dependencies change (new entries in `bun.lock`), regenerate the Nix lockfile:
  ```bash
  bunx bun2nix -o .nix/bun.nix
  ```
- If Cargo dependencies change (new git deps in `Cargo.lock`), the flake handles this automatically via `allowBuiltinFetchGit`.
- Quit Handy before copying to `/Applications/` to avoid file-in-use issues.
