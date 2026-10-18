# Correctr

AI text corrector & translator for the macOS menu bar — the Rust/Tauri rewrite of the
Python *Smart Translator*, slimmer and with a native settings UI.

## Features

- **Menu-bar app** — tray icon with live status (provider + model), no dock clutter
- **In-place Diff Preview & Streaming** — a floating glass HUD streams the answer with a
  word-level diff (additions/deletions) before it replaces your text
- **Anchored to your selection** — the HUD is placed next to the text you selected
  (Accessibility `AXBoundsForRange`), below it by default, flipped above when there is no
  room, clamped to the monitor it belongs to
- **Transient by design** — `↵` accepts and pastes, `Esc` dismisses, *any other key* or a
  click anywhere else makes it disappear
- **Global hotkey** `Ctrl+Cmd+C` — correct the selected text anywhere; the result is pasted
  back over the selection and kept on the clipboard
- **Providers** — OpenAI, Gemini, Anthropic, plus **any OpenAI-compatible endpoint**
  (Together, Fireworks, vLLM, LM Studio, a company gateway…)
- **Translate** — clipboard → any language, targets managed in settings
- **Skills** — one-click clipboard actions (Token Saver, Debug Helper, Explain Code, Commit
  Message, JSON Cleaner) + your own
- **Undo** — one click restores the previous clipboard
- Keys stay local: `~/Library/Application Support/Correctr/config.json` with `600` permissions

Removed vs. the Python version: Playwright Gemini bridge, LLM-generated skill wizard,
Ollama native protocol (any Ollama endpoint works as a custom OpenAI-compatible provider),
and the old legacy config migration path.

## Requirements

- macOS 12+
- Rust 1.77+, Node 18+

## Develop

```bash
npm install
npm run tauri dev
```

## Build

```bash
npm install
npm run tauri build
```

Output: `src-tauri/target/release/bundle/macos/Correctr.app`

The bundle is signed ad-hoc (`signingIdentity: "-"` in `tauri.conf.json`). That matters:
an app whose bundle seal is invalid makes the *whole* Accessibility pane in System Settings
hang, and cargo's linker-only signature is not a valid bundle signature. Verify after
installing with `codesign -v --deep --strict /Applications/Correctr.app`.

Grant **Accessibility** on first launch (hotkey, ⌘C/⌘V synthesis and the selection anchor all
need it): System Settings → Privacy & Security → Accessibility. Because the signature is
ad-hoc, every rebuild changes the cdhash and macOS drops the grant — re-tick it after
installing a new build, or use a stable self-signed identity if you iterate a lot.

## Hotkey & flow

1. Select text anywhere, press `Ctrl+Cmd+C`
2. Correctr waits for the modifiers to be released, synthesizes ⌘C, reads the selection
3. The preview HUD appears next to the selection and streams the correction with a diff
4. `↵` replaces the selection (clipboard + synthesized ⌘V into the app you came from);
   `Esc`, any other key, or a click elsewhere dismisses without touching your text

Without the preview (`preview_before_replace: false`) the result goes straight to the
clipboard, and is pasted back when `auto_paste` is on.

## Adding a custom provider

Settings → Add custom provider: name, base URL (the part before `/chat/completions`),
API key, model. It appears in the tray status line immediately.

## Config

`~/Library/Application Support/Correctr/config.json` (0600):

```json
{
  "provider": "openai",
  "openai_model": "gpt-4o-mini",
  "temperature": null,
  "languages": ["French", "English", "Chinese", "Spanish"],
  "preview_before_replace": true,
  "auto_paste": true,
  "hotkey": "Ctrl+Cmd+C",
  "custom_providers": [],
  "skills": [],
  "api_keys": {}
}
```

API keys set in the UI take priority over env vars (`OPENAI_API_KEY`, `GEMINI_API_KEY`,
`ANTHROPIC_API_KEY`).

## Architecture

```
src-tauri/src/
  lib.rs        — app shell, tray menu, global hotkey, paste-back, preview window, commands
  ax.rs         — Accessibility: where is the selection on screen (AXBoundsForRange)
  config.rs     — config schema, persistence (0600), skills
  providers.rs  — OpenAI / Anthropic / Gemini / Ollama wire clients (SSE streaming) + models
  engine.rs     — provider resolution, prompt templates
src/
  main.ts       — settings UI (vanilla TS + Vite), four panes
  preview.ts    — preview HUD: word diff, adaptive sizing, dismiss rules
  style.css     — settings + HUD styling
```

macOS notes worth knowing before changing the paste/HUD code:

- ⌘C/⌘V are posted as real key events from a **private** `CGEventSource` with the Command
  flag set explicitly; enigo's synthetic modifier press does not survive the trip.
- The HUD takes focus, so the frontmost app's bundle id is captured *before* it is shown
  (`lsappinfo`) and re-activated before pasting.
- The preview window is frameless, transparent, **shadowless** (a non-opaque NSWindow's
  shadow is drawn from a rectangular backing store and reads as an outline), and its blur is
  clipped with `clip-path: inset(0 round 12px)` because WKWebView does not round a
  backdrop-filter layer that also hosts children.
