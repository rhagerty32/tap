import { listen } from "@tauri-apps/api/event";

const root = document.querySelector<HTMLDivElement>("#overlay-root")!;

type OverlayPayload = {
  visible: boolean;
  senderName?: string;
  color?: string;
  chimeEnabled?: boolean;
};

let chimeEnabled = false;

function render(payload: OverlayPayload) {
  if (!payload.visible) {
    root.innerHTML = "";
    document.body.style.background = "transparent";
    return;
  }
  const color = payload.color ?? "#3b82f6";
  document.body.style.background = "transparent";
  root.innerHTML = `
    <div class="frame" style="--accent:${color}">
      <div class="banner">${payload.senderName ?? "Someone"} is trying to get your attention</div>
    </div>
  `;
  if (payload.chimeEnabled ?? chimeEnabled) {
    playChime();
  }
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
      ctx.close();
    }, 120);
  } catch {
    // ignore audio failures
  }
}

listen<OverlayPayload>("overlay-changed", (event) => {
  render(event.payload);
});

document.documentElement.style.background = "transparent";
document.body.style.margin = "0";
document.body.style.overflow = "hidden";

const style = document.createElement("style");
style.textContent = `
  html, body, #overlay-root { width: 100%; height: 100%; }
  .frame {
    width: 100%; height: 100%; box-sizing: border-box;
    border: 14px solid var(--accent);
    box-shadow: 0 0 40px 8px color-mix(in srgb, var(--accent) 70%, transparent), inset 0 0 24px color-mix(in srgb, var(--accent) 35%, transparent);
    display: flex; align-items: flex-start; justify-content: center;
    pointer-events: none;
  }
  .banner {
    margin-top: 24px;
    background: rgba(0,0,0,0.65);
    color: white;
    padding: 10px 16px;
    border-radius: 999px;
    font: 600 16px/1.2 system-ui, sans-serif;
    pointer-events: none;
  }
`;
document.head.appendChild(style);
