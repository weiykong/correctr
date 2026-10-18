import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

// ── Types ──────────────────────────────────────────────────────────────
interface Skill {
  id: string;
  name: string;
  description: string;
  prompt: string;
  builtin: boolean;
}
interface CustomProvider {
  id: string;
  name: string;
  base_url: string;
  api_key: string;
  model: string;
}
interface Config {
  provider: string;
  openai_model: string;
  gemini_model: string;
  anthropic_model: string;
  temperature: number | null;
  languages: string[];
  preview_before_replace: boolean;
  auto_paste: boolean;
  hotkey: string;
  custom_providers: CustomProvider[];
  skills: Skill[];
  api_keys: Record<string, string>;
  models: Record<string, string[]>;
}

// ── Icons ──────────────────────────────────────────────────────────────
const svg = (d: string) =>
  `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round">${d}</svg>`;

const ICONS: Record<string, string> = {
  provider: svg(
    '<path d="M4.6 11.4a2.6 2.6 0 0 1 .2-5.2 3.3 3.3 0 0 1 6.3.9 2.4 2.4 0 0 1-.6 4.7H4.6z"/>',
  ),
  languages: svg(
    '<circle cx="8" cy="8" r="5.7"/><path d="M2.3 8h11.4"/><path d="M8 2.3c1.6 1.5 2.5 3.5 2.5 5.7S9.6 12.2 8 13.7c-1.6-1.5-2.5-3.5-2.5-5.7S6.4 3.8 8 2.3z"/>',
  ),
  skills: svg('<path d="M8.9 2.2 4.4 8.8h2.9l-.7 5 4.9-6.9H8.4l.5-4.7z"/>'),
  general: svg(
    '<path d="M2.6 5.2h4.2M10.2 5.2h3.2M2.6 10.8h2.6M8.6 10.8h4.8"/><circle cx="8" cy="5.2" r="1.5"/><circle cx="6.6" cy="10.8" r="1.5"/>',
  ),
};

// ── State ──────────────────────────────────────────────────────────────
let cfg: Config;
let activeTab = "provider";

const TABS: { id: string; label: string; hint: string }[] = [
  { id: "provider", label: "Provider", hint: "Endpoint, key and model" },
  { id: "languages", label: "Languages", hint: "Translation targets" },
  { id: "skills", label: "Skills", hint: "One-click clipboard actions" },
  { id: "general", label: "General", hint: "Behaviour and shortcuts" },
];

const QUICK_LANGS = [
  "French", "English", "Chinese", "Spanish", "German", "Italian",
  "Portuguese", "Japanese", "Korean", "Arabic", "Russian", "Dutch",
];

const app = document.getElementById("app")!;

