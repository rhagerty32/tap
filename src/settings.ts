import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "./styles.css";
import "./settings.css";
import { applyTheme, normalizeTheme } from "./theme";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";

type Settings = {
  displayName: string;
  senderColor: string;
  teamKey: string;
  doNotDisturb: boolean;
  chimeEnabled: boolean;
  launchAtLogin: boolean;
  tailnetOnly: boolean;
  manualPeers: string[];
  hiddenPeers?: string[];
  listenPort: number;
  followSystemDnd: boolean;
  suppressSystemDnd: boolean;
  glowStyle: string;
  showBanner: boolean;
  overlayScreens: string;
  selectedDisplays: string[];
  peerColors: Record<string, string>;
  peerKeys?: Record<string, string>;
  rosterShortcut?: string | null;
  theme?: string;
  trackpad?: TrackpadSettings;
};

type EdgeSensor = {
  enabled: boolean;
  action: string;
  start: number;
  end: number;
  thickness: number;
};

type CornerActions = {
  single: string;
  double: string;
  holdPeer: string;
};

type TrackpadSettings = {
  enabled: boolean;
  edges: { left: EdgeSensor; right: EdgeSensor; top: EdgeSensor; bottom: EdgeSensor };
  corners: {
    topLeft: CornerActions;
    topRight: CornerActions;
    bottomLeft: CornerActions;
    bottomRight: CornerActions;
  };
  sensitivity: number;
  requireModifier: string;
  flipVerticalEdges: boolean;
  freezeCursor: boolean;
  preciseSliding: boolean;
  fineControlWithModifiers: boolean;
  hapticsEnabled: boolean;
  hapticStrength: string;
  mediaStripReverse: boolean;
  showHUD: boolean;
};

type Preview = { settings: Settings; peers?: TeamPeer[] };

const preview = (window as Window & { __TAP_PREVIEW__?: Preview }).__TAP_PREVIEW__ ?? null;

const GLOW_STYLES = ["line", "edge", "bold", "soft", "wash", "corners"];

type TeamPeer = {
  id: string;
  name: string;
  color?: string;
  avatarHash?: string;
  key?: string;
  source?: string;
  addresses?: string[];
};

type PersonRow = {
  id: string;
  name: string;
  color: string;
  avatarHash?: string;
  manualIndex?: number;
  detail?: string;
  hotkey: string;
};

type ManualEntry = { index: number; name: string; host: string; id: string };

const AUTO_COLORS = ["#F7A53F", "#F86E51", "#E8638C", "#7C9A6E", "#6B8FB8", "#B47CC7"];
const LIGHTNESS = 54;

let settings: Settings | null = null;
let peers: TeamPeer[] = [];
let peerPhotos = new Map<string, string>();
let colorFor: string | null = null;
let dial = { h: 12, s: 78 };
let dialDirty = false;
let sliding = false;
let colorSaveGen = 0;
let colorBefore: { hadSaved: boolean; color?: string } | null = null;
let displays: string[] = [];
let photo = "";
let adding = false;
let addDraft = "";
let refreshedAt: number | null = null;
let refreshBusy = false;
let error = "";
let appVersion = "";
let updateBusy = false;
let updateLabel = "Check for updates";
let updateNote = "";

type Tab = "profile" | "glow" | "team" | "trackpad" | "app";

const linux = /Linux/i.test(navigator.userAgent);
// Trackpad gestures only exist on macOS, so the tab is hidden everywhere else.
const isMac = /Mac/i.test(navigator.platform || navigator.userAgent);

const tabs: { id: Tab; label: string }[] = [
  { id: "profile", label: "Profile" },
  { id: "glow", label: "Glow" },
  { id: "team", label: "Team" },
  ...(isMac ? [{ id: "trackpad" as Tab, label: "Trackpad" }] : []),
  { id: "app", label: "App" },
];

let tab: Tab = "profile";
let armingShortcut = false;
let shortcutNote = "";

const root = document.querySelector<HTMLDivElement>("#settings")!;

if (preview) document.body.classList.add("preview");

