# Tap

Tap is a small cross-platform office app for getting a coworker's attention when they are wearing headphones. Ryan press-and-holds a peer in the menu bar / system tray popover; while he holds, the peer's screens show a click-through glowing border and banner. Releasing stops the signal immediately.

Built with **Tauri v2** (Rust core + lightweight TypeScript UI).

## How it works

- **Sender:** open the tray popover, press and hold a peer row (pointer down/up — not a native menu item).
- **While holding:** UDP `start`, then `heartbeat` every ~500ms.
- **On release:** UDP `stop`.
- **Receiver:** one transparent, always-on-top overlay per display (click-through, no focus steal). If heartbeats stop for ~2s, the overlay clears itself.
- **Transport:** peer-to-peer UDP on LAN or Tailscale; no central server.
- **Discovery:** mDNS (`_tap._udp.local.`), manual peers, and `tailscale status --json` when Tailscale is installed.
- **Auth:** HMAC-SHA256 with a shared team key, timestamp skew + nonce replay protection. Optional **Tailnet only** mode (100.64.0.0/10).

Default UDP port: **37561**.

## Install

### macOS

1. Download the `.dmg` from a GitHub Release (tag `v*`), or build locally (see below).
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
3. **Optional Tailscale:** install Tailscale on both machines, log into the same tailnet. Tap reads peers from `tailscale status --json`. You can add manual peers like `Alex@100.64.0.5` in Settings.
4. Enable **Accept only Tailscale IPs** if you want to ignore non-tailnet senders on mixed networks. Turn that off before using **Preview on this Mac**.

Per-person **mute/allow** lists live in `~/.config/tap/settings.json` (`muteSenders` / `allowSenders` maps keyed by sender id) for v1.

## Try it on this Mac

1. Open Tap from the menu bar. The first launch opens the window for you.
2. Press **Preview on this Mac**.
3. The border and banner should show on your screens for a couple of seconds, then clear.

That uses the real signed UDP path, sent to this machine. It does not check discovery between two computers.

## Try it on two machines

1. Build or install Tap on **Machine A** and **Machine B** (same team key).
2. On both: open tray → **Settings** → set display names, team key, matching sender color optional → Save.
3. Wait a few seconds for mDNS/Tailscale discovery, or add manual peer `Name@ip` on the sender.
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