const esc = (s: string) =>
  String(s ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");

// ── Shell ──────────────────────────────────────────────────────────────
function shell(): void {
  const nav = TABS.map(
    (t) => `<button class="tab" data-tab="${t.id}">${ICONS[t.id]}<span>${t.label}</span></button>`,
  ).join("");

  app.innerHTML = `
    <div class="traffic">
      <button class="dot red" id="close" title="Close"></button>
      <button class="dot yellow" id="min" title="Minimise"></button>
      <span class="dot green"></span>
    </div>

    <aside class="sidebar">
      <div class="brand">
        <div class="brand-mark">C</div>
        <div>
          <div class="brand-name">Correctr</div>
          <div class="brand-sub">AI text tools</div>
        </div>
      </div>
      <nav class="nav">${nav}</nav>
      <div class="sidebar-foot">
        <div class="engine" id="engine">
          <span class="engine-dot"></span>
          <div class="engine-text">
            <div class="engine-name">—</div>
            <div class="engine-model">loading…</div>
          </div>
        </div>
      </div>
    </aside>

    <main class="main">
      <header class="topbar">
        <h1 id="page-title">Provider</h1>
      </header>
      <div class="content" id="content"></div>
    </main>

    <div id="toasts"></div>
  `;

  document.getElementById("close")!.onclick = () => getCurrentWindow().hide();
  document.getElementById("min")!.onclick = () => getCurrentWindow().minimize();
  document.querySelectorAll<HTMLButtonElement>(".tab").forEach((btn) => {
    btn.onclick = () => {
      activeTab = btn.dataset.tab!;
      location.hash = activeTab;
      render();
    };
  });

  // Remember the last pane across window closes (hash survives a webview reload).
  const fromHash = location.hash.replace("#", "");
  if (TABS.some((t) => t.id === fromHash)) activeTab = fromHash;
}

function banner(kind: "ok" | "err", msg: string): void {
  const host = document.getElementById("toasts");
  if (!host) return;
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = msg;
  host.appendChild(el);
  window.setTimeout(() => {
    el.style.transition = "opacity .18s";
    el.style.opacity = "0";
    window.setTimeout(() => el.remove(), 200);
  }, 3400);
}

// ── Provider helpers ───────────────────────────────────────────────────
const BUILTIN = [
  { id: "openai", name: "OpenAI", host: "api.openai.com" },
  { id: "gemini", name: "Gemini", host: "googleapis.com" },
  { id: "anthropic", name: "Anthropic", host: "api.anthropic.com" },
];

function builtinModel(id: string): string {
  return id === "openai" ? cfg.openai_model : id === "gemini" ? cfg.gemini_model : cfg.anthropic_model;
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url.replace(/^https?:\/\//, "").replace(/\/.*$/, "");
  }
}

function activeInfo(): { name: string; model: string; keyed: boolean } {
  const b = BUILTIN.find((p) => p.id === cfg.provider);
  if (b) {
    return {
      name: b.name,
      model: builtinModel(b.id) || "provider default",
      keyed: !!cfg.api_keys[b.id],
    };
  }
  const c = cfg.custom_providers.find((p) => p.id === cfg.provider);
  if (c) {
    return { name: c.name, model: c.model || "no model", keyed: !!c.api_key };
  }
  return { name: "No provider", model: "not configured", keyed: false };
}

// ── Field helper ───────────────────────────────────────────────────────
const field = (label: string, input: string, hint?: string): string => `
  <div class="field">
    <label>${label}</label>
    ${input}
    ${hint ? `<span class="hint">${hint}</span>` : ""}
  </div>`;

// ── Tab: Provider ──────────────────────────────────────────────────────
function tabProvider(): string {
  const cards = [
    ...BUILTIN.map((p) => ({ id: p.id, name: p.name, url: p.host })),
    ...cfg.custom_providers.map((p) => ({ id: p.id, name: p.name, url: hostOf(p.base_url) })),
  ]
    .map(
      (p) => `
    <button class="prov ${cfg.provider === p.id ? "is-active" : ""}" data-provider="${p.id}">
      <span class="prov-name">${esc(p.name)}</span>
      <span class="prov-url">${esc(p.url)}</span>
      ${cfg.provider === p.id ? '<span class="prov-check">✓</span>' : ""}
    </button>`,
    )
    .join("");

  const isBuiltin = BUILTIN.some((p) => p.id === cfg.provider);
  const custom = cfg.custom_providers.find((p) => p.id === cfg.provider);

  let detail = "";
  if (isBuiltin) {
    const id = cfg.provider;
    const known = cfg.models[id] ?? [];
    const current = builtinModel(id);
    const options = Array.from(new Set([...known, current].filter((m) => m.trim())));
    const datalist = options.length
      ? `<datalist id="dl-${id}">${options.map((m) => `<option value="${esc(m)}">`).join("")}</datalist>`
      : "";

    detail = `
      ${field(
        "API key",
        `<input type="password" id="key-${id}" placeholder="sk-…" value="${esc(cfg.api_keys[id] ?? "")}" spellcheck="false">`,
        "Stored locally in config.json (chmod 600). Environment variables are used as a fallback.",
      )}
      <div class="field">
        <label>Model</label>
        <div class="field-row">
          <input type="text" id="model-${id}" list="dl-${id}" value="${esc(current)}" placeholder="pick or type a model id" spellcheck="false">
          ${datalist}
          <button class="btn ghost sm" id="refresh-models">Refresh</button>
        </div>
        <span class="hint">${options.length} model${options.length === 1 ? "" : "s"} known for this provider.</span>
      </div>
      <div class="actions">
        <button class="btn primary" id="save-provider">Save changes</button>
      </div>`;
  } else if (custom) {
    detail = `
      ${field("Display name", `<input type="text" id="cp-name" value="${esc(custom.name)}">`)}
      ${field(
        "Base URL",
        `<input type="text" id="cp-url" value="${esc(custom.base_url)}" spellcheck="false">`,
        "Everything before <code>/chat/completions</code>.",
      )}
      <div class="grid-2">
        ${field(
          "API key",
          `<input type="password" id="cp-key" value="${esc(custom.api_key)}" placeholder="optional for local servers" spellcheck="false">`,
        )}
        ${field(
          "Model",
          `<input type="text" id="cp-model" value="${esc(custom.model)}" placeholder="model id" spellcheck="false">`,
        )}
      </div>
      <div class="actions">
        <button class="btn primary" id="save-custom">Save changes</button>
        <span class="spacer"></span>
        <button class="btn danger" id="remove-custom">Remove provider</button>
      </div>`;
  } else {
    detail = `<div class="muted">Select a provider above to configure it.</div>`;
  }

  return `
    <section class="section">
      <div class="section-head">
        <div>
          <h2>Active provider</h2>
          <p class="sub">Where correction requests are sent. Any OpenAI-compatible endpoint works.</p>
        </div>
        <button class="btn ghost sm" id="test-conn">Test connection</button>
      </div>
      <div class="provider-grid">${cards}</div>
      <div class="divider"></div>
      ${detail}
    </section>

    <section class="section">
      <div class="section-head">
        <div>
          <h2>Add a custom provider</h2>
          <p class="sub">OpenAI-compatible gateway, vLLM, LM Studio, Ollama or a proxy.</p>
        </div>
      </div>
      <div class="grid-2">
        ${field("Display name", `<input type="text" id="new-name" placeholder="Together AI">`)}
        ${field("Base URL", `<input type="text" id="new-url" placeholder="https://api.together.xyz/v1" spellcheck="false">`)}
        ${field("API key", `<input type="password" id="new-key" placeholder="optional for local servers" spellcheck="false">`)}
        ${field("Model", `<input type="text" id="new-model" placeholder="meta-llama/Llama-3.3-70B" spellcheck="false">`)}
      </div>
      <div class="actions">
        <button class="btn primary" id="add-custom">Add provider</button>
      </div>
    </section>`;
}

// ── Tab: Languages ─────────────────────────────────────────────────────
function tabLanguages(): string {
  const chips = cfg.languages
    .map(
      (l) => `<span class="chip">${esc(l)}<button data-lang="${esc(l)}" title="Remove">×</button></span>`,
    )
    .join("");

  const suggestions = QUICK_LANGS.filter((l) => !cfg.languages.includes(l))
    .map((l) => `<button data-quick="${esc(l)}">${esc(l)}</button>`)
    .join("");

  return `
    <section class="section">
      <div class="section-head">
        <div>
          <h2>Translation targets</h2>
          <p class="sub">Each language appears under <b>Translate</b> in the menu bar. Type and press ↵ to add your own.</p>
        </div>
      </div>
      <div class="chips" id="chips">
        ${chips}
        <input type="text" class="chip-input" id="new-lang" placeholder="+ language" maxlength="24">
      </div>
      ${suggestions ? `<div class="suggest-label">Suggestions</div><div class="suggest">${suggestions}</div>` : ""}
    </section>`;
}

// ── Tab: Skills ────────────────────────────────────────────────────────
function tabSkills(): string {
  const items = cfg.skills
    .map(
      (s) => `
    <div class="list-item">
      <div class="info">
        <div class="name">${esc(s.name)}${s.builtin ? '<span class="badge">built-in</span>' : ""}</div>
        <div class="desc">${esc(s.description)}</div>
      </div>
      ${s.builtin ? "" : `<button class="btn ghost sm" data-del="${s.id}">Delete</button>`}
    </div>`,
    )
    .join("");

  return `
    <section class="section">
      <div class="section-head">
        <div>
          <h2>Skills</h2>
          <p class="sub">One-click clipboard actions, available from the menu bar.</p>
        </div>
      </div>
      ${items ? `<div class="list">${items}</div>` : `<div class="muted">No skills configured.</div>`}
    </section>

    <section class="section">
      <div class="section-head">
        <div>
          <h2>New skill</h2>
          <p class="sub">Use <code>{text}</code> where the clipboard content should be inserted.</p>
        </div>
      </div>
      <div class="grid-2">
        ${field("Name", `<input type="text" id="sk-name" placeholder="Rewrite formal">`)}
        ${field("Description", `<input type="text" id="sk-desc" placeholder="Make it formal">`)}
      </div>
      ${field(
        "Prompt",
        `<textarea id="sk-prompt" placeholder="Rewrite the following text in a formal tone.&#10;&#10;Text:&#10;{text}"></textarea>`,
      )}
      <div class="actions">
        <button class="btn primary" id="add-skill">Add skill</button>
      </div>
    </section>`;
}

// ── Tab: General ───────────────────────────────────────────────────────
function tabGeneral(): string {
  return `
    <section class="section">
      <div class="section-head">
        <div><h2>Behaviour</h2><p class="sub">How results reach the app you are typing in.</p></div>
      </div>
      <div class="pref">
        <div class="pref-text">
          <div class="pref-title">Preview before replacing</div>
          <div class="pref-desc">Stream the answer into a floating diff panel. ↵ accepts, Esc dismisses.</div>
        </div>
        <label class="switch"><input type="checkbox" id="preview-mode" ${cfg.preview_before_replace ? "checked" : ""}><span class="slider"></span></label>
      </div>
      <div class="pref">
        <div class="pref-text">
          <div class="pref-title">Auto-paste result</div>
          <div class="pref-desc">Paste the corrected text over your selection automatically.</div>
        </div>
        <label class="switch"><input type="checkbox" id="auto-paste" ${cfg.auto_paste ? "checked" : ""}><span class="slider"></span></label>
      </div>
    </section>

    <section class="section">
      <div class="section-head">
        <div><h2>Shortcut</h2><p class="sub">Global hotkey that corrects the current selection.</p></div>
      </div>
      <div class="field-row">
        <div class="field">
          <label>Hotkey</label>
          <input type="text" id="hotkey" value="${esc(cfg.hotkey)}" placeholder="Ctrl+Cmd+C" spellcheck="false">
        </div>
        <button class="btn ghost" id="save-hotkey">Update</button>
      </div>
      <span class="hint" style="display:block;margin-top:9px;font-size:10.5px;color:var(--text-3)">
        Requires Accessibility permission in System Settings → Privacy &amp; Security.
      </span>
    </section>

    <section class="section">
      <div class="section-head">
        <div><h2>Model</h2><p class="sub">Generation parameters shared by every provider.</p></div>
      </div>
      <div class="field" style="max-width:190px">
        <label>Temperature</label>
        <input type="number" id="temp" min="0" max="2" step="0.1" value="${cfg.temperature ?? ""}" placeholder="default">
        <span class="hint">Leave empty for the provider default.</span>
      </div>
    </section>

    <section class="section">
      <div class="section-head">
        <div>
          <h2>Config file</h2>
          <p class="sub"><code>~/Library/Application Support/Correctr/config.json</code> — permissions 600.</p>
        </div>
      </div>
      <div class="actions" style="margin-top:0">
        <button class="btn ghost sm" id="open-config">Reveal in Finder</button>
      </div>
    </section>`;
}

// ── Wiring per tab ─────────────────────────────────────────────────────
function wire(): void {
  const q = <T extends HTMLElement>(s: string) => document.querySelector<T>(s);

  if (activeTab === "provider") {
    document.querySelectorAll<HTMLElement>(".prov").forEach((card) => {
      card.onclick = () => {
        cfg.provider = card.dataset.provider!;
        persist();
      };
    });

    q<HTMLButtonElement>("#save-provider")?.addEventListener("click", () => {
      const id = cfg.provider;
      if (BUILTIN.some((p) => p.id === id)) {
        const key = q<HTMLInputElement>(`#key-${id}`)!.value.trim();
        const model = q<HTMLInputElement>(`#model-${id}`)!.value.trim();
        if (key) cfg.api_keys[id] = key;
        else delete cfg.api_keys[id];
        if (id === "openai") cfg.openai_model = model;
        if (id === "gemini") cfg.gemini_model = model;
        if (id === "anthropic") cfg.anthropic_model = model;
      }
      persist().then(() => banner("ok", "Saved"));
    });

    q<HTMLButtonElement>("#refresh-models")?.addEventListener("click", async () => {
      const btn = q<HTMLButtonElement>("#refresh-models")!;
      btn.disabled = true;
      btn.textContent = "…";
      try {
        const models = await invoke<string[]>("list_models");
        cfg = await invoke<Config>("get_config");
        render();
        banner("ok", `${models.length} models loaded`);
      } catch (e) {
        banner("err", String(e));
        btn.disabled = false;
        btn.textContent = "Refresh";
      }
    });

    q<HTMLButtonElement>("#test-conn")?.addEventListener("click", async (ev) => {
      const btn = ev.currentTarget as HTMLButtonElement;
      const prev = btn.textContent;
      btn.disabled = true;
      btn.textContent = "Testing…";
      try {
        banner("ok", await invoke<string>("test_connection"));
      } catch (e) {
        banner("err", String(e));
      } finally {
        btn.disabled = false;
        btn.textContent = prev;
      }
    });

    q<HTMLButtonElement>("#save-custom")?.addEventListener("click", () => {
      const cp = cfg.custom_providers.find((p) => p.id === cfg.provider)!;
      cp.name = q<HTMLInputElement>("#cp-name")!.value;
      cp.base_url = q<HTMLInputElement>("#cp-url")!.value;
      cp.api_key = q<HTMLInputElement>("#cp-key")!.value;
      cp.model = q<HTMLInputElement>("#cp-model")!.value;
      persist().then(() => banner("ok", "Saved"));
    });

    q<HTMLButtonElement>("#remove-custom")?.addEventListener("click", async () => {
      cfg = await invoke<Config>("remove_custom_provider", { id: cfg.provider });
      render();
      banner("ok", "Provider removed");
    });

    q<HTMLButtonElement>("#add-custom")?.addEventListener("click", async () => {
      const name = q<HTMLInputElement>("#new-name")!.value.trim();
      const url = q<HTMLInputElement>("#new-url")!.value.trim();
      if (!name || !url) {
        banner("err", "Name and Base URL are required");
        return;
      }
      cfg = await invoke<Config>("add_custom_provider", {
        name,
        baseUrl: url,
        apiKey: q<HTMLInputElement>("#new-key")!.value.trim(),
        model: q<HTMLInputElement>("#new-model")!.value.trim(),
      });
      render();
      banner("ok", `Added ${name}`);
    });
  }

  if (activeTab === "languages") {
    const add = (v: string) => {
      const val = v.trim();
      if (val && !cfg.languages.includes(val)) cfg.languages.push(val);
      persist();
    };

    q<HTMLInputElement>("#new-lang")!.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        const input = e.target as HTMLInputElement;
        add(input.value);
        input.value = "";
      }
    });

    document.querySelectorAll<HTMLButtonElement>("[data-lang]").forEach((btn) => {
      btn.onclick = () => {
        cfg.languages = cfg.languages.filter((l) => l !== btn.dataset.lang);
        persist();
      };
    });

    document.querySelectorAll<HTMLButtonElement>("[data-quick]").forEach((btn) => {
      btn.onclick = () => add(btn.dataset.quick!);
    });
  }

  if (activeTab === "skills") {
    q<HTMLButtonElement>("#add-skill")?.addEventListener("click", async () => {
      const name = q<HTMLInputElement>("#sk-name")!.value.trim();
      const prompt = q<HTMLTextAreaElement>("#sk-prompt")!.value.trim();
      if (!name || !prompt) {
        banner("err", "Name and prompt are required");
        return;
      }
      cfg.skills = await invoke<Skill[]>("add_skill", {
        name,
        description: q<HTMLInputElement>("#sk-desc")!.value.trim(),
        prompt,
      });
      render();
      banner("ok", `Skill "${name}" added`);
    });
    document.querySelectorAll<HTMLButtonElement>("[data-del]").forEach((btn) => {
      btn.onclick = async () => {
        cfg.skills = await invoke<Skill[]>("remove_skill", { id: btn.dataset.del });
        render();
      };
    });
  }

  if (activeTab === "general") {
    q<HTMLInputElement>("#preview-mode")!.addEventListener("change", (e) => {
      cfg.preview_before_replace = (e.target as HTMLInputElement).checked;
      persist();
    });
    q<HTMLInputElement>("#auto-paste")!.addEventListener("change", (e) => {
      cfg.auto_paste = (e.target as HTMLInputElement).checked;
      persist();
    });
    const temp = q<HTMLInputElement>("#temp")!;
    temp.addEventListener("change", () => {
      cfg.temperature = temp.value ? parseFloat(temp.value) : null;
      persist();
    });
    q<HTMLButtonElement>("#save-hotkey")?.addEventListener("click", () => {
      invoke("set_hotkey", { hotkey: q<HTMLInputElement>("#hotkey")!.value.trim() })
        .then(() => banner("ok", "Hotkey updated"))
        .catch((e) => banner("err", String(e)));
    });
    q<HTMLButtonElement>("#open-config")?.addEventListener("click", () => {
      invoke("reveal_config").catch(() => {});
    });
  }
}