function hideSettings() {
  if (preview) return;
  void invoke("hide_settings").catch(() => {});
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function autoColor(id: string): string {
  let hash = 0;
  for (let i = 0; i < id.length; i++) {
    hash = (Math.imul(hash, 31) + id.charCodeAt(i)) >>> 0;
  }
  return AUTO_COLORS[hash % AUTO_COLORS.length];
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
  const hp = ((((h % 360) + 360) % 360) / 60);
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

function bareHost(value: string): string {
  if (value.startsWith("[")) {
    const end = value.indexOf("]");
    return end > 0 ? value.slice(1, end) : value;
  }
  const parts = value.split(":");
  if (parts.length === 2 && /^\d+$/.test(parts[1])) return parts[0];
  return value;
}

function portOf(value: string): string | null {
  if (value.startsWith("[")) {
    const end = value.indexOf("]");
    if (end >= 0 && value[end + 1] === ":") return value.slice(end + 2);
    return null;
  }
  const parts = value.split(":");
  if (parts.length === 2 && /^\d+$/.test(parts[1])) return parts[1];
  return null;
}

function sameHost(manualHost: string, address: string): boolean {
  const manual = manualHost.trim().toLowerCase();
  const addr = address.trim().toLowerCase();
  if (!manual || !addr) return false;
  if (manual === addr) return true;
  if (bareHost(manual) !== bareHost(addr)) return false;
  const manualPort = portOf(manual);
  const addrPort = portOf(addr);
  if (manualPort && addrPort) return manualPort === addrPort;
  return true;
}

function refreshedLabel(at: number | null): string {
  if (at == null) return "Not refreshed yet";
  const time = new Date(at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return `Last refreshed ${time}`;
}

function personKey(name: string): string {
  return name.trim().replace(/\.+$/, "").toLowerCase();
}

function samePerson(a: TeamPeer, b: TeamPeer): boolean {
  const ips = new Set((a.addresses ?? []).map((addr) => bareHost(addr).toLowerCase()));
  if ((b.addresses ?? []).some((addr) => ips.has(bareHost(addr).toLowerCase()))) return true;
  const left = personKey(a.name);
  const right = personKey(b.name);
  return left.length > 0 && left === right;
}

function clusterIds(id: string): string[] {
  const ids = new Set<string>([id]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const peer of peers) {
      if (ids.has(peer.id)) continue;
      if (peers.some((other) => ids.has(other.id) && samePerson(peer, other))) {
        ids.add(peer.id);
        grew = true;
      }
    }
  }
  return [...ids];
}

async function refreshPeople() {
  if (!settings) return;
  if (preview) {
    refreshedAt = Date.now();
    render();
    return;
  }
  refreshBusy = true;
  render();
  try {
    await invoke("refresh_peers");
    error = "";
  } catch (err) {
    refreshBusy = false;
    error = typeof err === "string" ? err : "Couldn't refresh.";
    render();
  }
}

async function removePerson(id: string) {
  if (!settings) return;
  const cluster = new Set(clusterIds(id));
  settings.hiddenPeers = [...new Set([...(settings.hiddenPeers ?? []), ...cluster])];
  settings.manualPeers = settings.manualPeers.filter((entry, index) => {
    const parsed = parseManual(entry, index);
    if (!parsed) return false;
    if (cluster.has(parsed.id)) return false;
    const removed = peers.filter((peer) => cluster.has(peer.id));
    if (removed.some((peer) => personKey(peer.name) === personKey(parsed.name))) return false;
    if (removed.some((peer) => (peer.addresses ?? []).some((addr) => sameHost(parsed.host, addr)))) return false;
    return true;
  });
  peers = peers.filter((peer) => !cluster.has(peer.id));
  if (colorFor && cluster.has(colorFor)) colorFor = null;
  render();
  if (preview) return;
  try {
    settings = await invoke<Settings>("remove_person", { peerId: id });
    peers = await invoke<TeamPeer[]>("list_peers");
    error = "";
  } catch (err) {
    error = typeof err === "string" ? err : "Couldn't remove that person.";
  }
  render();
}

function parseManual(entry: string, index: number): ManualEntry | null {
  const trimmed = entry.trim();
  if (!trimmed) return null;
  const at = trimmed.indexOf("@");
  const name = (at >= 0 ? trimmed.slice(0, at) : trimmed).trim();
  const host = (at >= 0 ? trimmed.slice(at + 1) : trimmed).trim();
  if (!host) return null;
  return { index, name: name || host, host, id: `manual-${name || host}-${host}` };
}

function shortcutLabel(raw: string | null | undefined): string {
  if (!raw) return "";
  return raw
    .split("+")
    .map((part) => {
      if (part === "ctrl") return "Ctrl";
      if (part === "alt") return "Alt";
      if (part === "shift") return "Shift";
      if (part === "super") return "Super";
      if (part.startsWith("Key") && part.length === 4) return part.slice(3);
      if (part.startsWith("Digit")) return part.slice(5);
      if (part.startsWith("Arrow")) return part.slice(5);
      if (part.startsWith("Numpad")) return `Num ${part.slice(6)}`;
      return part;
    })
    .join("+");
}

function chordFromEvent(event: KeyboardEvent): string | null {
  if (!(event.metaKey || event.ctrlKey || event.altKey || event.shiftKey)) return null;
  if (/^(Control|Alt|Shift|Meta|OS)/.test(event.code)) return null;
  const mods: string[] = [];
  if (event.ctrlKey) mods.push("ctrl");
  if (event.altKey) mods.push("alt");
  if (event.shiftKey) mods.push("shift");
  if (event.metaKey) mods.push("super");
  return [...mods, event.code].join("+");
}

async function saveRosterShortcut(raw: string) {
  armingShortcut = false;
  if (!settings) return;
  if (preview) {
    settings.rosterShortcut = raw;
    shortcutNote = "";
    render();
    return;
  }
  try {
    settings = await invoke<Settings>("set_roster_shortcut", { shortcut: raw });
    shortcutNote = await invoke<string>("roster_shortcut_error");
  } catch (err) {
    shortcutNote = typeof err === "string" ? err : "Couldn't set that shortcut.";
  }
  render();
}

function savedKey(id: string): string {
  const keys = settings?.peerKeys;
  if (!keys) return "";
  if (keys[id]) return keys[id];
  for (const other of clusterIds(id)) {
    if (keys[other]) return keys[other];
  }
  return "";
}

async function assignKey(id: string, raw: string) {
  if (!settings || !id) return;
  error = "";
  if (preview) {
    const next = { ...(settings.peerKeys || {}) };
    const key = raw.trim().toLowerCase();
    for (const other of clusterIds(id)) delete next[other];
    if (key) next[id] = key.slice(0, 1);
    settings.peerKeys = next;
    render();
    return;
  }
  try {
    settings = await invoke<Settings>("set_peer_key", { peerId: id, key: raw });
    peers = await invoke<TeamPeer[]>("list_peers");
    error = "";
  } catch (err) {
    error = typeof err === "string" ? err : "Couldn't save that key.";
  }
  render();
}

function manualClaims(manuals: ManualEntry[], peer: TeamPeer): boolean {
  return manuals.some(
    (entry) => entry.id === peer.id || (peer.addresses ?? []).some((addr) => sameHost(entry.host, addr)),
  );
}

function peopleRows(): PersonRow[] {
  const seenManual = new Set<string>();
  const manuals = (settings?.manualPeers ?? [])
    .map((entry, index) => parseManual(entry, index))
    .filter((entry): entry is ManualEntry => {
      if (!entry) return false;
      const key = `${entry.name}\0${entry.host}`.toLowerCase();
      if (seenManual.has(key)) return false;
      seenManual.add(key);
      return true;
    });
  const hidden = new Set(settings?.hiddenPeers ?? []);
  const used = new Set<number>();
  const byId = new Map(manuals.map((entry) => [entry.id, entry]));
  const rows: PersonRow[] = peers
    .filter((peer) => !hidden.has(peer.id) || manualClaims(manuals, peer))
    .map((peer) => {
    let manual = byId.get(peer.id);
    if (!manual) {
      manual = manuals.find(
        (entry) => !used.has(entry.index) && (peer.addresses ?? []).some((addr) => sameHost(entry.host, addr)),
      );
    }
    if (manual) used.add(manual.index);
    return {
      id: peer.id,
      name: peer.name,
      color: peer.color || settings?.peerColors?.[peer.id] || autoColor(peer.id),
      avatarHash: peer.avatarHash,
      manualIndex: manual?.index,
      detail: manual && manual.host !== peer.name ? manual.host : undefined,
      hotkey: peer.key || "",
    };
  });
  for (const manual of manuals) {
    if (used.has(manual.index)) continue;
    rows.push({
      id: manual.id,
      name: manual.name,
      color: settings?.peerColors?.[manual.id] || autoColor(manual.id),
      manualIndex: manual.index,
      detail: manual.host !== manual.name ? manual.host : undefined,
      hotkey: savedKey(manual.id),
    });
  }
  rows.sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
  return rows;
}

function cornerBlank(): CornerActions {
  return { single: "none", double: "none", holdPeer: "" };
}

function sensor(action: string, enabled = true): EdgeSensor {
  return { enabled, action, start: 0, end: 1, thickness: 0.18 };
}

function defaultTrackpad(): TrackpadSettings {
  const topRight = cornerBlank();
  topRight.double = "mute";
  return {
    enabled: true,
    edges: {
      left: sensor("brightness"),
      right: sensor("volume"),
      top: sensor("media", false),
      bottom: sensor("media"),
    },
    corners: {
      topLeft: cornerBlank(),
      topRight,
      bottomLeft: cornerBlank(),
      bottomRight: cornerBlank(),
    },
    sensitivity: 1,
    requireModifier: "none",
    flipVerticalEdges: false,
    freezeCursor: true,
    preciseSliding: false,
    fineControlWithModifiers: true,
    hapticsEnabled: true,
    hapticStrength: "medium",
    mediaStripReverse: false,
    showHUD: true,
  };
}

function trackpadOf(current: Settings): TrackpadSettings {
  if (!current.trackpad) current.trackpad = defaultTrackpad();
  return current.trackpad;
}

const SLIDE_ACTIONS = [
  ["off", "Off"],
  ["volume", "Volume"],
  ["brightness", "Brightness"],
  ["media", "Media"],
] as const;

const CLICK_ACTIONS = [
  ["none", "None"],
  ["mute", "Mute"],
  ["playPause", "Play / pause"],
  ["next", "Next track"],
  ["previous", "Previous track"],
] as const;

function slideValue(edge: EdgeSensor): string {
  return edge.enabled ? edge.action : "off";
}

function options(pairs: readonly (readonly [string, string])[], selected: string): string {
  return pairs
    .map(([value, label]) => `<option value="${esc(value)}"${value === selected ? " selected" : ""}>${esc(label)}</option>`)
    .join("");
}

// ---------- UI building blocks ----------

const ICONS = {
  chevron: `<svg class="chev" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M2 3.5 5 6.5l3-3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  copy: `<svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true"><rect x="4.5" y="4.5" width="7" height="7" rx="1.6" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M9.5 4.5V3.6A1.6 1.6 0 0 0 7.9 2H3.6A1.6 1.6 0 0 0 2 3.6v4.3a1.6 1.6 0 0 0 1.6 1.6h.9" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>`,
  plus: `<svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M6 2v8M2 6h8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>`,
  close: `<svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="m3 3 6 6M9 3 3 9" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>`,
  refresh: `<svg width="13" height="13" viewBox="0 0 14 14" aria-hidden="true"><path d="M11.5 7A4.5 4.5 0 1 1 9.9 3.6M11.5 2v2.4H9.1" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  volume: `<svg width="13" height="13" viewBox="0 0 14 14" aria-hidden="true"><path d="M2 5.4h2L7 3v8L4 8.6H2z" fill="currentColor"/><path d="M9.2 5a3 3 0 0 1 0 4M10.8 3.4a5.2 5.2 0 0 1 0 7.2" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>`,
  brightness: `<svg width="13" height="13" viewBox="0 0 14 14" aria-hidden="true"><circle cx="7" cy="7" r="2.4" fill="currentColor"/><path d="M7 1.4v1.3M7 11.3v1.3M1.4 7h1.3M11.3 7h1.3M3 3l.9.9M10.1 10.1l.9.9M11 3l-.9.9M3.9 10.1 3 11" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>`,
  media: `<svg width="13" height="13" viewBox="0 0 14 14" aria-hidden="true"><path d="M3 2.6v8.8L8.6 7z" fill="currentColor"/><path d="M10.6 3v8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>`,
  off: `<svg width="13" height="13" viewBox="0 0 14 14" aria-hidden="true"><path d="M3.5 7h7" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>`,
};

function sw(id: string, on: boolean, label: string): string {
  return `<button id="${id}" class="sw" type="button" role="switch" aria-checked="${on ? "true" : "false"}" aria-label="${esc(label)}"><span class="sw-knob"></span></button>`;
}

function row(title: string, desc: string, control: string, cls = ""): string {
  return `<div class="s-row${cls ? ` ${cls}` : ""}">
    <div class="row-text"><div class="row-title">${title}</div>${desc ? `<div class="row-desc">${desc}</div>` : ""}</div>
    <div class="row-ctl">${control}</div>
  </div>`;
}

/** A row that only exists while its parent setting is on. It collapses smoothly. */
function subRow(open: boolean, title: string, desc: string, control: string): string {
  return `<div class="sub" data-open="${open ? "true" : "false"}"${open ? "" : " inert"}><div class="sub-inner">${row(title, desc, control, "is-sub")}</div></div>`;
}

function group(title: string, body: string, foot = ""): string {
  return `<section class="group">${title ? `<h2 class="group-title">${title}</h2>` : ""}<div class="rows">${body}</div>${foot ? `<p class="group-foot">${foot}</p>` : ""}</section>`;
}

function seg(attr: string, label: string, items: readonly (readonly [string, string])[], current: string): string {
  const index = Math.max(0, items.findIndex(([value]) => value === current));
  return `<div class="seg" role="radiogroup" aria-label="${esc(label)}" style="--n:${items.length};--i:${index}"><i class="seg-thumb"></i>${items
    .map(
      ([value, text]) =>
        `<button type="button" role="radio" aria-checked="${value === current ? "true" : "false"}" ${attr}="${esc(value)}">${esc(text)}</button>`,
    )
    .join("")}</div>`;
}

/** A styled button face over a real <select>, so the menu stays native and keyboard friendly. */
function sel(
  attrs: string,
  label: string,
  pairs: readonly (readonly [string, string])[],
  current: string,
  opts: { icon?: string; cls?: string } = {},
): string {
  const text = pairs.find(([value]) => value === current)?.[1] ?? "";
  return `<span class="sel${opts.cls ? ` ${opts.cls}` : ""}">${opts.icon ?? ""}<span class="sel-text">${esc(text)}</span>${ICONS.chevron}<select ${attrs} aria-label="${esc(label)}">${options(pairs, current)}</select></span>`;
}

/** Patches the DOM in place so CSS transitions run and focus survives a re-render. */
function morph(from: Element, to: Element) {
  for (const attr of [...from.attributes]) {
    if (!to.hasAttribute(attr.name)) from.removeAttribute(attr.name);
  }
  for (const attr of [...to.attributes]) {
    if (from.getAttribute(attr.name) !== attr.value) from.setAttribute(attr.name, attr.value);
  }
  if (from instanceof HTMLInputElement && to instanceof HTMLInputElement) {
    if (document.activeElement !== from) from.value = to.getAttribute("value") ?? "";
    if (from.type === "checkbox") from.checked = to.hasAttribute("checked");
    return;
  }
  if (from instanceof HTMLSelectElement && to instanceof HTMLSelectElement) {
    if (from.innerHTML !== to.innerHTML) from.innerHTML = to.innerHTML;
    from.value = to.value;
    return;
  }
  const have = [...from.childNodes];
  const want = [...to.childNodes];
  want.forEach((node, index) => {
    const current = have[index];
    if (!current) {
      from.appendChild(node.cloneNode(true));
    } else if (
      current instanceof Element &&
      node instanceof Element &&
      current.tagName === node.tagName &&
      current.id === node.id &&
      current.getAttribute("data-tab") === node.getAttribute("data-tab") &&
      !current.hasAttribute("data-fresh") &&
      !node.hasAttribute("data-fresh")
    ) {
      morph(current, node);
    } else if (current.nodeType === Node.TEXT_NODE && node.nodeType === Node.TEXT_NODE) {
      if (current.nodeValue !== node.nodeValue) current.nodeValue = node.nodeValue;
    } else {
      current.replaceWith(node.cloneNode(true));
    }
  });
  for (const extra of have.slice(want.length)) extra.remove();
}

// ---------- Trackpad ----------

const EDGES = [
  ["left", "Left edge"],
  ["right", "Right edge"],
  ["top", "Top edge"],
  ["bottom", "Bottom edge"],
] as const;

const CORNERS = [
  ["topLeft", "Top left", "tl"],
  ["topRight", "Top right", "tr"],
  ["bottomLeft", "Bottom left", "bl"],
  ["bottomRight", "Bottom right", "br"],
] as const;

type CornerKey = (typeof CORNERS)[number][0];
let padCorner: CornerKey = "topRight";

function labelOf(pairs: readonly (readonly [string, string])[], value: string): string {
  return pairs.find(([v]) => v === value)?.[1] ?? value;
}

function cornerSummary(actions: CornerActions): string {
  const parts: string[] = [];
  if (actions.single && actions.single !== "none") parts.push(labelOf(CLICK_ACTIONS, actions.single));
  if (actions.double && actions.double !== "none") parts.push(`${labelOf(CLICK_ACTIONS, actions.double)} ×2`);
  if (actions.holdPeer) {
    const who = peers.find((peer) => peer.id === actions.holdPeer)?.name;
    parts.push(who ? `Tap ${who.split(" ")[0]}` : "Tap");
  }
  if (parts.length === 0) return "No action";
  return parts.length > 1 ? `${parts[0]} +${parts.length - 1}` : parts[0];
}

function padDiagram(pad: TrackpadSettings): string {
  const icon = (action: string) => ICONS[action as keyof typeof ICONS] ?? ICONS.off;
  const pill = (key: (typeof EDGES)[number][0], label: string) => {
    const edge = pad.edges[key];
    const value = slideValue(edge);
    return sel(`data-edge="${key}"`, label, SLIDE_ACTIONS, value, { icon: icon(value), cls: `pill ${key}` });
  };
  const bands = EDGES.map(
    ([key]) => `<i class="band ${key}" data-on="${pad.edges[key].enabled ? "true" : "false"}"></i>`,
  ).join("");
  const chips = CORNERS.map(
    ([key, name, pos]) =>
      `<button type="button" class="chip ${pos}" data-corner-pick="${key}" aria-pressed="${padCorner === key ? "true" : "false"}" aria-label="${name} corner: ${esc(cornerSummary(pad.corners[key]))}" title="${name} corner">${esc(cornerSummary(pad.corners[key]))}</button>`,
  ).join("");
  return `<div class="pad${pad.enabled ? "" : " is-off"}">
    <div class="pad-face">
      ${bands}
      ${EDGES.map(([key, label]) => pill(key, label)).join("")}
      ${chips}
      <span class="pad-hint">Slide along an edge</span>
    </div>
  </div>`;
}

function trackpadPanel(current: Settings): string {
  const pad = trackpadOf(current);
  const cornerName = CORNERS.find(([key]) => key === padCorner)?.[1] ?? "";
  const actions = pad.corners[padCorner];
  const people = peers.filter((peer) => peer.name).map((peer) => [peer.id, peer.name] as const);
  const hold = [["", "No one"], ...people] as const;
  const pct = ((pad.sensitivity - 0.4) / 1.6) * 100;
  return `
    ${group("", row("Trackpad gestures", "Edge slides and corner actions on this Mac", sw("trackpad-enabled", pad.enabled, "Trackpad gestures")))}
    ${padDiagram(pad)}
    <p class="pad-note">Pick what each edge does. Select a corner to set its actions below.</p>
    ${group(
      `${cornerName} corner`,
      `<div class="corner-tabs">${seg("data-corner-tab", "Corner", CORNERS.map(([key, name]) => [key, name] as const), padCorner)}</div>` +
        row("Click", "", sel(`data-corner="${padCorner}" data-click="single"`, `${cornerName} click`, CLICK_ACTIONS, actions.single || "none")) +
        row("Double click", "", sel(`data-corner="${padCorner}" data-click="double"`, `${cornerName} double click`, CLICK_ACTIONS, actions.double || "none")) +
        row("Rest a finger to tap", "Hold in this corner to tap someone on your team", sel(`data-corner="${padCorner}" data-click="hold"`, `${cornerName} hold`, hold, actions.holdPeer || "")),
    )}
    ${group(
      "Sliding",
      row(
        "Sensitivity",
        "How far a finger moves the level",
        `<input id="trackpad-sensitivity" class="range" type="range" min="0.4" max="2" step="0.1" value="${pad.sensitivity}" style="--p:${pct}%" aria-label="Sensitivity" /><span id="trackpad-sensitivity-value" class="value">${pad.sensitivity.toFixed(1)}×</span>`,
      ) +
        row(
          "Require a key",
          "Only slide while this key is held",
          seg(
            "data-modifier",
            "Required key",
            [
              ["none", "None"],
              ["fn", "Fn"],
              ["option", "Option"],
              ["control", "Control"],
            ],
            pad.requireModifier || "none",
          ),
        ) +
        row("Keep the pointer still", "The cursor stays put while you slide", sw("trackpad-freeze", pad.freezeCursor, "Keep the pointer still")) +
        row("Precise sliding", "Quarter steps. With this off, hold Shift and Option for the same", sw("trackpad-precise", pad.preciseSliding, "Precise sliding")) +
        row("Flip vertical edges", "For a trackpad that reports up and down backwards", sw("trackpad-flip", pad.flipVerticalEdges, "Flip vertical edges")) +
        row("Reverse media swipes", "Swap which way is next and previous", sw("trackpad-reverse", pad.mediaStripReverse, "Reverse media swipes")),
    )}
    ${group(
      "Feedback",
      row("Haptics", "A tick each time the level crosses a step", sw("trackpad-haptics", pad.hapticsEnabled, "Haptics")) +
        subRow(
          pad.hapticsEnabled,
          "Strength",
          "",
          seg(
            "data-strength",
            "Haptic strength",
            [
              ["light", "Light"],
              ["medium", "Medium"],
              ["strong", "Strong"],
            ],
            pad.hapticStrength || "medium",
          ),
        ) +
        row("Show the level", "A small panel while you slide", sw("trackpad-hud", pad.showHUD, "Show the level")),
    )}`;
}

function bindTrackpad() {
  if (!settings || tab !== "trackpad") return;
  const pad = trackpadOf(settings);
  const save = () => {
    void persist();
    render();
  };
  const toggle = (id: string, apply: (value: boolean) => void) => {
    const button = document.getElementById(id);
    if (!button) return;
    button.onclick = () => {
      apply(button.getAttribute("aria-checked") !== "true");
      save();
    };
  };
  toggle("trackpad-enabled", (value) => (pad.enabled = value));
  toggle("trackpad-flip", (value) => (pad.flipVerticalEdges = value));
  toggle("trackpad-freeze", (value) => (pad.freezeCursor = value));
  toggle("trackpad-precise", (value) => (pad.preciseSliding = value));
  toggle("trackpad-haptics", (value) => (pad.hapticsEnabled = value));
  toggle("trackpad-reverse", (value) => (pad.mediaStripReverse = value));
  toggle("trackpad-hud", (value) => (pad.showHUD = value));

  for (const select of root.querySelectorAll<HTMLSelectElement>("select[data-edge]")) {
    select.onchange = () => {
      const key = select.dataset.edge as keyof TrackpadSettings["edges"] | undefined;
      if (!key) return;
      const edge = pad.edges[key];
      if (select.value === "off") edge.enabled = false;
      else {
        edge.enabled = true;
        edge.action = select.value;
      }
      save();
    };
  }
  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-modifier]")) {
    button.onclick = () => {
      pad.requireModifier = button.dataset.modifier || "none";
      save();
    };
  }
  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-strength]")) {
    button.onclick = () => {
      pad.hapticStrength = button.dataset.strength || "medium";
      save();
    };
  }
  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-corner-pick], [data-corner-tab]")) {
    button.onclick = () => {
      padCorner = (button.dataset.cornerPick || button.dataset.cornerTab) as CornerKey;
      render();
    };
  }
  const sensitivity = document.getElementById("trackpad-sensitivity") as HTMLInputElement | null;
  if (sensitivity) {
    sensitivity.oninput = () => {
      pad.sensitivity = Number(sensitivity.value);
      sensitivity.style.setProperty("--p", `${((pad.sensitivity - 0.4) / 1.6) * 100}%`);
      const value = document.getElementById("trackpad-sensitivity-value");
      if (value) value.textContent = `${pad.sensitivity.toFixed(1)}×`;
    };
    sensitivity.onchange = () => void persist();
  }
  for (const select of root.querySelectorAll<HTMLSelectElement>("select[data-corner]")) {
    select.onchange = () => {
      const key = select.dataset.corner as CornerKey | undefined;
      const which = select.dataset.click;
      if (!key || !which) return;
      const actions = pad.corners[key];
      if (which === "hold") actions.holdPeer = select.value;
      else if (which === "single") actions.single = select.value;
      else actions.double = select.value;
      save();
    };
  }
}

