# Oga Release Checklist

## One-time setup

- Install `cargo-tauri`, `gh`, `op`, and Xcode Command Line Tools.
- Install both Rust targets: `rustup target add aarch64-apple-darwin x86_64-apple-darwin`.
- Import the Developer ID Application certificate into the login keychain.
- Create `Oga-signing` in the `DesgnSpace` 1Password vault.
- Add the fields listed in `.env.1password`, including `TAURI_PUBLIC_KEY`.
- Confirm `gh auth status` and `op whoami` succeed.

## Release

1. Start from a clean worktree.
2. Run `make publish fix`, or choose `major` or `minor`.
3. Confirm the version prompt.
4. Check the GitHub release contains the app, DMG, updater archive, signature, and `latest.json`.

The release uses a mounted `.env` first. Without it, `op run` resolves
`.env.1password` live. Never commit `.env` or print secret values.

## Verification

The publish script builds a universal macOS bundle, signs the sidecar and app
with the hardened runtime, notarizes and staples both app and DMG, validates
the tickets, verifies the app signature, and only then tags and uploads.