// ── Render / persist ───────────────────────────────────────────────────
async function persist(): Promise<void> {
  try {
    cfg = await invoke<Config>("save_config", { cfg });
    render();
  } catch (e) {
    banner("err", String(e));
  }
}

function renderSidebar(): void {
  document.querySelectorAll<HTMLButtonElement>(".tab").forEach((t) => {
    t.classList.toggle("active", t.dataset.tab === activeTab);
  });
  const title = document.getElementById("page-title");
  if (title) title.textContent = TABS.find((t) => t.id === activeTab)!.label;

  const info = activeInfo();
  const engine = document.getElementById("engine");
  if (!engine) return;
  engine.querySelector(".engine-name")!.textContent = info.name;
  engine.querySelector(".engine-model")!.textContent = info.model;
  engine.querySelector(".engine-dot")!.classList.toggle("warn", !info.keyed);
  engine.setAttribute("title", info.keyed ? `${info.name} · ${info.model}` : "No API key set");
}

function render(): void {
  const content = document.getElementById("content");
  if (!content) return;
  const bodies: Record<string, () => string> = {
    provider: tabProvider,
    languages: tabLanguages,
    skills: tabSkills,
    general: tabGeneral,
  };
  content.innerHTML = bodies[activeTab]();
  renderSidebar();
  wire();
}

// ── Boot ───────────────────────────────────────────────────────────────
const FALLBACK: Config = {
  provider: "openai",
  openai_model: "gpt-4o-mini",
  gemini_model: "",
  anthropic_model: "",
  temperature: null,
  languages: ["French", "English", "Chinese", "Spanish"],
  preview_before_replace: true,
  auto_paste: false,
  hotkey: "Ctrl+Cmd+C",
  custom_providers: [],
  skills: [],
  api_keys: {},
  models: {},
};

shell();
invoke<Config>("get_config")
  .then((loaded) => {
    cfg = loaded;
    render();
  })
  .catch((e) => {
    // Keep the window usable (instead of blank) if the backend is unreachable.
    console.error("get_config failed:", e);
    cfg = FALLBACK;
    render();
    banner("err", "Could not read the configuration — showing defaults");
  });