function selectTab(next: Tab | undefined) {
  if (!next || next === tab || !tabs.some((item) => item.id === next)) return;
  tab = next;
  adding = false;
  colorFor = null;
  armingShortcut = false;
  render();
  window.scrollTo(0, 0);
}

function render() {
  const addField = document.getElementById("add-peer") as HTMLInputElement | null;
  const keepAddFocus = addField != null && document.activeElement === addField;
  const addSel: [number, number] | null = keepAddFocus
    ? [addField.selectionStart ?? addDraft.length, addField.selectionEnd ?? addDraft.length]
    : null;
  if (addField) addDraft = addField.value;
  if (!settings) {
    root.innerHTML = `<div class="settings"><p class="error">${esc(error || "Loading settings")}</p></div>`;
    return;
  }
  const glow = GLOW_STYLES.includes(settings.glowStyle) ? settings.glowStyle : "edge";
  const screens = settings.overlayScreens || "all";
  const selected = new Set(settings.selectedDisplays || []);
  const people = peopleRows();
  const currentTheme = normalizeTheme(settings.theme);
  const tabIndex = Math.max(0, tabs.findIndex((item) => item.id === tab));
  const panel =
    tab === "profile"
      ? group(
          "Profile",
          row(
            "Photo",
            "Teammates nearby see it, and again when it changes",
            `<button id="pick-photo" class="avatar photo" type="button" title="Change photo" aria-label="Change photo">${photo ? `<img src="${photo}" alt="" />` : "You"}</button><input id="photo-file" type="file" accept="image/*" hidden data-fresh />`,
          ) +
            row(
              "Name",
              "Shown to your team",
              `<input id="name" class="input" type="text" value="${esc(settings.displayName)}" aria-label="Your name" data-fresh />`,
            ),
        ) +
        group(
          "Team",
          row(
            "Team key",
            "Use the same key on every computer in your team",
            `<input id="key" class="input mono" type="text" value="${esc(settings.teamKey)}" spellcheck="false" aria-label="Team key" data-fresh /><button id="copy" class="btn icon" type="button" title="Copy team key" aria-label="Copy team key">${ICONS.copy}</button>`,
          ),
        )
      : tab === "glow"
        ? group(
            "Display",
            row(
              "Screens",
              "Where the glow appears",
              seg(
                "data-screens",
                "Screens",
                [
                  ["all", "All"],
                  ["cursor", "Pointer"],
                  ["selected", "Choose"],
                ],
                screens,
              ),
            ) +
              (screens === "selected"
                ? displays.length === 0
                  ? row("No displays found", "", "", "is-empty")
                  : displays
                      .map((name) =>
                        row(
                          esc(name),
                          "",
                          `<label class="cb"><input type="checkbox" data-display="${esc(name)}" ${selected.has(name) ? "checked" : ""} aria-label="${esc(name)}" data-fresh /><span class="cb-box"></span></label>`,
                          "is-sub",
                        ),
                      )
                      .join("")
                : ""),
          ) +
          `<section class="group"><h2 class="group-title">Style</h2>
            <div class="tiles" role="radiogroup" aria-label="Glow style">${GLOW_STYLES.map(
              (style) =>
                `<button class="tile" type="button" role="radio" aria-checked="${glow === style ? "true" : "false"}" data-glow="${style}"><span class="tile-art g-${style}"></span><span class="tile-name">${style[0].toUpperCase()}${style.slice(1)}</span></button>`,
            ).join("")}</div>
            <div class="rows">${
              row("Name banner", "Show who tapped you", sw("banner", settings.showBanner !== false, "Show the name banner")) +
              row("Preview", "See the glow on your screens", `<button id="preview-glow" class="btn" type="button" data-fresh>Preview</button>`)
            }</div></section>`
        : tab === "team"
          ? group(
              `People${people.length ? ` · ${people.length}` : ""}`,
              (people.length === 0
                ? row("No one yet", "People show up here after Tap has seen them", "", "is-empty")
                : people
                    .map((person) => {
                      const photoUrl = peerPhotos.get(person.id);
                      const face = photoUrl ? `<img src="${esc(photoUrl)}" alt="" />` : esc(initials(person.name));
                      const open = colorFor === person.id;
                      return `<div class="s-row member" data-person="${esc(person.id)}">
                        <span class="avatar" style="background:${esc(person.color)}">${face}</span>
                        <div class="row-text"><div class="row-title">${esc(person.name)}</div>${person.detail ? `<div class="row-desc mono">${esc(person.detail)}</div>` : ""}</div>
                        <div class="row-ctl">
                          <label class="keycap" title="Tap key. Hold this key to tap ${esc(person.name)}."><input data-hotkey="${esc(person.id)}" data-fresh type="text" inputmode="text" maxlength="1" spellcheck="false" autocomplete="off" value="${esc(person.hotkey)}" placeholder="Key" aria-label="Tap key for ${esc(person.name)}" /></label>
                          <button type="button" class="swatch" data-colors="${esc(person.id)}" style="--swatch:${esc(person.color)}" aria-expanded="${open ? "true" : "false"}" aria-label="Color for ${esc(person.name)}" title="Color"></button>
                          <button type="button" class="btn icon ghost" data-remove="${esc(person.id)}" title="Remove from this computer" aria-label="Remove ${esc(person.name)}">${ICONS.close}</button>
                        </div>
                      </div>`;
                    })
                    .join("")) +
                (adding
                  ? `<div class="s-row is-add"><input id="add-peer" class="input mono wide" type="text" placeholder="Host name or IP, then Enter" value="${esc(addDraft)}" data-fresh /></div>`
                  : `<div class="s-row is-add"><button id="add" class="btn ghost add" type="button" data-fresh>${ICONS.plus}<span>Add someone</span></button></div>`),
              `Hold a person's key after ${linux ? "the Open team list shortcut" : "your Raycast shortcut opens Tap"} to tap them. Press Backspace in a key field to clear it. Colors and removals only apply on this computer.`,
            ) +
            group(
              "Discovery",
              row(
                "Find people",
                esc(refreshedLabel(refreshedAt)),
                `<button id="refresh-people" class="btn" type="button" ${refreshBusy ? "disabled" : ""} data-fresh>${ICONS.refresh}<span>${refreshBusy ? "Refreshing…" : "Refresh"}</span></button>`,
              ) + row("Tailscale only", "Only find people on your tailnet", sw("tailnet", settings.tailnetOnly, "Tailscale only")),
            )
          : tab === "trackpad"
            ? trackpadPanel(settings)
            : group(
                "Appearance",
                row(
                  "Theme",
                  "",
                  seg(
                    "data-theme",
                    "Theme",
                    [
                      ["system", "System"],
                      ["light", "Light"],
                      ["dark", "Dark"],
                    ],
                    currentTheme,
                  ),
                ),
              ) +
              group(
                "General",
                (linux
                  ? row(
                      "Open team list",
                      esc(armingShortcut ? "Press Ctrl, Alt, Shift, or Super with a key. Backspace clears it." : shortcutNote || "Then hold someone's key from Team"),
                      `<button id="roster-shortcut" class="btn${armingShortcut ? " is-armed" : ""}" type="button" data-fresh>${armingShortcut ? "Listening…" : esc(shortcutLabel(settings.rosterShortcut) || "Set")}</button>`,
                    )
                  : "") +
                  row("Launch at login", "Open Tap when you sign in", sw("login", settings.launchAtLogin, "Launch at login")) +
                  row("Follow system Focus", "Also pause taps while the system is in Focus", sw("focus", settings.followSystemDnd, "Follow system Focus")),
              ) +
              group(
                "About",
                row(
                  "Updates",
                  esc(updateNote || (appVersion ? `Version ${appVersion}` : "Release builds can update from GitHub")),
                  `<button id="check-update" class="btn" type="button" ${updateBusy || preview ? "disabled" : ""} data-fresh>${esc(updateLabel)}</button>`,
                ),
              );

  const shell = `
    <div class="settings">
      <header class="bar">
        <div class="wrap">
          <div class="tabs" role="tablist" aria-label="Settings" style="--n:${tabs.length};--i:${tabIndex}">
            ${tabs
              .map(
                (item) =>
                  `<button type="button" role="tab" id="tab-${item.id}" data-tab="${item.id}" aria-selected="${tab === item.id ? "true" : "false"}" aria-controls="panel" tabindex="${tab === item.id ? "0" : "-1"}">${item.label}</button>`,
              )
              .join("")}
            <i class="tab-ind" aria-hidden="true"></i>
          </div>
        </div>
      </header>
      <main class="wrap tab-panel" id="panel" role="tabpanel" aria-labelledby="tab-${tab}" data-tab="${tab}">
        ${error ? `<p class="error" role="alert">${esc(error)}</p>` : ""}
        ${panel}
      </main>
    </div>
  `;
  const template = document.createElement("template");
  template.innerHTML = shell.trim();
  const next = template.content.firstElementChild!;
  const current = root.firstElementChild;
  if (current && current.classList.contains("settings")) morph(current, next);
  else root.replaceChildren(next);

  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-tab]")) {
    button.onclick = () => selectTab(button.dataset.tab as Tab | undefined);
  }

  const name = document.getElementById("name") as HTMLInputElement | null;
  name?.addEventListener("change", () => {
    settings!.displayName = name.value.trim();
    void persist();
  });
  const key = document.getElementById("key") as HTMLInputElement | null;
  key?.addEventListener("change", () => {
    settings!.teamKey = key.value;
    void persist();
  });
  const copy = document.getElementById("copy");
  if (copy) {
    copy.onclick = () => {
      void navigator.clipboard.writeText(settings!.teamKey).catch(() => {});
      copy.classList.add("is-done");
      copy.setAttribute("title", "Copied");
      window.setTimeout(() => {
        copy.classList.remove("is-done");
        copy.setAttribute("title", "Copy team key");
      }, 1400);
    };
  }
  const pickPhoto = document.getElementById("pick-photo");
  if (pickPhoto) {
    pickPhoto.onclick = () => {
      document.getElementById("photo-file")?.click();
    };
  }
  document.getElementById("photo-file")?.addEventListener("change", () => {
    const input = document.getElementById("photo-file") as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    void file.arrayBuffer().then(async (buffer) => {
      try {
        const hash = await invoke<string>("set_avatar", { bytes: Array.from(new Uint8Array(buffer)) });
        photo = (await invoke<string | null>("my_avatar")) || "";
        void hash;
        render();
      } catch (err) {
        error = typeof err === "string" ? err : "Couldn't save that photo.";
        render();
      }
    });
  });
  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-screens]")) {
    button.onclick = () => {
      settings!.overlayScreens = button.dataset.screens || "all";
      void persist();
      render();
    };
  }
  for (const box of root.querySelectorAll<HTMLInputElement>("[data-display]")) {
    box.onchange = () => {
      const name = box.dataset.display || "";
      const next = new Set(settings!.selectedDisplays || []);
      if (box.checked) next.add(name);
      else next.delete(name);
      settings!.selectedDisplays = [...next];
      void persist();
    };
  }
  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-glow]")) {
    button.onclick = () => {
      settings!.glowStyle = button.dataset.glow || "edge";
      void persist();
      render();
    };
  }
  const banner = document.getElementById("banner");
  if (banner) {
    banner.onclick = () => {
      settings!.showBanner = banner.getAttribute("aria-checked") !== "true";
      void persist();
      render();
    };
  }
  const previewGlow = document.getElementById("preview-glow");
  if (previewGlow) {
    previewGlow.onclick = () => {
      void (async () => {
        await persist();
        try {
          await invoke("preview_glow");
        } catch (err) {
          error = typeof err === "string" ? err : "Couldn't preview the glow.";
          render();
        }
      })();
    };
  }
  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-theme]")) {
    button.onclick = () => {
      settings!.theme = button.dataset.theme || "system";
      applyTheme(settings!.theme);
      void persist();
      render();
    };
  }
  document.getElementById("roster-shortcut")?.addEventListener("click", () => {
    armingShortcut = true;
    render();
  });
  bindTrackpad();
  const login = document.getElementById("login");
  if (login) {
    login.onclick = () => {
      settings!.launchAtLogin = !settings!.launchAtLogin;
      void persist();
      render();
    };
  }
  const focus = document.getElementById("focus");
  if (focus) {
    focus.onclick = () => {
      settings!.followSystemDnd = !settings!.followSystemDnd;
      void persist();
      render();
    };
  }
  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-colors]")) {
    button.onclick = () => {
      const id = button.dataset.colors || "";
      if (colorFor === id) {
        colorFor = null;
        render();
        return;
      }
      const person = peopleRows().find((row) => row.id === id);
      dial = hexToHsl(person?.color || "#F86E51");
      dialDirty = false;
      colorBefore = {
        hadSaved: Object.prototype.hasOwnProperty.call(settings?.peerColors || {}, id),
        color: settings?.peerColors?.[id] ?? person?.color,
      };
      colorFor = id;
      render();
    };
  }
  for (const input of root.querySelectorAll<HTMLInputElement>("[data-hotkey]")) {
    input.addEventListener("keydown", (event) => {
      event.stopPropagation();
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const id = input.dataset.hotkey || "";
      if (event.key === "Escape") {
        event.preventDefault();
        input.blur();
        return;
      }
      if (event.key === "Backspace" || event.key === "Delete") {
        event.preventDefault();
        void assignKey(id, "");
        return;
      }
      if (event.key.length !== 1) return;
      event.preventDefault();
      void assignKey(id, event.key);
    });
    input.addEventListener("paste", (event) => {
      event.preventDefault();
      const text = event.clipboardData?.getData("text") ?? "";
      void assignKey(input.dataset.hotkey || "", text.trim());
    });
  }
  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-remove]")) {
    button.onclick = () => {
      const id = button.dataset.remove || "";
      if (id) void removePerson(id);
    };
  }
  document.getElementById("refresh-people")?.addEventListener("click", () => {
    void refreshPeople();
  });
  document.getElementById("add")?.addEventListener("click", () => {
    adding = true;
    addDraft = "";
    render();
    document.getElementById("add-peer")?.focus();
  });
  const add = document.getElementById("add-peer") as HTMLInputElement | null;
  add?.addEventListener("input", () => {
    addDraft = add.value;
  });
  add?.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      const value = add.value.trim();
      adding = false;
      addDraft = "";
      if (value) {
        const key = value.toLowerCase();
        if (!settings!.manualPeers.some((entry) => entry.trim().toLowerCase() === key)) {
          settings!.manualPeers = [...settings!.manualPeers, value];
        }
        const parsed = parseManual(value, 0);
        if (parsed) {
          settings!.hiddenPeers = (settings!.hiddenPeers ?? []).filter((id) => id !== parsed.id);
        }
      }
      void persist();
      render();
    } else if (event.key === "Escape") {
      adding = false;
      addDraft = "";
      render();
    }
  });
  const tailnet = document.getElementById("tailnet");
  if (tailnet) {
    tailnet.onclick = () => {
      settings!.tailnetOnly = !settings!.tailnetOnly;
      void persist();
      render();
    };
  }
  document.getElementById("check-update")?.addEventListener("click", () => {
    void checkForUpdates();
  });
  mountColorPop();
  if (keepAddFocus) {
    const restored = document.getElementById("add-peer") as HTMLInputElement | null;
    restored?.focus();
    if (restored && addSel) restored.setSelectionRange(addSel[0], addSel[1]);
  }
}

