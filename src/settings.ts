import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "./styles.css";
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

type Tab = "profile" | "glow" | "team" | "app";

const tabs: { id: Tab; label: string }[] = [
  { id: "profile", label: "Profile" },
  { id: "glow", label: "Glow" },
  { id: "team", label: "Team members" },
  { id: "app", label: "App" },
];

let tab: Tab = "profile";
let armingShortcut = false;
let shortcutNote = "";
const linux = /Linux/i.test(navigator.userAgent);

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
  const panel =
    tab === "profile"
      ? `
      <div class="profile">
        <button id="pick-photo" class="avatar photo" type="button" title="Choose photo">${photo ? `<img src="${photo}" alt="" />` : "You"}</button>
        <div>
          <strong>Profile photo</strong>
          <small>People on your team load it when you are nearby, and again when it changes.</small>
        </div>
        <input id="photo-file" type="file" accept="image/*" hidden />
      </div>
      <label class="block"><span>Your name</span>
        <input id="name" type="text" value="${esc(settings.displayName)}" />
      </label>
      <label class="block"><span>Team key</span>
        <div class="key-field">
          <input id="key" class="mono" type="text" value="${esc(settings.teamKey)}" spellcheck="false" />
          <button id="copy" class="copy" type="button">Copy</button>
        </div>
      </label>`
      : tab === "glow"
        ? `
      <div>
        <div class="swatch-label">Screens</div>
        <div class="choices">
          ${["all", "cursor", "selected"]
            .map((mode) => {
              const label = mode === "all" ? "All displays" : mode === "cursor" ? "Under the pointer" : "Choose";
              return `<button class="choice${screens === mode ? " selected" : ""}" type="button" data-screens="${mode}">${label}</button>`;
            })
            .join("")}
        </div>
        ${
          screens === "selected"
            ? displays
                .map(
                  (name) =>
                    `<label class="check"><input type="checkbox" data-display="${esc(name)}" ${selected.has(name) ? "checked" : ""} />${esc(name)}</label>`,
                )
                .join("")
            : ""
        }
      </div>
      <div>
        <div class="swatch-label">Glow</div>
        <div class="choices">
          ${GLOW_STYLES.map(
            (style) =>
              `<button class="choice${glow === style ? " selected" : ""}" type="button" data-glow="${style}">${style[0].toUpperCase()}${style.slice(1)}</button>`,
          ).join("")}
        </div>
        <label class="check"><input id="banner" type="checkbox" ${settings.showBanner !== false ? "checked" : ""} />Show the name banner</label>
        <button id="preview-glow" class="preview" type="button">Preview</button>
      </div>`
        : tab === "team"
          ? `
      <div>
        <div class="swatch-label">People</div>
        ${
          peopleRows().length === 0
            ? `<p class="note">People show up here after Tap has seen them.</p>`
            : peopleRows()
                .map((person) => {
                  const photoUrl = peerPhotos.get(person.id);
                  const face = photoUrl ? `<img src="${esc(photoUrl)}" alt="" />` : esc(initials(person.name));
                  const open = colorFor === person.id;
                  return `<div class="member" data-person="${esc(person.id)}">
                    <span class="avatar" style="background:${esc(person.color)}">${face}</span>
                    <span class="member-name">
                      <span class="member-title">${esc(person.name)}</span>
                      ${person.detail ? `<span class="member-detail">${esc(person.detail)}</span>` : ""}
                    </span>
                    <input class="member-key" data-hotkey="${esc(person.id)}" type="text" inputmode="text" maxlength="1" spellcheck="false" autocomplete="off" value="${esc(person.hotkey)}" placeholder="·" aria-label="Key for ${esc(person.name)}" title="Press a letter or number. Backspace clears it." />
                    <button type="button" class="swatch" data-colors="${esc(person.id)}" style="--swatch:${esc(person.color)}" aria-expanded="${open ? "true" : "false"}" aria-label="Color for ${esc(person.name)}"></button>
                    <button type="button" class="member-remove" data-remove="${esc(person.id)}">Remove</button>
                  </div>`;
                })
                .join("")
        }
        ${
          adding
            ? `<input id="add-peer" class="add-input mono" type="text" placeholder="Host name or IP" value="${esc(addDraft)}" />`
            : `<button id="add" class="add" type="button">+ Add someone</button>`
        }
        <p class="note">Give each person one key on this computer. ${linux ? "After the Open team list shortcut," : "After your Raycast shortcut opens Tap,"} hold that key to tap them. Backspace clears a key. The color stays on this computer. Remove hides someone here.</p>
      </div>
      <div class="update-row">
        <div><strong>Find people</strong><small>${esc(refreshedLabel(refreshedAt))}</small></div>
        <button id="refresh-people" type="button" ${refreshBusy ? "disabled" : ""}>${refreshBusy ? "Refreshing…" : "Refresh"}</button>
      </div>
      <div class="toggle-row">
        <div><strong>Tailscale only</strong><small>Only find people on your tailnet</small></div>
        <button id="tailnet" class="switch" type="button" role="switch" aria-checked="${settings.tailnetOnly ? "true" : "false"}">
          <span class="knob"></span>
        </button>
      </div>`
          : `
      <div>
        <div class="swatch-label">Appearance</div>
        <div class="choices">
          ${["system", "light", "dark"]
            .map((mode) => {
              const label = mode === "system" ? "Follow system" : mode === "light" ? "Light" : "Dark";
              const current = normalizeTheme(settings!.theme);
              return `<button class="choice${current === mode ? " selected" : ""}" type="button" data-theme="${mode}">${label}</button>`;
            })
            .join("")}
        </div>
      </div>
      ${
          linux
            ? `<div class="update-row">
        <div><strong>Open team list</strong><small>${esc(armingShortcut ? "Press Ctrl, Alt, Shift, or Super with a key. Backspace clears it." : shortcutNote || "Then hold someone's key from Team members.")}</small></div>
        <button id="roster-shortcut" type="button">${armingShortcut ? "Listening…" : esc(shortcutLabel(settings.rosterShortcut) || "Set")}</button>
      </div>`
            : ""
        }
      <div class="toggle-row">
        <div><strong>Launch at login</strong><small>Open Tap when you sign in</small></div>
        <button id="login" class="switch" type="button" role="switch" aria-checked="${settings.launchAtLogin ? "true" : "false"}">
          <span class="knob"></span>
        </button>
      </div>
      <div class="toggle-row">
        <div><strong>Follow system Focus</strong><small>Also pause taps while the system is in Focus</small></div>
        <button id="focus" class="switch" type="button" role="switch" aria-checked="${settings.followSystemDnd ? "true" : "false"}">
          <span class="knob"></span>
        </button>
      </div>
      <div class="update-row">
        <div><strong>Updates</strong><small>${esc(updateNote || (appVersion ? `Version ${appVersion}` : "Release builds can update from GitHub"))}</small></div>
        <button id="check-update" type="button" ${updateBusy || preview ? "disabled" : ""}>${esc(updateLabel)}</button>
      </div>`;

  root.innerHTML = `
    <div class="settings">
      <div class="tabs-bar">
        <div class="tabs" role="tablist">
          ${tabs
            .map(
              (item) =>
                `<button type="button" role="tab" data-tab="${item.id}" aria-selected="${tab === item.id ? "true" : "false"}">${item.label}</button>`,
            )
            .join("")}
        </div>
      </div>
      <div class="tab-panel" role="tabpanel">
        ${error ? `<p class="error">${esc(error)}</p>` : ""}
        ${panel}
      </div>
    </div>
  `;

  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-tab]")) {
    button.onclick = () => {
      const next = button.dataset.tab as Tab | undefined;
      if (!next || next === tab) return;
      tab = next;
      adding = false;
      colorFor = null;
      armingShortcut = false;
      render();
      window.scrollTo(0, 0);
    };
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
  document.getElementById("banner")?.addEventListener("change", () => {
    const box = document.getElementById("banner") as HTMLInputElement;
    settings!.showBanner = box.checked;
    void persist();
  });
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

void boot();
