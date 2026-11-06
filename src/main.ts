import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "./styles.css";
import { bootTheme } from "./theme";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Peer = {
  id: string;
  name: string;
  online: boolean;
  seenSecsAgo?: number;
  addresses?: string[];
  status?: string;
  avatarHash?: string;
  color?: string;
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
  peerColors?: Record<string, string>;
};

type Preview = {
  settings: Settings;
  peers: Peer[];
  holdingId?: string;
};

const ARM_MS = 350;
const SAFETY_MS = 60_000;
const LIGHTNESS = 54;
const AVATAR_COLORS = ["#F7A53F", "#F86E51", "#E8638C", "#7C9A6E", "#6B8FB8", "#B47CC7"];

const preview = (window as Window & { __TAP_PREVIEW__?: Preview }).__TAP_PREVIEW__ ?? null;

if (!preview) bootTheme();

let peers: Peer[] = [];
let dndOn = false;
const avatarUrls = new Map<string, string>();
const dndPeers = new Set<string>();
let statusMessage = "";
let armingId: string | null = null;
let holdingId: string | null = null;
let safetyTimer = 0;
let colorFor: string | null = null;
let colorBefore: string | undefined;
let dial = { h: 12, s: 78 };
let dialDirty = false;
let sliding = false;

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

function avatarColor(peer: Peer): string {
  if (peer.color) return peer.color;
  let hash = 0;
  for (const char of peer.id) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return AVATAR_COLORS[hash % AVATAR_COLORS.length];
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function hexToHsl(hex: string): { h: number; s: number } {
  const raw = hex.replace("#", "");
  if (raw.length !== 6) return { h: 12, s: 78 };
  const r = parseInt(raw.slice(0, 2), 16) / 255;
  const g = parseInt(raw.slice(2, 4), 16) / 255;
  const b = parseInt(raw.slice(4, 6), 16) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  let h = 0;
  if (d !== 0) {
    if (max === r) h = ((g - b) / d) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h *= 60;
    if (h < 0) h += 360;
  }
  const l = (max + min) / 2;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  return { h, s: s * 100 };
}

function hslToHex(h: number, s: number, l = LIGHTNESS): string {
  const sn = clamp(s, 0, 100) / 100;
  const ln = l / 100;
  const c = (1 - Math.abs(2 * ln - 1)) * sn;
  const hp = (((h % 360) + 360) % 360) / 60;
  const x = c * (1 - Math.abs((hp % 2) - 1));
  let r = 0;
  let g = 0;
  let b = 0;
  if (hp < 1) [r, g, b] = [c, x, 0];
  else if (hp < 2) [r, g, b] = [x, c, 0];
  else if (hp < 3) [r, g, b] = [0, c, x];
  else if (hp < 4) [r, g, b] = [0, x, c];
  else if (hp < 5) [r, g, b] = [x, 0, c];
  else [r, g, b] = [c, 0, x];
  const m = ln - c / 2;
  const byte = (channel: number) =>
    Math.round(clamp(channel + m, 0, 1) * 255)
      .toString(16)
      .padStart(2, "0");
  return `#${byte(r)}${byte(g)}${byte(b)}`.toUpperCase();
}

function hueGradient(): string {
  const stops = [0, 30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 330, 360]
    .map((h) => `${hslToHex(h, 100)} ${(h / 360) * 100}%`)
    .join(", ");
  return `linear-gradient(90deg, ${stops})`;
}

function dialColor(): string {
  return hslToHex(dial.h, dial.s);
}

function away(peer: Peer): boolean {
  return !peer.online && (peer.seenSecsAgo ?? 99999) < 3600;
}

function holdable(peer: Peer): boolean {
  return peer.online || away(peer);
}

function statusText(peer: Peer): string {
  if (holdingId === peer.id) return "Tapping now";
  if (dndPeers.has(peer.id)) return "Do not disturb";
  if (peer.online && peer.status && peer.status !== "Online") return peer.status;
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

async function fitWindow() {
  if (preview) return;
  const card = app.querySelector<HTMLElement>(".popover");
  if (!card) return;
  const height = Math.max(120, Math.ceil(card.getBoundingClientRect().height));
  try {
    await invoke("reposition_popover", { width: 368, height });
  } catch {
    // Outside the Tauri window this is a plain page.
  }
}

function render() {
  if (colorFor && !peers.some((peer) => peer.id === colorFor)) colorFor = null;
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
            const photo = avatarUrls.get(peer.id);
            const face = photo
              ? `<img src="${esc(photo)}" alt="" />`
              : esc(initials(peer.name));
            const colorPanel =
              colorFor === peer.id
                ? `<div class="row-color">
                    <div class="row-color-title">Change color</div>
                    <div class="dial-block">
                      <div class="dial-label">Hue</div>
                      <div class="dial" data-dial="hue" role="slider" tabindex="0" aria-label="Hue" aria-valuemin="0" aria-valuemax="360" aria-valuenow="${Math.round(dial.h)}" style="--track:${hueGradient()}">
                        <span class="knob" style="--t:${dial.h / 360}; --knob:${dialColor()}"></span>
                      </div>
                    </div>
                    <div class="dial-block">
                      <div class="dial-label">Saturation</div>
                      <div class="dial" data-dial="sat" role="slider" tabindex="0" aria-label="Saturation" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${Math.round(dial.s)}" style="--track:linear-gradient(90deg, ${hslToHex(dial.h, 0)}, ${hslToHex(dial.h, 100)})">
                        <span class="knob" style="--t:${dial.s / 100}; --knob:${dialColor()}"></span>
                      </div>
                    </div>
                  </div>`
                : "";
            return `<div class="${classes}" data-id="${esc(peer.id)}">
              <span class="avatar-wrap">
                <svg class="ring" viewBox="0 0 44 44" aria-hidden="true"><circle cx="22" cy="22" r="20"></circle></svg>
                <span class="avatar" style="background:${avatarColor(peer)}">${face}</span>
              </span>
              <span class="who"><span class="name">${esc(peer.name)}</span><span class="status">${esc(statusText(peer))}</span></span>
              <span class="hint">${esc(hintText(peer))}</span>
            </div>${colorPanel}`;
          })
          .join("");

  app.innerHTML = `
    <div class="popover">
      ${statusMessage ? `<p class="error">${esc(statusMessage)}</p>` : ""}
      <div class="peers">${rows}</div>
      <footer class="foot">
        <button type="button" id="open-settings" class="icon-btn" aria-label="Settings" title="Settings">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z"></path>
            <path d="M19.4 13.5a7.7 7.7 0 0 0 .1-1.5 7.7 7.7 0 0 0-.1-1.5l2-1.6-2-3.4-2.4 1a7.4 7.4 0 0 0-2.6-1.5L14 2h-4l-.4 2.5a7.4 7.4 0 0 0-2.6 1.5l-2.4-1-2 3.4 2 1.6a7.7 7.7 0 0 0-.1 1.5 7.7 7.7 0 0 0 .1 1.5l-2 1.6 2 3.4 2.4-1a7.4 7.4 0 0 0 2.6 1.5L10 22h4l.4-2.5a7.4 7.4 0 0 0 2.6-1.5l2.4 1 2-3.4-2-1.6Z"></path>
          </svg>
        </button>
        <div class="dnd">
          <span>Do not disturb</span>
          <button id="dnd" class="switch sm" type="button" role="switch" aria-checked="${dndOn ? "true" : "false"}" aria-label="Do not disturb">
            <span class="knob"></span>
          </button>
        </div>
      </footer>
    </div>
  `;

  document.getElementById("open-settings")!.onclick = () => {
    void openSettings();
  };
  document.getElementById("dnd")!.onclick = () => {
    dndOn = !dndOn;
    render();
    void invoke("set_do_not_disturb", { enabled: dndOn }).catch((error) => {
      dndOn = !dndOn;
      statusMessage = formatErr(error);
      render();
    });
  };
  app.querySelector(".popover")?.addEventListener("contextmenu", (event) => {
    event.preventDefault();
  });
  app.querySelector(".popover")?.addEventListener("pointerdown", (event) => {
    if (!colorFor || sliding) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    if (target.closest(".row") || target.closest(".row-color")) return;
    closeColor();
  });
  for (const row of app.querySelectorAll<HTMLElement>(".row")) {
    const peer = peers.find((item) => item.id === row.dataset.id);
    if (!peer) continue;
    row.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (holdingId || armingId) return;
      if (colorFor === peer.id) {
        closeColor();
        return;
      }
      dial = hexToHsl(avatarColor(peer));
      dialDirty = false;
      colorBefore = peer.color;
      colorFor = peer.id;
      render();
    });
    if (!holdable(peer)) continue;
    row.addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || event.ctrlKey) return;
      if (colorFor) {
        closeColor();
        return;
      }
      event.preventDefault();
      void beginArm(peer);
    });
  }
  mountColorDials();

  if (armingId && !holdingId) {
    const id = armingId;
    requestAnimationFrame(() => {
      if (armingId !== id) return;
      app.querySelector<HTMLElement>(`[data-id="${CSS.escape(id)}"]`)?.classList.add("arming");
    });
  }

  void fitWindow();
}

