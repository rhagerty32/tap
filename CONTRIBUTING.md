# Contributing to Tap

Thanks for your interest in Tap. This guide covers how to build, run, and test the project locally and how we handle pull requests.

## What you'll need

- [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS (GTK/WebKit on Linux, Xcode toolchain on macOS).
- **Node.js 22+** and npm.
- **Rust stable** (the repo pins `stable` in `rust-toolchain.toml`; `rustup` will install it automatically).

On Ubuntu or other Debian-based Linux desktops, CI installs these packages before building:

```bash
sudo apt-get update
sudo apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf libssl-dev
```

For running the built app (not required to compile), tray support may need `libayatana-appindicator3-1` and `libwebkit2gtk-4.1-0`.

## Build and run locally

```bash
npm ci
npm run tauri dev
```

That starts the Vite frontend and the Tauri shell. Use the menu bar / system tray icon to open settings and the team list.

Frontend-only iteration (no Rust reload):

```bash
npm run dev
```

## Tests and checks

Run the Rust test suite (same as CI's test step):

```bash
npm test
```

Equivalent command:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Before opening a PR, it's helpful to mirror CI locally when you can:

```bash
npm ci
npm audit --audit-level=high
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

CI also runs `cargo audit` (non-blocking) and Tauri builds on Ubuntu and macOS.

## Branch and pull request flow

1. Fork the repository (or branch from `main` if you have write access).
2. Create a focused branch for your change.
3. Keep commits small and descriptive.
4. Open a pull request against `main` and fill out the PR template.
5. Ensure CI is green before merge.

Bug fixes and documentation improvements are always welcome. For new features or larger refactors, open an issue first so we can agree on approach—especially for networking, overlay, or platform-specific behavior.

## Code style

- **Rust:** Follow existing patterns in `src-tauri/src/`. Run `cargo fmt` and `cargo clippy` when you touch Rust code.
- **TypeScript:** The frontend uses TypeScript with Vite. Match the style in `src/` (minimal dependencies, direct DOM where the project already does).
- **No secrets in the repo:** Team keys live in local settings only (`~/.config/tap/settings.json` on Linux). Never commit keys, signing material, or Apple/Tauri updater private keys.

## Security

If you believe you've found a security issue, please open a GitHub issue with details rather than posting exploit material in public threads. Do not commit real team keys or credentials.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Be kind and constructive in issues and reviews.

## Release maintainers

Tagged releases and signing keys are documented in the README under **Cut a release**. Contributors do not need those steps for ordinary development.
