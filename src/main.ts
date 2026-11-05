import "./styles.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Peer = {
  id: string;
  name: string;
  online: boolean;
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

let peers: Peer[] = [];
let holding = false;
let previewing = false;
let activeTab: "peers" | "settings" = "peers";
let currentSettings: Settings | null = null;
let statusMessage = "";

const app = document.querySelector<HTMLDivElement>("#app")!;

function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function render() {
  const teamKey = currentSettings?.teamKey ?? "";
  app.innerHTML = `
    <header>
      <h1>Tap</h1>
      <div class="tabs">
        <button id="tab-peers" class="${activeTab === "peers" ? "active" : ""}">Peers</button>
        <button id="tab-settings" class="${activeTab === "settings" ? "active" : ""}">Settings</button>
      </div>
    </header>
    <div id="status" class="status">${esc(statusMessage)}</div>
    <section id="peers-panel" style="display:${activeTab === "peers" ? "block" : "none"}">
      <div class="try-card">
        <div class="try-title">Try it on this Mac</div>
        <p class="hint">No second computer needed. This flashes the border on your screens for a few seconds.</p>
        <button id="preview-btn" class="save-btn" ${previewing ? "disabled" : ""}>${
          previewing ? "Preview running…" : "Preview on this Mac"
        }</button>
        <label class="key-label">Team key
          <div class="key-row">
            <input id="team-key" type="text" readonly value="${esc(teamKey)}" />
            <button id="copy-key" type="button">Copy</button>
          </div>
        </label>
        <p class="hint">On another computer, install Tap and paste this same key in Settings.</p>
      </div>
      <p class="hint">Press and hold a peer to signal their screen. Release to stop.</p>
      <ul class="peer-list" id="peer-list"></ul>
    </section>
    <section id="settings-panel" class="settings ${activeTab === "settings" ? "visible" : ""}"></section>
  `;

  document.getElementById("tab-peers")!.onclick = () => {
    activeTab = "peers";
    render();
  };
  document.getElementById("tab-settings")!.onclick = () => {
    activeTab = "settings";
    render();
  };
  document.getElementById("preview-btn")!.onclick = () => {
    void runPreview();
  };
  document.getElementById("copy-key")!.onclick = () => {
    void copyTeamKey();
  };

  const list = document.getElementById("peer-list")!;
  if (peers.length === 0) {
    list.innerHTML = `<li class="hint">No other machines yet. They show up here when they use this team key on the same network.</li>`;
  } else {
    for (const peer of peers) {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.className = `peer-row${holding ? " holding" : ""}`;
      btn.innerHTML = `<span class="dot ${peer.online ? "online" : ""}"></span><span class="peer-name">${esc(peer.name)}</span><span>Hold</span>`;
      bindHold(btn, peer.id);
      li.appendChild(btn);
      list.appendChild(li);
    }
  }

  if (activeTab === "settings") {
    void renderSettings();
  }
}

async function renderSettings() {
  let settings: Settings;
  try {
    settings = currentSettings ?? (await invoke<Settings>("get_settings"));
  } catch (e) {
    setStatus(formatErr(e));
    return;
  }
  currentSettings = settings;
  const panel = document.getElementById("settings-panel");
  if (!panel) return;
  panel.innerHTML = `
    <label>Display name<input id="displayName" type="text" value="${esc(settings.displayName)}" /></label>
    <label>Your signal color<input id="senderColor" type="text" value="${esc(settings.senderColor)}" /></label>
    <label>Team key<input id="teamKey" type="text" value="${esc(settings.teamKey)}" /></label>
    <p class="hint">Already filled in. Use the same value on every machine.</p>
    <label>Manual peers (one per line, name@host or host)
      <textarea id="manualPeers" rows="3">${esc(settings.manualPeers.join("\n"))}</textarea>
    </label>
    <label class="row"><input id="tailnetOnly" type="checkbox" ${settings.tailnetOnly ? "checked" : ""}/> Accept only Tailscale IPs (100.64.0.0/10)</label>
    <label class="row"><input id="doNotDisturb" type="checkbox" ${settings.doNotDisturb ? "checked" : ""}/> Do Not Disturb</label>
    <label class="row"><input id="chimeEnabled" type="checkbox" ${settings.chimeEnabled ? "checked" : ""}/> Soft chime when tapped</label>
    <label class="row"><input id="launchAtLogin" type="checkbox" ${settings.launchAtLogin ? "checked" : ""}/> Launch at login</label>
    <button class="save-btn" id="save-settings">Save settings</button>
  `;
  document.getElementById("save-settings")!.onclick = async () => {
    const next: Settings = {
      displayName: (document.getElementById("displayName") as HTMLInputElement).value.trim(),
      senderColor: (document.getElementById("senderColor") as HTMLInputElement).value.trim(),
      teamKey: (document.getElementById("teamKey") as HTMLInputElement).value,
      manualPeers: (document.getElementById("manualPeers") as HTMLTextAreaElement).value
        .split("\n")
        .map((s) => s.trim())
        .filter(Boolean),
      tailnetOnly: (document.getElementById("tailnetOnly") as HTMLInputElement).checked,
      doNotDisturb: (document.getElementById("doNotDisturb") as HTMLInputElement).checked,
      chimeEnabled: (document.getElementById("chimeEnabled") as HTMLInputElement).checked,
      launchAtLogin: (document.getElementById("launchAtLogin") as HTMLInputElement).checked,
      listenPort: settings.listenPort,
    };
    try {
      await invoke("save_app_settings", { settings: next });
      currentSettings = next;
      setStatus("Settings saved.");
    } catch (e) {
      setStatus(formatErr(e));
    }
  };
}

async function runPreview() {
  if (previewing) return;
  previewing = true;
  render();
  try {
    await invoke("preview_local");
    setStatus("Look at your screens. The border stays up for a couple of seconds.");
  } catch (e) {
    previewing = false;
    setStatus(formatErr(e));
    render();
    return;
  }
  window.setTimeout(() => {
    previewing = false;
    setStatus("Preview finished.");
    render();
  }, 2600);
}

async function copyTeamKey() {
  const key = currentSettings?.teamKey ?? "";
  if (!key) {
    setStatus("No team key yet.");
    return;
  }
  try {
    await navigator.clipboard.writeText(key);
    setStatus("Team key copied.");
  } catch {
    setStatus("Select the team key and copy it.");
  }
}

function bindHold(el: HTMLElement, peerId: string) {
  const start = async (ev: Event) => {
    ev.preventDefault();
    if (holding) return;
    try {
      await invoke("hold_start", { peerId });
      holding = true;
      render();
    } catch (e) {
      setStatus(formatErr(e));
    }
  };
  const stop = async () => {
    if (!holding) return;
    holding = false;
    await invoke("hold_stop");
    render();
  };
  el.addEventListener("pointerdown", start);
  el.addEventListener("pointerup", stop);
  el.addEventListener("pointerleave", stop);
  el.addEventListener("pointercancel", stop);
}

function setStatus(msg: string) {
  statusMessage = msg;
  const el = document.getElementById("status");
  if (el) el.textContent = msg;
}

function formatErr(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) {
    return String((e as { message: unknown }).message);
  }
  return String(e);
}

void listen<Peer[]>("peers-updated", (event) => {
  peers = event.payload;
  render();
}).catch(() => {});

async function boot() {
  try {
    currentSettings = await invoke<Settings>("get_settings");
    peers = await invoke<Peer[]>("list_peers");
  } catch (e) {
    statusMessage = formatErr(e);
  }
  render();
}

void boot();