function dialColor(): string {
  return hslToHex(dial.h, dial.s);
}

function applyColorLocal(id: string, color: string) {
  const peer = peers.find((item) => item.id === id);
  if (peer) peer.color = color;
  if (settings) settings.peerColors = { ...(settings.peerColors || {}), [id]: color };
  const row = document.querySelector<HTMLElement>(`[data-person="${CSS.escape(id)}"]`);
  const avatar = row?.querySelector<HTMLElement>(".avatar");
  if (avatar) avatar.style.background = color;
  const swatch = row?.querySelector<HTMLElement>(".swatch");
  if (swatch) swatch.style.setProperty("--swatch", color);
}

function commitColor() {
  if (!colorFor || !dialDirty || !settings) return;
  const id = colorFor;
  const color = dialColor();
  const before = colorBefore;
  applyColorLocal(id, color);
  dialDirty = false;
  const gen = ++colorSaveGen;
  void invoke("set_peer_color", { peerId: id, color })
    .then(() => {
      if (gen !== colorSaveGen) return;
      colorBefore = { hadSaved: true, color };
      if (error) {
        error = "";
        if (!sliding) render();
      }
    })
    .catch((err) => {
      if (gen !== colorSaveGen) return;
      const peer = peers.find((item) => item.id === id);
      if (peer) peer.color = before?.color;
      if (before?.hadSaved && before.color) settings!.peerColors[id] = before.color;
      else delete settings!.peerColors[id];
      if (before?.color) dial = hexToHsl(before.color);
      dialDirty = false;
      error = typeof err === "string" ? err : "Couldn't save that color.";
      render();
    });
}

