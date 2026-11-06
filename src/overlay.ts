import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "./styles.css";
import { bootTheme } from "./theme";
import { listen } from "@tauri-apps/api/event";

const root = document.querySelector<HTMLDivElement>("#overlay-root")!;
document.body.classList.add("overlay-app");

type OverlayPayload = {
  visible: boolean;
  senderName?: string;
  color?: string;
  chimeEnabled?: boolean;
  glowStyle?: string;
  showBanner?: boolean;
  avatar?: string | null;
};

type Preview = { senderName: string; color?: string };

const preview = (window as Window & { __TAP_PREVIEW__?: Preview }).__TAP_PREVIEW__;
if (preview) document.body.classList.add("preview");
else bootTheme();

let chimeEnabled = false;
let shownKey = "";
let hideTimer = 0;

function reducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

function esc(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

const GLOW_STYLES = new Set(["edge", "soft", "bold"]);

function glowStyle(value: string | undefined): string {
  return value && GLOW_STYLES.has(value) ? value : "edge";
}

function render(payload: OverlayPayload) {
  if (!payload.visible) {
    if (!shownKey) return;
    shownKey = "";
    root.classList.add("leaving");
    window.clearTimeout(hideTimer);
    hideTimer = window.setTimeout(() => {
      root.innerHTML = "";
      root.classList.remove("leaving");
    }, reducedMotion() ? 150 : 500);
    return;
  }

  const name = payload.senderName ?? "Someone";
  const color = payload.color || "#F86E51";
  const glow = glowStyle(payload.glowStyle);
  const banner = payload.showBanner !== false;
  const key = `${name}|${color}|${glow}|${banner}|${payload.avatar || ""}`;
  if (key === shownKey) return;
  const becameVisible = shownKey === "";
  shownKey = key;
  window.clearTimeout(hideTimer);
  root.classList.remove("leaving");
  const face = payload.avatar
    ? `<img src="${esc(payload.avatar)}" alt="" />`
    : esc(initials(name));
  root.innerHTML = `
    <div class="glow show ${glow}" style="--glow:${esc(color)}"></div>
    ${
      banner
        ? `<div class="banner show">
      <span class="avatar" style="background:${esc(color)};--glow:${esc(color)}">${face}</span>
      <div><b>${esc(name)}</b> is tapping you</div>
    </div>`
        : ""
    }
  `;
  if (becameVisible && (payload.chimeEnabled ?? chimeEnabled)) playChime();
}

function playChime() {
  try {
    const ctx = new AudioContext();
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = "sine";
    osc.frequency.value = 880;
    gain.gain.value = 0.04;
    osc.connect(gain);
    gain.connect(ctx.destination);
    osc.start();
    setTimeout(() => {
      osc.stop();
      void ctx.close();
    }, 120);
  } catch {
    // ignore audio failures
  }
}

if (preview) {
  render({ visible: true, senderName: preview.senderName, color: preview.color, chimeEnabled: false });
} else {
  void listen<OverlayPayload>("overlay-changed", (event) => {
    if (typeof event.payload.chimeEnabled === "boolean") chimeEnabled = event.payload.chimeEnabled;
    render(event.payload);
  });
}
