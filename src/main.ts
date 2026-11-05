import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "./styles.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

type Peer = {
  id: string;
  name: string;
  online: boolean;
  seenSecsAgo?: number;
  addresses?: string[];
};

type Settings = {
  displayName: string;
  senderColor: string;
  teamKey: string;
  doNotDisturb: boolean;
  chimeEnabled: boolean;
  launchAtLogin: boolean;
  tailnetOnly: boolean;
  manualPeers: string[];
  listenPort: number;
};

type Preview = {
  settings: Settings;
  peers: Peer[];
  holdingId?: string;
};

const ARM_MS = 350;
const SAFETY_MS = 60_000;
const AVATAR_COLORS = ["#F7A53F", "#F86E51", "#E8638C", "#7C9A6E", "#6B8FB8", "#B47CC7"];

const preview = (window as Window & { __TAP_PREVIEW__?: Preview }).__TAP_PREVIEW__ ?? null;

let peers: Peer[] = [];
let currentSettings: Settings | null = null;
let statusMessage = "";
let armingId: string | null = null;
let holdingId: string | null = null;
let safetyTimer = 0;

const app = document.querySelector<HTMLDivElement>("#app")!;

if (/Linux/i.test(navigator.userAgent)) {
  document.documentElement.classList.add("no-blur");
}
if (preview) {
  document.body.classList.add("preview");
}

function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function reducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

function avatarColor(name: string): string {
  let hash = 0;
  for (const char of name) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return AVATAR_COLORS[hash % AVATAR_COLORS.length];
}

function away(peer: Peer): boolean {
  return !peer.online && (peer.seenSecsAgo ?? 99999) < 3600;
}

function holdable(peer: Peer): boolean {
  return peer.online || away(peer);
}

function statusText(peer: Peer): string {
  if (holdingId === peer.id) return "Tapping now";
  if (peer.online) return "Online";
  if (away(peer)) {
    const minutes = Math.max(1, Math.round((peer.seenSecsAgo ?? 60) / 60));
    return `Away ${minutes}m`;
  }
  return "Offline";
}

function hintText(peer: Peer): string {
  if (!holdable(peer)) return "";
  if (holdingId === peer.id) return "Release to stop";
  return "Hold to tap";
}

function nearbyLabel(): string {
  const count = peers.filter((peer) => peer.online).length;
  if (count === 0) return "Nobody nearby";
  return count === 1 ? "1 nearby" : `${count} nearby`;
}

function cardHeight(): number {
  const list = peers.length === 0 ? 72 : peers.length * 56 + Math.max(0, peers.length - 1) * 2;
  const error = statusMessage ? 24 : 0;
  return Math.min(480, 16 + 12 + 38 + error + list + 36);
}

async function fitWindow() {
  if (preview) return;
  try {
    await getCurrentWindow().setSize(new LogicalSize(368, cardHeight() + 48));
  } catch {
    // Outside the Tauri window this is a plain page.
  }
}