function paintDials() {
  const pop = document.getElementById("color-pop");
  if (!pop) return;
  const color = dialColor();
  const hue = pop.querySelector<HTMLElement>('[data-dial="hue"]');
  const sat = pop.querySelector<HTMLElement>('[data-dial="sat"]');
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
  if (colorFor) applyColorLocal(colorFor, color);
}

function placeColorPop() {
  const pop = document.getElementById("color-pop");
  if (!pop || !colorFor) return;
  const swatch = document.querySelector<HTMLElement>(`[data-colors="${CSS.escape(colorFor)}"]`);
  if (!swatch) return;
  const rect = swatch.getBoundingClientRect();
  const popRect = pop.getBoundingClientRect();
  const margin = 8;
  let top = rect.bottom + margin;
  if (top + popRect.height > window.innerHeight - margin) top = Math.max(margin, rect.top - margin - popRect.height);
  let left = rect.right - popRect.width;
  left = Math.max(margin, Math.min(left, window.innerWidth - popRect.width - margin));
  pop.style.top = `${top}px`;
  pop.style.left = `${left}px`;
}

function setDialFromPointer(dialEl: HTMLElement, kind: "hue" | "sat", clientX: number) {
  const rect = dialEl.getBoundingClientRect();
  const pad = 11;
  const span = Math.max(1, rect.width - pad * 2);
  const t = clamp((clientX - rect.left - pad) / span, 0, 1);
  if (kind === "hue") dial.h = t * 360;
  else dial.s = t * 100;
  dialDirty = true;
  paintDials();
}

