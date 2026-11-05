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
};

let peers: Peer[] = [];
let holding = false;
let activeTab: "peers" | "settings" = "peers";

const app = document.querySelector<HTMLDivElement>("#app")!;

function render() {
  app.innerHTML = `
    <header>
      <h1>Tap</h1>
      <div class="tabs">
        <button id="tab-peers" class="${activeTab === "peers" ? "active" : ""}">Peers</button>
        <button id="tab-settings" class="${activeTab === "settings" ? "active" : ""}">Settings</button>
      </div>
    </header>
    <div id="status" class="status"></div>
    <section id="peers-panel" style="display:${activeTab === "peers" ? "block" : "none"}">
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
    renderSettings();
  };

  const list = document.getElementById("peer-list")!;
  if (peers.length === 0) {
    list.innerHTML = `<li class="hint">No peers yet. Set the same team key on both machines and wait for discovery.</li>`;
  } else {
    for (const peer of peers) {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.className = `peer-row${holding ? " holding" : ""}`;
      btn.innerHTML = `<span class="dot ${peer.online ? "online" : ""}"></span><span class="peer-name">${peer.name}</span><span>Hold</span>`;
      bindHold(btn, peer.id);
      li.appendChild(btn);
      list.appendChild(li);
    }
  }

  if (activeTab === "settings") {
    renderSettings();
  }
}

async function renderSettings() {
  const settings = await invoke<Settings>("get_settings");
  const panel = document.getElementById("settings-panel")!;
  panel.innerHTML = `
    <label>Display name<input id="displayName" type="text" value="${settings.displayName}" /></label>
    <label>Your signal color<input id="senderColor" type="text" value="${settings.senderColor}" /></label>
    <label>Team key (shared secret)<input id="teamKey" type="password" value="${settings.teamKey}" placeholder="same on every teammate" /></label>
    <label>Manual peers (one per line, name@host or host)
      <textarea id="manualPeers" rows="3">${settings.manualPeers.join("\n")}</textarea>
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
    };
    await invoke("save_app_settings", { settings: next });
    setStatus("Settings saved.");
  };
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
      setStatus(String(e));
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
  const el = document.getElementById("status");
  if (el) el.textContent = msg;
}

async function refreshPeers() {
  peers = await invoke<Peer[]>("list_peers");
  render();
}

listen<Peer[]>("peers-updated", (event) => {
  peers = event.payload;
  render();
});

refreshPeers();
render();