function render() {
  const you = currentSettings?.displayName?.trim() || "you";
  const rows =
    peers.length === 0
      ? `<div class="empty"><strong>Nobody nearby yet</strong><p>Share your team key so teammates can find you.</p></div>`
      : peers
          .map((peer) => {
            const classes = [
              "row",
              holdable(peer) ? "" : "offline",
              armingId === peer.id ? "pressing" : "",
              holdingId === peer.id ? "pressing active" : "",
            ]
              .filter(Boolean)
              .join(" ");
            return `<button class="${classes}" type="button" data-id="${esc(peer.id)}">
              <span class="avatar-wrap">
                <svg class="ring" viewBox="0 0 44 44" aria-hidden="true"><circle cx="22" cy="22" r="20"></circle></svg>
                <span class="avatar" style="background:${avatarColor(peer.name)}">${esc(initials(peer.name))}</span>
              </span>
              <span class="who"><span class="name">${esc(peer.name)}</span><span class="status">${esc(statusText(peer))}</span></span>
              <span class="hint">${esc(hintText(peer))}</span>
            </button>`;
          })
          .join("");

  app.innerHTML = `
    <div class="popover">
      <header class="top">
        <div class="mark" aria-hidden="true">t</div>
        <h1>Tap</h1>
        <div class="nearby">${esc(nearbyLabel())}</div>
      </header>
      ${statusMessage ? `<p class="error">${esc(statusMessage)}</p>` : ""}
      <div class="peers">${rows}</div>
      <footer class="foot"><span>You are <b>${esc(you)}</b></span><button type="button" id="open-settings">Settings</button></footer>
    </div>
  `;

  document.getElementById("open-settings")!.onclick = () => {
    void openSettings();
  };
  for (const row of app.querySelectorAll<HTMLButtonElement>(".row")) {
    const peer = peers.find((item) => item.id === row.dataset.id);
    if (!peer || !holdable(peer)) continue;
    row.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      void beginArm(peer);
    });
  }

  if (armingId && !holdingId) {
    const id = armingId;
    requestAnimationFrame(() => {
      if (armingId !== id) return;
      app.querySelector<HTMLElement>(`[data-id="${CSS.escape(id)}"]`)?.classList.add("arming");
    });
  }

  void fitWindow();
}

async function beginArm(peer: Peer) {
  if (holdingId || armingId) return;
  armingId = peer.id;
  statusMessage = "";
  const stop = () => {
    window.removeEventListener("pointerup", stop);
    window.removeEventListener("pointercancel", stop);
    void endHold(peer);
  };
  window.addEventListener("pointerup", stop);
  window.addEventListener("pointercancel", stop);
  render();
  window.setTimeout(() => {
    if (armingId !== peer.id) return;
    armingId = null;
    holdingId = peer.id;
    render();
    void startTap(peer);
  }, ARM_MS);
}

async function startTap(peer: Peer) {
  if (holdingId !== peer.id) return;
  if (preview) {
    safetyTimer = window.setTimeout(() => void endHold(peer), SAFETY_MS);
    return;
  }
  try {
    await invoke("hold_start", { peerId: peer.id });
    safetyTimer = window.setTimeout(() => void endHold(peer), SAFETY_MS);
  } catch (error) {
    holdingId = null;
    statusMessage = friendly(formatErr(error), peer.name);
    render();
  }
}

async function endHold(peer: Peer) {
  const wasArming = armingId === peer.id;
  const wasHolding = holdingId === peer.id;
  if (!wasArming && !wasHolding) return;
  armingId = null;
  holdingId = null;
  window.clearTimeout(safetyTimer);
  const row = app.querySelector<HTMLElement>(`[data-id="${CSS.escape(peer.id)}"]`);
  row?.classList.remove("arming", "active", "pressing");
  if (wasArming && !wasHolding) {
    row?.classList.add("retract");
  }
  if (wasHolding && !preview) {
    try {
      await invoke("hold_stop");
    } catch (error) {
      statusMessage = formatErr(error);
    }
  }
  window.setTimeout(() => {
    if (!armingId && !holdingId) render();
  }, reducedMotion() ? 0 : 180);
}

async function openSettings() {
  try {
    const settings = await WebviewWindow.getByLabel("settings");
    await settings?.show();
    await settings?.setFocus();
  } catch (error) {
    statusMessage = formatErr(error);
    render();
  }
}

function friendly(message: string, name: string): string {
  if (/reachable|address/i.test(message)) {
    return `Couldn't reach ${name}. Check it's on the same network.`;
  }
  return message;
}

function formatErr(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return String(error);
}

void listen<Peer[]>("peers-updated", (event) => {
  peers = event.payload;
  render();
}).catch(() => {});

async function boot() {
  if (preview) {
    currentSettings = preview.settings;
    peers = preview.peers;
    if (preview.holdingId) holdingId = preview.holdingId;
    render();
    return;
  }
  try {
    currentSettings = await invoke<Settings>("get_settings");
    peers = await invoke<Peer[]>("list_peers");
  } catch (error) {
    statusMessage = formatErr(error);
  }
  render();
}

void boot();
