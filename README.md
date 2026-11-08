# Tap

**Press and hold a teammate in the menu bar to flash a click-through glow on their screens—built for offices where people work in headphones.**

[![CI](https://github.com/rhagerty32/tap/actions/workflows/ci.yml/badge.svg)](https://github.com/rhagerty32/tap/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/github/license/rhagerty32/tap)](LICENSE)

Tap is a small cross-platform desktop app for getting a coworker's attention when they are wearing headphones. You press-and-hold a peer in the menu bar / system tray popover; while you hold, the peer's screens show a click-through glowing border and banner. Releasing stops the signal immediately.

Built with **Tauri v2** (Rust core + lightweight TypeScript UI).

## Screenshot

<!-- Replace with a GIF or screenshot when you have one (overlay + tray list). -->
> **Placeholder:** Add something like `docs/images/tap-demo.gif` showing the receiver overlay and the tray team list.

## Features

- **Press-and-hold signaling** from the tray popover (pointer down/up—not a native menu item).
- **Live receiver overlay** on every display: transparent, always-on-top, click-through, no focus steal.
- **Peer-to-peer UDP** on LAN or Tailscale; no central server or accounts.
- **Team discovery** via shared team key + Tailscale tailnet, with mDNS on the local network and optional manual host fallback.
- **Signed messages** (HMAC-SHA256) with timestamp skew and nonce replay protection; optional **Tailnet only** mode (100.64.0.0/10).
- **Keyboard shortcuts** for opening the team list and holding a teammate's assigned key (platform-specific).
- **No telemetry**—settings and keys stay on your machines.

Default UDP port: **37561**.

## How it works

- **Sender:** open the tray popover, press and hold a peer row (pointer down/up — not a native menu item).
- **While holding:** UDP `start`, then `heartbeat` every ~500ms.
- **On release:** UDP `stop`.
- **Receiver:** one transparent, always-on-top overlay per display (click-through, no focus steal). If heartbeats stop for ~2s, the overlay clears itself.
- **Transport:** peer-to-peer UDP on LAN or Tailscale; no central server.
- **Discovery:** same team key on the same Tailscale tailnet. Tap says hello to each online machine and shows the ones that answer. On a local network it also uses mDNS. A typed host is only a fallback.
- **Auth:** HMAC-SHA256 with a shared team key, timestamp skew + nonce replay protection. Optional **Tailnet only** mode (100.64.0.0/10).

## Keyboard

Letter keys are not global. They only work while the team list is open.

1. Settings → Team members: focus the key box on a person and press one letter or number. Backspace clears it. Two people cannot share a key.
2. Open the list with a shortcut, then hold a teammate's key. Releasing stops the tap. Escape closes the list.

Tap has to be running (it is a menu-bar app).

### macOS

In Raycast, add the `raycast` folder in this repo as a Script Command directory. Assign **Open Tap team** a hotkey (Hyper plus whatever key you want). The script is silent: it runs `open tap://keys` and does not leave Raycast on screen. The first launch of a build registers the `tap://` link.

### Linux

Settings → App → **Open team list**. The first run uses **Ctrl+Alt+Shift+T**. Click it and press a new chord to change it. Backspace while it is listening clears it.

On GNOME that chord is saved as a custom keyboard shortcut, which works on Wayland. On X11, Tap grabs the chord itself. On other Wayland desktops, add a shortcut in the system keyboard settings that runs:

```bash
xdg-open tap://keys
```

`linux/open-team.sh` is that same command.

## Install

### macOS

1. Download the `.dmg` from a [GitHub Release](https://github.com/rhagerty32/tap/releases) (tag `v*`), or build locally (see below).
2. Open the DMG and drag **Tap** to Applications.
3. macOS will say the app is damaged. That dialog is Gatekeeper, and right-click → Open does not get past it. Drag Tap to Applications, then run:
   ```bash
   xattr -cr /Applications/Tap.app
   open /Applications/Tap.app
   ```
4. Grant **Local Network** access when prompted (required for mDNS/UDP).

### Linux (Ubuntu / GNOME)

**AppImage**

```bash
chmod +x Tap_*.AppImage
./Tap_*.AppImage
```

**Debian package**

```bash
sudo dpkg -i tap_*_amd64.deb
sudo apt-get install -f
```

Install tray dependencies if needed: `libayatana-appindicator3-1`, `libwebkit2gtk-4.1-0`.

## Team key & Tailscale setup

1. Open Tap. The first launch creates a **team key** and shows it on the Peers tab. Copy that same value into **Settings → Team key** on every other machine.
2. Set a recognizable **Display name** on each machine.
3. Install Tailscale on each machine and log into the same tailnet. Within a few seconds, everyone running Tap with this team key shows up. You do not need their IP.
4. **Tailscale only** is on by default, so Tap ignores people who are not on the tailnet. Turn it off to also find people on the local network, and before using **Preview on this Mac**.

Per-person **mute/allow** lists live in `~/.config/tap/settings.json` (`muteSenders` / `allowSenders` maps keyed by sender id) for v1.

## Try it on this Mac

1. Open Tap from the menu bar. The first launch opens the window for you.
2. Press **Preview on this Mac**.
3. The border and banner should show on your screens for a couple of seconds, then clear.

That uses the real signed UDP path, sent to this machine. It does not check discovery between two computers.

## Try it on two machines

1. Build or install Tap on **Machine A** and **Machine B** (same team key).
2. On both: open tray → **Settings** → set display names, team key, matching sender color optional → Save.
3. Wait a few seconds. Both machines need the same team key and the same tailnet.
4. On **Machine A**, open the popover and **press-and-hold** Machine B's row.
5. On **Machine B**, confirm the border + banner on all monitors; release on A and confirm it disappears.
6. Test safety: kill Tap on A while holding — B's overlay should clear within ~2s.

## Platform support & overlay behavior

| Platform | Tray / menu bar | Receiver overlay | Notes |
|----------|-----------------|------------------|-------|
| **macOS** (Apple Silicon) | Menu bar + popover | Transparent always-on-top webview windows per display; `setIgnoreCursorEvents`, high window level, collection behavior for all Spaces / fullscreen auxiliary | Best-effort non-activating panel behavior via NSWindow APIs. Notarization/signing when Apple secrets are configured in CI. |
| **Linux X11** | Ayatana app indicator | Transparent always-on-top undecorated windows; `set_ignore_cursor_events(true)` | Click-through works reliably on X11 sessions. |
| **Linux Wayland (wlroots: Sway, Hyprland, etc.)** | App indicator (via XWayland or native) | Same GTK/WebKit windows; compositor-dependent stacking | wlr-layer-shell is **not** wired in v1; behavior varies by compositor. |
| **Ubuntu / GNOME / Mutter (Wayland)** | Supported tray | Always-on-top transparent windows + ignored cursor events | Mutter has **no** wlr-layer-shell; this is the documented fallback (not a true layer-shell overlay). Stacking above full-screen apps may be inconsistent. |

## Build from source

Prerequisites: [Tauri prerequisites](https://tauri.app/start/prerequisites/), Node 22+, Rust stable (see `rust-toolchain.toml`).

```bash
npm ci
npm run tauri dev
```

Release bundles:

```bash
npm run tauri build
```

Artifacts land in `src-tauri/target/release/bundle/`.

See [CONTRIBUTING.md](CONTRIBUTING.md) for Linux packages CI uses, tests, and PR expectations.

## Cut a release

1. Bump `version` in `package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml` so it matches the tag (`v0.1.0` for version `0.1.0`).
2. Commit and push a tag. That is what publishes; the workflow does not run on normal pushes.
   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```
3. The **Release** workflow builds an Apple Silicon `.dmg`, a Linux `.AppImage`, and a `.deb`, then attaches them to the GitHub Release along with `latest.json`.
4. Settings → **Check for updates** downloads the next version for an app installed from the `.dmg` (dragged to Applications) or for the `.AppImage`. A `.deb` install is updated by installing the new package.
5. Before the first tag, set the repository secret `TAURI_SIGNING_PRIVATE_KEY` to the contents of `~/.tauri/tap.key`. That private key stays off the repo. Optional Apple signing: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) for build, test, and PR guidelines, and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for community expectations.

## Security notes

- No telemetry, accounts, or hosted server.
- Do not commit team keys; use Settings locally.
- Firewall must allow UDP **37561** (or your chosen port) between peers.

## Tests

```bash
npm test
```

That runs `cargo test`. The suite covers message signing, replay rejection, the 2s heartbeat timeout, and a loopback preview: signed UDP start, heartbeats, and stop drive the overlay visible, then hidden. No window and no second machine.

## CI

- **CI** workflow: `npm audit`, frontend build, `cargo test`, `cargo audit` (non-blocking), Tauri build on Ubuntu + macOS for PRs.
- **Release** workflow: tagged releases only.

## License

Tap is released under the [MIT License](LICENSE). Copyright Ryan Hagerty.