function closeColor() {
  if (dialDirty) void commitColor();
  colorFor = null;
  render();
}

function paintColor() {
  const panel = app.querySelector<HTMLElement>(".row-color");
  if (!panel || !colorFor) return;
  const color = dialColor();
  const hue = panel.querySelector<HTMLElement>('[data-dial="hue"]');
  const sat = panel.querySelector<HTMLElement>('[data-dial="sat"]');
  const hueKnob = hue?.querySelector<HTMLElement>(".knob");
  const satKnob = sat?.querySelector<HTMLElement>(".knob");
  if (hueKnob) {
    hueKnob.style.setProperty("--t", String(dial.h / 360));
    hueKnob.style.setProperty("--knob", color);
  }
  if (satKnob) {
    satKnob.style.setProperty("--t", String(dial.s / 100));
    satKnob.style.setProperty("--knob", color);
  }
  if (hue) hue.setAttribute("aria-valuenow", String(Math.round(dial.h)));
  if (sat) {
    sat.style.setProperty("--track", `linear-gradient(90deg, ${hslToHex(dial.h, 0)}, ${hslToHex(dial.h, 100)})`);
    sat.setAttribute("aria-valuenow", String(Math.round(dial.s)));
  }
  const peer = peers.find((item) => item.id === colorFor);
  if (peer) peer.color = color;
  const avatar = app.querySelector<HTMLElement>(`[data-id="${CSS.escape(colorFor)}"] .avatar`);
  if (avatar) avatar.style.background = color;
}

