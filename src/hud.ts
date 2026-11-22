import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "./styles.css";
import { listen } from "@tauri-apps/api/event";

const root = document.querySelector<HTMLDivElement>("#hud")!;
document.body.classList.add("hud-app");

type HudPayload = {
  visible: boolean;
  kind: string;
  level: number;
  muted: boolean;
};

const NAMES: Record<string, string> = {
  volume: "Volume",
  brightness: "Brightness",
  play: "Play",
  next: "Next",
  previous: "Previous",
  mute: "Mute",
};

function render(payload: HudPayload) {
  if (!payload.visible) {
    root.replaceChildren();
    return;
  }
  const name = NAMES[payload.kind] ?? "Tap";
  const leveled = payload.kind === "volume" || payload.kind === "brightness" || payload.kind === "mute";
  const amount = Math.round(Math.min(1, Math.max(0, payload.level)) * 100);
  const label = payload.kind === "mute" && payload.muted ? "Muted" : leveled ? `${amount}%` : name;
  root.innerHTML = `<div class="level">
    <strong>${name}</strong>
    <span>${label}</span>
    ${leveled ? `<i style="--level:${amount}%"></i>` : ""}
  </div>`;
}

void listen<HudPayload>("trackpad-hud", (event) => render(event.payload));