function mountColorPop() {
  document.getElementById("color-pop")?.remove();
  if (!colorFor || tab !== "team") return;
  const color = dialColor();
  const pop = document.createElement("div");
  pop.id = "color-pop";
  pop.className = "color-pop";
  pop.setAttribute("role", "dialog");
  pop.setAttribute("aria-label", "Color");
  pop.innerHTML = `
    <div class="dial-block">
      <div class="dial-label">Hue</div>
      <div class="dial" data-dial="hue" role="slider" tabindex="0" aria-label="Hue" aria-valuemin="0" aria-valuemax="360" aria-valuenow="${Math.round(dial.h)}" style="--track:${hueGradient()}">
        <span class="knob" style="--t:${dial.h / 360}; --knob:${color}"></span>
      </div>
    </div>
    <div class="dial-block">
      <div class="dial-label">Saturation</div>
      <div class="dial" data-dial="sat" role="slider" tabindex="0" aria-label="Saturation" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${Math.round(dial.s)}" style="--track:linear-gradient(90deg, ${hslToHex(dial.h, 0)}, ${hslToHex(dial.h, 100)})">
        <span class="knob" style="--t:${dial.s / 100}; --knob:${color}"></span>
      </div>
    </div>`;
  document.body.appendChild(pop);
  for (const dialEl of pop.querySelectorAll<HTMLElement>("[data-dial]")) {
    const kind = dialEl.dataset.dial === "sat" ? "sat" : "hue";
    dialEl.addEventListener("pointerdown", (event) => {
      sliding = true;
      setDialFromPointer(dialEl, kind, event.clientX);
      try {
        dialEl.setPointerCapture(event.pointerId);
      } catch {
        // Some events cannot capture the pointer. The down position still applies.
      }
    });
    dialEl.addEventListener("pointermove", (event) => {
      if (!dialEl.hasPointerCapture(event.pointerId)) return;
      setDialFromPointer(dialEl, kind, event.clientX);
    });
    const finish = () => {
      if (!sliding) return;
      sliding = false;
      commitColor();
    };
    dialEl.addEventListener("pointerup", finish);
    dialEl.addEventListener("pointercancel", finish);
    dialEl.addEventListener("keydown", (event) => {
      const step = event.shiftKey ? 10 : 2;
      let used = true;
      if (event.key === "ArrowRight" || event.key === "ArrowUp") {
        if (kind === "hue") dial.h = (dial.h + step) % 360;
        else dial.s = clamp(dial.s + step, 0, 100);
      } else if (event.key === "ArrowLeft" || event.key === "ArrowDown") {
        if (kind === "hue") dial.h = (dial.h - step + 360) % 360;
        else dial.s = clamp(dial.s - step, 0, 100);
      } else {
        used = false;
      }
      if (!used) return;
      event.preventDefault();
      dialDirty = true;
      paintDials();
      commitColor();
    });
  }
  placeColorPop();
}

