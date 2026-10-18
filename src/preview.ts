import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

interface PreviewData {
  original: string;
  initial?: string;
  streaming?: boolean;
}

interface DiffWord {
  type: "same" | "add" | "del";
  text: string;
}

let originalText = "";
let currentResult = "";
let isStreaming = false;

const app = document.getElementById("preview-app")!;
const currentWin = getCurrentWindow();

// ── Dismiss rules ──────────────────────────────────────────────────────
// The panel is a transient HUD: it must never linger over the user's text.
// It goes away on Esc, on any key other than ↵/Esc, and as soon as the window
// loses focus (a click anywhere else).
let closing = false;
// Focus is changing while the window is being shown/focused — ignore blurs
// until the panel has actually settled on screen.
let armed = false;
let armTimer: number | undefined;

function arm(): void {
  closing = false;
  armed = false;
  window.clearTimeout(armTimer);
  armTimer = window.setTimeout(() => {
    armed = true;
  }, 250);
}

function dismiss(): void {
  if (closing) return;
  closing = true;
  armed = false;
  invoke("dismiss_preview").catch(console.error);
}

function accept(): void {
  if (closing) return;
  closing = true;
  armed = false;
  invoke("accept_preview", { text: currentResult }).catch(console.error);
}

currentWin.onFocusChanged(({ payload: focused }) => {
  if (!focused && armed) dismiss();
});

// Auto-adapt window size according to content length
let resizeTimeout: number | undefined;
function adjustWindowSize(): void {
  window.clearTimeout(resizeTimeout);
  resizeTimeout = window.setTimeout(async () => {
    try {
      const container = document.querySelector<HTMLElement>(".popover-container");
      if (!container) return;

      // Estimate width based on max line length
      const fullText = currentResult || originalText || "Generating…";
      const lines = fullText.split("\n");
      const maxLineLen = Math.max(...lines.map((l) => l.length), 10);

      // Width: between 240 and 420 px
      const targetWidth = Math.min(Math.max(240, maxLineLen * 7.5 + 40), 420);

      // Height: natural content scroll height + header + footer margins
      const bodyEl = document.querySelector<HTMLElement>(".popover-body");
      const bodyHeight = bodyEl ? Math.min(bodyEl.scrollHeight, 260) : 60;
      const targetHeight = Math.min(Math.max(80, bodyHeight + 55), 320);

      await currentWin.setSize(new LogicalSize(Math.round(targetWidth), Math.round(targetHeight)));
    } catch (_) {}
  }, 40);
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// Word-level diff calculation
function computeDiff(orig: string, curr: string): DiffWord[] {
  const origWords = orig.split(/(\s+)/);
  const currWords = curr.split(/(\s+)/);

  const n = origWords.length;
  const m = currWords.length;

  // Short-circuit for empty / very long strings
  if (n === 0 && m === 0) return [];
  if (n * m > 40000) {
    return [
      { type: "del", text: orig },
      { type: "add", text: curr },
    ];
  }

  // Dynamic programming LCS table
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < m; j++) {
      if (origWords[i] === currWords[j]) {
        dp[i + 1][j + 1] = dp[i][j] + 1;
      } else {
        dp[i + 1][j + 1] = Math.max(dp[i + 1][j], dp[i][j + 1]);
      }
    }
  }

  let i = n;
  let j = m;
  const diffRev: DiffWord[] = [];

  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && origWords[i - 1] === currWords[j - 1]) {
      diffRev.push({ type: "same", text: origWords[i - 1] });
      i--;
      j--;
    } else if (j > 0 && (i === 0 || dp[i][j - 1] >= dp[i - 1][j])) {
      diffRev.push({ type: "add", text: currWords[j - 1] });
      j--;
    } else if (i > 0 && (j === 0 || dp[i][j - 1] < dp[i - 1][j])) {
      diffRev.push({ type: "del", text: origWords[i - 1] });
      i--;
    }
  }

  return diffRev.reverse();
}

function renderUI(): void {
  const diff = computeDiff(originalText, currentResult);
  const diffHtml = diff
    .map((d) => {
      if (d.type === "same") return escapeHtml(d.text);
      if (d.type === "add") return `<span class="diff-add">${escapeHtml(d.text)}</span>`;
      if (d.type === "del") return `<span class="diff-del">${escapeHtml(d.text)}</span>`;
      return "";
    })
    .join("");

  app.innerHTML = `
    <div class="popover-container">
      <div class="popover-header">
        <div class="popover-title">
          <span class="${isStreaming ? "streaming-dot" : "ready-dot"}"></span>
          <span>${isStreaming ? "Enhancing…" : "Correctr"}</span>
        </div>
        <button class="btn-close-mini" id="btn-cancel" title="Dismiss (Esc)">✕</button>
      </div>

      <div class="popover-body">
        <div id="view-diff" class="diff-box">${diffHtml || '<span class="muted" style="opacity:0.6;">Generating…</span>'}</div>
      </div>

      <div class="popover-footer">
        <div class="footer-hint">
          <kbd>↵</kbd> Accept &nbsp;•&nbsp; <kbd>Esc</kbd>
        </div>
        <div class="footer-buttons">
          <button class="btn-mini-ghost" id="footer-cancel">Dismiss</button>
          <button class="btn-mini-primary" id="footer-accept" ${isStreaming && !currentResult ? "disabled" : ""}>Accept</button>
        </div>
      </div>
    </div>
  `;

  const onAccept = () => accept();

  const onDismiss = () => dismiss();

  document.getElementById("footer-accept")?.addEventListener("click", onAccept);
  document.getElementById("footer-cancel")?.addEventListener("click", onDismiss);
  document.getElementById("btn-cancel")?.addEventListener("click", onDismiss);

  adjustWindowSize();
}

// Any key other than ↵ accepts/navigates and Esc cancels: for everything else
// the panel simply gets out of the way. Bare modifier presses are exempt so
// starting a ⌘-shortcut does not wipe the preview.
const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta", "CapsLock", "Fn", "OS"]);

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    e.preventDefault();
    dismiss();
    return;
  }
  if (e.key === "Enter") {
    e.preventDefault();
    accept();
    return;
  }
  if (MODIFIER_KEYS.has(e.key)) return;
  e.preventDefault();
  dismiss();
});

// Setup listeners for streaming & events
async function initListeners() {
  await listen<PreviewData>("preview-start", (event) => {
    originalText = event.payload.original || "";
    currentResult = event.payload.initial || "";
    isStreaming = event.payload.streaming ?? true;
    arm();
    renderUI();
  });

  await listen<string>("preview-chunk", (event) => {
    currentResult += event.payload;
    const viewDiff = document.getElementById("view-diff");
    if (viewDiff) {
      const diff = computeDiff(originalText, currentResult);
      viewDiff.innerHTML = diff
        .map((d) => {
          if (d.type === "same") return escapeHtml(d.text);
          if (d.type === "add") return `<span class="diff-add">${escapeHtml(d.text)}</span>`;
          if (d.type === "del") return `<span class="diff-del">${escapeHtml(d.text)}</span>`;
          return "";
        })
        .join("");
      adjustWindowSize();
    }
  });

  await listen<string>("preview-done", (event) => {
    isStreaming = false;
    currentResult = event.payload;
    renderUI();
  });

  await listen<string>("preview-error", (event) => {
    isStreaming = false;
    const viewDiff = document.getElementById("view-diff");
    if (viewDiff) {
      viewDiff.innerHTML = `<span style="color:var(--red);">Error: ${escapeHtml(event.payload)}</span>`;
    }
  });
}

initListeners();
renderUI();
