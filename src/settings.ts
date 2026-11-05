import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "./styles.css";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

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

type Preview = { settings: Settings };

const COLORS = ["#F7A53F", "#F86E51", "#E8638C", "#7C9A6E", "#6B8FB8", "#B47CC7"];
const preview = (window as Window & { __TAP_PREVIEW__?: Preview }).__TAP_PREVIEW__ ?? null;

let settings: Settings | null = null;
let adding = false;
let error = "";

const root = document.querySelector<HTMLDivElement>("#settings")!;

if (preview) document.body.classList.add("preview");

function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function render() {
  if (!settings) {
    root.innerHTML = `<div class="settings"><h1>Settings</h1><p class="error">${esc(error || "Loading settings")}</p></div>`;
    return;
  }
  const color = settings.senderColor.toLowerCase();
  root.innerHTML = `
    <div class="settings">
      <h1>Settings</h1>
      <label class="block"><span>Your name</span>
        <input id="name" type="text" value="${esc(settings.displayName)}" />
      </label>
      <label class="block"><span>Team key</span>
        <div class="key-field">
          <input id="key" class="mono" type="text" value="${esc(settings.teamKey)}" spellcheck="false" />
          <button id="copy" class="copy" type="button">Copy</button>
        </div>
      </label>
      <div>
        <div class="swatch-label">Your color</div>
        <div class="swatches">
          ${COLORS.map(
            (swatch) =>
              `<button class="swatch${swatch.toLowerCase() === color ? " selected" : ""}" type="button" style="--swatch:${swatch}" data-color="${swatch}" aria-label="${swatch}"></button>`,
          ).join("")}
        </div>
      </div>
      <div>
        <div class="swatch-label">Manual peers</div>
        ${settings.manualPeers
          .map(
            (peer, index) =>
              `<div class="peer-line"><span class="mono">${esc(peer)}</span><button type="button" data-remove="${index}">Remove</button></div>`,
          )
          .join("")}
        ${
          adding
            ? `<input id="add-peer" class="add-input mono" type="text" placeholder="Host name or IP" />`
            : `<button id="add" class="add" type="button">+ Add peer</button>`
        }
      </div>
      ${error ? `<p class="error">${esc(error)}</p>` : ""}
      <div class="toggle-row">
        <div><strong>Tailscale only</strong><small>Only find peers on your tailnet</small></div>
        <button id="tailnet" class="switch" type="button" role="switch" aria-checked="${settings.tailnetOnly ? "true" : "false"}">
          <span class="knob"></span>
        </button>
      </div>
    </div>
  `;

  const name = document.getElementById("name") as HTMLInputElement;
  name.addEventListener("change", () => {
    settings!.displayName = name.value.trim();
    void persist();
  });
  const key = document.getElementById("key") as HTMLInputElement;
  key.addEventListener("change", () => {
    settings!.teamKey = key.value;
    void persist();
  });
  document.getElementById("copy")!.onclick = () => {
    void navigator.clipboard.writeText(settings!.teamKey).catch(() => {});
  };
  for (const swatch of root.querySelectorAll<HTMLButtonElement>(".swatch")) {
    swatch.onclick = () => {
      settings!.senderColor = swatch.dataset.color || settings!.senderColor;
      void persist();
      render();
    };
  }
  for (const button of root.querySelectorAll<HTMLButtonElement>("[data-remove]")) {
    button.onclick = () => {
      const index = Number(button.dataset.remove);
      settings!.manualPeers = settings!.manualPeers.filter((_, i) => i !== index);
      void persist();
      render();
    };
  }
  document.getElementById("add")?.addEventListener("click", () => {
    adding = true;
    render();
    document.getElementById("add-peer")?.focus();
  });
  const add = document.getElementById("add-peer") as HTMLInputElement | null;
  add?.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      const value = add.value.trim();
      adding = false;
      if (value) settings!.manualPeers = [...settings!.manualPeers, value];
      void persist();
      render();
    } else if (event.key === "Escape") {
      adding = false;
      render();
    }
  });
  document.getElementById("tailnet")!.onclick = () => {
    settings!.tailnetOnly = !settings!.tailnetOnly;
    void persist();
    render();
  };
}

async function persist() {
  if (!settings || preview) return;
  try {
    await invoke("save_app_settings", { settings });
    error = "";
  } catch (err) {
    error = typeof err === "string" ? err : "Couldn't save settings.";
    render();
  }
}

async function boot() {
  if (preview) {
    settings = preview.settings;
    render();
    return;
  }
  try {
    settings = await invoke<Settings>("get_settings");
  } catch (err) {
    error = typeof err === "string" ? err : "Couldn't load settings.";
  }
  render();
  window.addEventListener("blur", () => {
    void getCurrentWindow().hide().catch(() => {});
  });
  window.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !adding) {
      void getCurrentWindow().hide().catch(() => {});
    }
  });
}

void boot();