async function checkForUpdates() {
  if (preview || updateBusy) return;
  updateBusy = true;
  updateLabel = "Checking…";
  render();
  try {
    const update = await check();
    if (!update) {
      updateNote = appVersion ? `Version ${appVersion} is current` : "You're on the latest version";
      updateLabel = "Check for updates";
      updateBusy = false;
      render();
      return;
    }
    updateLabel = "Downloading…";
    updateNote = `Installing ${update.version}`;
    render();
    await update.downloadAndInstall();
    updateNote = "Restarting…";
    render();
    await relaunch();
  } catch (err) {
    updateBusy = false;
    updateLabel = "Check for updates";
    updateNote = typeof err === "string" ? err : err instanceof Error ? err.message : "Couldn't check for updates.";
    render();
  }
}

async function persist() {
  if (!settings || preview) return;
  try {
    const saved = await invoke<Settings>("save_app_settings", { settings });
    settings.manualPeers = saved.manualPeers;
    settings.hiddenPeers = saved.hiddenPeers;
    error = "";
    if (tab === "team" && !sliding && !adding) render();
  } catch (err) {
    error = typeof err === "string" ? err : "Couldn't save settings.";
    render();
  }
}

async function loadPeerPhoto(peer: TeamPeer) {
  if (!peer.avatarHash || peerPhotos.has(`${peer.id}:${peer.avatarHash}`)) return;
  try {
    const url = await invoke<string | null>("peer_avatar", { peerId: peer.id, hash: peer.avatarHash });
    if (!url) return;
    peerPhotos.set(peer.id, url);
    peerPhotos.set(`${peer.id}:${peer.avatarHash}`, url);
    if (tab === "team") render();
  } catch {
    // The file may still be downloading.
  }
}