function setDialFromPointer(dialEl: HTMLElement, kind: "hue" | "sat", clientX: number) {
  const rect = dialEl.getBoundingClientRect();
  const pad = 11;
  const span = Math.max(1, rect.width - pad * 2);
  const t = clamp((clientX - rect.left - pad) / span, 0, 1);
  if (kind === "hue") dial.h = t * 360;
  else dial.s = t * 100;
  dialDirty = true;
  paintColor();
}

function mountColorDials() {
  const panel = app.querySelector<HTMLElement>(".row-color");
  if (!panel) return;
  panel.addEventListener("contextmenu", (event) => event.preventDefault());
  for (const dialEl of panel.querySelectorAll<HTMLElement>("[data-dial]")) {
    const kind = dialEl.dataset.dial === "sat" ? "sat" : "hue";
    dialEl.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      event.stopPropagation();
      sliding = true;
      setDialFromPointer(dialEl, kind, event.clientX);
      try {
        dialEl.setPointerCapture(event.pointerId);
      } catch {
        // The down position still applies when capture is unavailable.
      }
    });
    dialEl.addEventListener("pointermove", (event) => {
      if (!dialEl.hasPointerCapture(event.pointerId)) return;
      setDialFromPointer(dialEl, kind, event.clientX);
    });
    const finish = () => {
      if (!sliding) return;
      sliding = false;
      void commitColor();
    };
    dialEl.addEventListener("pointerup", finish);
    dialEl.addEventListener("pointercancel", finish);
  }
}

async function commitColor() {
  if (!colorFor || !dialDirty) return;
  const id = colorFor;
  const color = dialColor();
  const before = colorBefore;
  dialDirty = false;
  const peer = peers.find((item) => item.id === id);
  if (peer) peer.color = color;
  if (preview) {
    colorBefore = color;
    return;
  }
  try {
    await invoke("set_peer_color", { peerId: id, color });
    colorBefore = color;
  } catch (error) {
    if (peer) peer.color = before;
    statusMessage = formatErr(error);
    render();
  }
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
  dndPeers.delete(peer.id);
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
    await invoke("open_settings");
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
  void refreshAvatars();
  render();
}).catch(() => {});

void listen<boolean>("dnd-changed", (event) => {
  dndOn = event.payload;
  render();
}).catch(() => {});

void listen<string>("peer-busy", (event) => {
  dndPeers.add(event.payload);
  if (holdingId === event.payload || armingId === event.payload) {
    holdingId = null;
    armingId = null;
    void invoke("hold_stop").catch(() => {});
  }
  render();
}).catch(() => {});

void listen<{ id: string; hash: string }>("avatar-updated", (event) => {
  void loadAvatar(event.payload.id, event.payload.hash);
}).catch(() => {});

async function loadAvatar(id: string, hash: string) {
  if (!hash || avatarUrls.get(`${id}:${hash}`)) return;
  try {
    const url = await invoke<string | null>("peer_avatar", { peerId: id, hash });
    if (!url) return;
    avatarUrls.set(id, url);
    avatarUrls.set(`${id}:${hash}`, url);
    render();
  } catch {
    // The file may still be downloading.
  }
}

async function refreshAvatars() {
  await Promise.all(peers.filter((peer) => peer.avatarHash).map((peer) => loadAvatar(peer.id, peer.avatarHash!)));
}

async function boot() {
  if (preview) {
    peers = preview.peers;
    if (preview.holdingId) holdingId = preview.holdingId;
    render();
    return;
  }
  try {
    peers = await invoke<Peer[]>("list_peers");
    dndOn = await invoke<boolean>("dnd_state");
    void refreshAvatars();
  } catch (error) {
    statusMessage = formatErr(error);
  }
  render();
}

void boot();