async function boot() {
  if (preview) {
    settings = preview.settings;
    peers = preview.peers ?? [];
    applyTheme(settings.theme);
    render();
    return;
  }
  try {
    settings = await invoke<Settings>("get_settings");
    applyTheme(settings.theme);
    displays = await invoke<string[]>("list_displays");
    photo = (await invoke<string | null>("my_avatar")) || "";
    peers = await invoke<TeamPeer[]>("list_peers");
    if (linux) shortcutNote = await invoke<string>("roster_shortcut_error");
    void Promise.all(peers.map((peer) => loadPeerPhoto(peer)));
  } catch (err) {
    error = typeof err === "string" ? err : "Couldn't load settings.";
  }
  try {
    appVersion = await getVersion();
  } catch {
    appVersion = "";
  }
  render();
  void listen<number>("discovery-refreshed", (event) => {
    refreshedAt = event.payload;
    refreshBusy = false;
    if (tab === "team" && !sliding) render();
  }).catch(() => {});
  try {
    refreshedAt = await invoke<number | null>("discovery_refreshed_at");
    if (tab === "team") render();
  } catch {
    // Discovery has not run yet.
  }
  void listen<TeamPeer[]>("peers-updated", (event) => {
    peers = event.payload;
    void Promise.all(peers.map((peer) => loadPeerPhoto(peer)));
    if (tab === "team" && !sliding) render();
  }).catch(() => {});
  window.addEventListener("keydown", (event) => {
    if (armingShortcut) {
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat) return;
      if (event.key === "Escape") {
        armingShortcut = false;
        render();
        return;
      }
      if (event.key === "Backspace" || event.key === "Delete") {
        void saveRosterShortcut("");
        return;
      }
      const chord = chordFromEvent(event);
      if (chord) void saveRosterShortcut(chord);
      return;
    }
    if (event.key === "Escape" && colorFor) {
      commitColor();
      colorFor = null;
      sliding = false;
      render();
      return;
    }
    if (event.key === "Escape" && !adding) {
      hideSettings();
    }
  });
  document.addEventListener("pointerdown", (event) => {
    if (!colorFor || sliding) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    if (target.closest("#color-pop") || target.closest("[data-colors]")) return;
    commitColor();
    colorFor = null;
    render();
  });
  document.addEventListener("scroll", () => placeColorPop(), true);
}

window.addEventListener("keydown", (event) => {
  if (armingShortcut) return;
  if ((event.altKey || event.metaKey || event.ctrlKey) && /^[1-9]$/.test(event.key)) {
    const target = tabs[Number(event.key) - 1];
    if (target) {
      event.preventDefault();
      selectTab(target.id);
    }
    return;
  }
  const focused = document.activeElement;
  if ((event.key === "ArrowRight" || event.key === "ArrowLeft") && focused instanceof HTMLButtonElement) {
    const group = focused.closest<HTMLElement>(".tabs, .seg, .tiles");
    if (group) {
      const items = [...group.querySelectorAll<HTMLButtonElement>("button")];
      const at = items.indexOf(focused);
      const step = event.key === "ArrowRight" ? 1 : -1;
      const target = items[(at + step + items.length) % items.length];
      event.preventDefault();
      target.focus();
      target.click();
      return;
    }
  }
});

void boot();
