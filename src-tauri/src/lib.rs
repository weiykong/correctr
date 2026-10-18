// Correctr — AI text corrector & translator for the macOS menu bar.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod engine;
mod providers;

#[cfg(target_os = "macos")]
mod ax;

use config::Config;
use std::sync::Mutex;
use tauri::tray::TrayIconBuilder;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

pub struct AppState {
    pub config: Mutex<Config>,
    pub undo_stack: Mutex<Vec<String>>,
    /// Bundle id of the app that was frontmost when the request started — the
    /// place the result has to be pasted back into.
    pub target_app: Mutex<Option<String>>,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            config: Mutex::new(config::load()),
            undo_stack: Mutex::new(Vec::new()),
            target_app: Mutex::new(None),
        })
        .setup(|app| {
            // Menu-bar only: no Dock icon — the tray is the app's whole presence.
            // The settings window still opens on demand from the tray menu.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();

            // ── Tray menu ──────────────────────────────────────────────
            build_tray(&handle)?;
            let tray = app.tray_by_id("main").expect("tray");

            tray.on_menu_event(|app, event| {
                let id = event.id().as_ref().to_string();
                handle_menu_event(app, &id);
            });

            // ── Global hotkey ──────────────────────────────────────────
            // NOTE: on macOS the event tap needs Accessibility permission;
            // without it registration can succeed but never fire. We check
            // that up front and guide the user from the tray.
            let h = app.handle().clone();
            // Register from config, not a hardcoded default — a combo saved in
            // Settings must survive a restart.
            let initial_hotkey = normalize_hotkey(&app.state::<AppState>().config.lock().unwrap().hotkey);
            let hotkey_res = app.global_shortcut().on_shortcut(
                if initial_hotkey.is_empty() { "ctrl+cmd+c" } else { initial_hotkey.as_str() },
                move |_app, _sc, event| {
                    if event.state() == ShortcutState::Pressed {
                        hotkey_correct(h.clone());
                    }
                },
            );
            if let Err(e) = hotkey_res {
                log::error!("hotkey registration failed: {e}");
                set_status(&handle, "✗ hotkey");
            }

            // ── Permission guidance ─────────────────────────────────────
            if !accessibility_granted() {
                set_status(&handle, "⚠ Accessibility");
                // One prompt per launch. It both registers the app in the
                // Accessibility list (a bare AXIsProcessTrusted() check does
                // not add it, so the user would find no switch to flip) and
                // replaces a custom notification with the system one.
                #[cfg(target_os = "macos")]
                {
                    let trusted = prompt_accessibility();
                    log::info!("accessibility prompt raised (trusted now: {trusted})");
                }
                // Watch for the switch being flipped — sparse checks only.
                watch_accessibility(handle.clone());
            }

            // Create (hidden) settings and preview windows eagerly so first open is instant.
            create_settings_window(app.handle(), false)?;
            create_preview_window(app.handle())?;

            // Dev aid: `CORRECTR_SHOW_SETTINGS=1` opens the settings window on launch.
            if std::env::var("CORRECTR_SHOW_SETTINGS").is_ok() {
                show_settings(app.handle());
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            get_skills,
            add_skill,
            remove_skill,
            get_providers,
            list_models,
            add_custom_provider,
            remove_custom_provider,
            test_connection,
            run_action,
            correct_clipboard,
            translate_clipboard,
            undo_clipboard,
            get_hotkey,
            set_hotkey,
            reveal_config,
            accept_preview,
            dismiss_preview,
            get_undo_available
        ])
        .run(tauri::generate_context!())
        .expect("error while running Correctr");
}

// ── Tray ─────────────────────────────────────────────────────────────

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let menu = build_tray_menu(app)?;
    let _ = TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("Correctr")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app)?;
    Ok(())
}

fn build_tray_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let state = app.state::<AppState>();
    let cfg = state.config.lock().unwrap().clone();
    let resolved = engine::resolve(&cfg);

    let status = MenuItem::with_id(
        app,
        "status",
        format!("● {} ({})", resolved.name, short_model(&resolved.model)),
        false,
        None::<&str>,
    )?;

    let correct = MenuItem::with_id(
        app,
        "correct",
        "Correct Selection / Clipboard",
        true,
        Some("Ctrl+Cmd+C"),
    )?;

    // Translate submenu
    let translate = Submenu::with_id(app, "translate", "Translate", true)?;
    for lang in &cfg.languages {
        let item = MenuItem::with_id(
            app,
            format!("translate:{lang}"),
            format!("To {lang}"),
            true,
            None::<&str>,
        )?;
        translate.append(&item)?;
    }
    translate.append(&PredefinedMenuItem::separator(app)?)?;
    translate.append(&MenuItem::with_id(
        app,
        "settings:languages",
        "Configure Languages…",
        true,
        None::<&str>,
    )?)?;

    // Skills submenu
    let skills = Submenu::with_id(app, "skills", "Transform & Skills", true)?;
    for skill in &cfg.skills {
        skills.append(&MenuItem::with_id(
            app,
            format!("skill:{}", skill.id),
            &skill.name,
            true,
            None::<&str>,
        )?)?;
    }
    if cfg.skills.is_empty() {
        skills.append(&MenuItem::with_id(app, "no-skills", "No Skills Configured", false, None::<&str>)?)?;
    }

    let undo = MenuItem::with_id(app, "undo", "Undo Last Replacement", true, Some("Cmd+Z"))?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &correct,
            &translate,
            &skills,
            &undo,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "settings", "Settings…", true, Some("Cmd+,"))?,
            &MenuItem::with_id(app, "ax-settings", "Accessibility Permissions…", true, None::<&str>)?,
            &MenuItem::with_id(app, "ax-reset", "Reset Accessibility Grant…", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit Correctr", true, Some("Cmd+Q"))?,
        ],
    )?;
    Ok(menu)
}

fn short_model(model: &str) -> String {
    if model.chars().count() <= 24 {
        model.to_string()
    } else {
        format!("{}…", model.chars().take(23).collect::<String>())
    }
}

fn refresh_tray(app: &tauri::AppHandle) {
    if let Ok(menu) = build_tray_menu(app) {
        if let Some(tray) = app.tray_by_id("main") {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

fn handle_menu_event(app: &tauri::AppHandle, id: &str) {
    match id {
        "correct" => correct_clipboard(app.clone()),
        "undo" => undo_clipboard(app.clone()),
        "settings" => show_settings(app),
        "provider" => show_settings(app),
        "quit" => app.exit(0),
        "ax-settings" => {
            let _ = app.opener().open_url(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                None::<&str>,
            );
            // Poll gently while the user is expected to flip the switch, so the
            // tray label clears on its own — spaced far enough apart to leave
            // tccd free for the pane that is opening.
            watch_accessibility(app.clone());
        }
        "ax-reset" => {
            // Clears a stale TCC row (the usual reason the Accessibility pane
            // hangs after repeated rebuilds: the stored code requirement no
            // longer matches the freshly signed bundle).
            let ok = std::process::Command::new("/usr/bin/tccutil")
                .args(["reset", "Accessibility", "app.correctr"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            notify(
                app,
                "Correctr",
                if ok {
                    "Accessibility grant cleared — relaunch, then tick Correctr again"
                } else {
                    "Could not reset the Accessibility grant"
                },
            );
        }
        "settings:languages" => show_settings(app),
        _ if id.starts_with("translate:") => {
            let lang = id.strip_prefix("translate:").unwrap_or("English");
            translate_clipboard(app.clone(), lang.to_string());
        }
        _ if id.starts_with("skill:") => {
            let skill_id = id.strip_prefix("skill:").unwrap_or("");
            run_skill(app.clone(), skill_id.to_string());
        }
        _ => {}
    }
}

// ── Window management ────────────────────────────────────────────────

fn create_settings_window(app: &tauri::AppHandle, visible: bool) -> tauri::Result<()> {
    if app.get_webview_window("settings").is_some() {
        return Ok(());
    }
    let builder = tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("Correctr")
    .inner_size(680.0, 620.0)
    .min_inner_size(600.0, 520.0)
    .visible(visible)
    .decorations(false)
    .transparent(true)
    .shadow(true);
    builder.build()?;
    Ok(())
}

fn create_preview_window(app: &tauri::AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    if let Some(win) = app.get_webview_window("preview") {
        return Ok(win);
    }
    let win = tauri::WebviewWindowBuilder::new(
        app,
        "preview",
        tauri::WebviewUrl::App("preview.html".into()),
    )
    .title("Correctr Preview")
    .inner_size(300.0, 110.0)
    .min_inner_size(220.0, 70.0)
    .visible(false)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    // A borderless, non-opaque NSWindow with hasShadow=true draws its shadow
    // from the window's backing store, which for a blurred webview is a full
    // rectangle — the user sees it as a hard outline around the glass panel.
    .shadow(false)
    .build()?;

    Ok(win)
}

fn show_preview_window(app: &tauri::AppHandle) {
    if let Ok(win) = create_preview_window(app) {
        let _ = win.set_size(tauri::Size::Logical(tauri::LogicalSize {
            width: 280.0,
            height: 90.0,
        }));
        place_preview(&win, app);
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Put the preview next to the text the user selected instead of the middle of
/// the screen. Falls back to centring when the Accessibility query gives us
/// nothing (some apps do not expose selection bounds).
fn place_preview(win: &tauri::WebviewWindow, app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let Some(anchor) = ax::selection_rect() else {
            let _ = win.center();
            return;
        };

        let scale = win.scale_factor().unwrap_or(1.0);
        let (ww, wh) = win
            .outer_size()
            .map(|s| (s.width as f64 / scale, s.height as f64 / scale))
            .unwrap_or((280.0, 90.0));

        // Monitors come back as physical pixels; convert to the same points the
        // Accessibility API speaks.
        let monitors = app.available_monitors().unwrap_or_default();
        let mut area: Option<(f64, f64, f64, f64)> = None; // x0, y0, w, h in points
        for m in &monitors {
            let ms = m.scale_factor();
            let x0 = m.position().x as f64 / ms;
            let y0 = m.position().y as f64 / ms;
            let w = m.size().width as f64 / ms;
            let h = m.size().height as f64 / ms;
            if anchor.0 >= x0 && anchor.0 < x0 + w && anchor.1 >= y0 && anchor.1 < y0 + h {
                area = Some((x0, y0, w, h));
                break;
            }
        }
        let (x0, y0, w, h) = match area.or_else(|| {
            monitors.first().map(|m| {
                let ms = m.scale_factor();
                (
                    m.position().x as f64 / ms,
                    m.position().y as f64 / ms,
                    m.size().width as f64 / ms,
                    m.size().height as f64 / ms,
                )
            })
        }) {
            Some(a) => a,
            None => {
                let _ = win.center();
                return;
            }
        };

        const GAP: f64 = 10.0;
        const EDGE: f64 = 8.0;
        let menu_bar = 34.0;

        // Prefer below the selection: the window grows downward as text streams
        // in, and growing away from the text keeps the diff readable.
        let below = anchor.1 + anchor.3 + GAP;
        let above = anchor.1 - wh - GAP;
        let y = if below + wh <= y0 + h - EDGE {
            below
        } else if above >= y0 + menu_bar {
            above
        } else {
            (y0 + h - wh - EDGE).max(y0 + menu_bar)
        };

        let max_x = x0 + w - ww - EDGE;
        let x = anchor.0.clamp(x0 + EDGE, max_x.max(x0 + EDGE));

        log::info!(
            "preview anchor=({:.0},{:.0},{:.0},{:.0}) size=({:.0}x{:.0}) monitor=({:.0},{:.0},{:.0},{:.0}) -> pos=({:.0},{:.0})",
            anchor.0, anchor.1, anchor.2, anchor.3, ww, wh, x0, y0, w, h, x, y
        );

        let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (win, app);
        let _ = win.center();
    }
}

fn hide_preview_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("preview") {
        let _ = win.hide();
    }
}

fn show_settings(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

// ── Clipboard actions ────────────────────────────────────────────────

fn read_clipboard(app: &tauri::AppHandle) -> String {
    #[cfg(target_os = "macos")]
    if let Some(text) = pasteboard::get_text() {
        return text;
    }
    app.clipboard().read_text().unwrap_or_default()
}

/// Write to the clipboard and verify it landed. The write result used to be
/// discarded with `let _ =`, so a write that silently did nothing was still
/// reported to the user as "Result is on your clipboard ✓".
fn write_clipboard(app: &tauri::AppHandle, text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let before = pasteboard::change_count();

    let plugin_err: Option<String> = match app.clipboard().write_text(text) {
        Ok(()) => None,
        Err(e) => Some(e.to_string()),
    };
    std::thread::sleep(std::time::Duration::from_millis(60));

    if read_clipboard(app) == text {
        log::info!("clipboard: wrote {} chars (plugin)", text.chars().count());
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let direct = pasteboard::set_text(text);
        std::thread::sleep(std::time::Duration::from_millis(60));
        if read_clipboard(app) == text {
            log::info!(
                "clipboard: wrote {} chars (direct NSPasteboard, plugin err={plugin_err:?})",
                text.chars().count()
            );
            return Ok(());
        }
        let after = pasteboard::change_count();
        log::error!(
            "clipboard: both writes failed (plugin err={plugin_err:?}, setString ok={direct}, \
             changeCount {before} -> {after}, readback chars={:?})",
            pasteboard::get_text().map(|t| t.chars().count())
        );
        Err(match plugin_err {
            Some(e) => format!("Could not write the clipboard: {e}"),
            None => "Could not write the clipboard (NSPasteboard refused)".to_string(),
        })
    }

    #[cfg(not(target_os = "macos"))]
    Err(plugin_err.unwrap_or_else(|| "clipboard write unverified".to_string()))
}

const UNDO_LIMIT: usize = 20;

fn push_undo(stack: &Mutex<Vec<String>>, text: &str) {
    let mut stack = stack.lock().unwrap();
    if stack.last().map(String::as_str) != Some(text) {
        stack.push(text.to_string());
        if stack.len() > UNDO_LIMIT {
            stack.remove(0);
        }
    }
}

// ── macOS pasteboard (NSPasteboard via the Objective-C runtime) ────────
// The change counter tells "the user re-copied the same text" apart from
// "nothing was copied", which a text comparison cannot. It also gives a
// direct write path, so a clipboard write that silently does nothing can be
// detected instead of being reported to the user as success.
#[cfg(target_os = "macos")]
mod pasteboard {
    use std::ffi::{CStr, CString};
    use std::mem::transmute;
    use std::os::raw::{c_char, c_void};

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        /// Declared with NO signature on purpose — it is only ever used as an
        /// address, cast to the arity actually being called. A variadic
        /// declaration passes the extra arguments on the stack on Apple
        /// arm64 while objc_msgSend reads them from registers, which raises
        /// an Objective-C exception that unwinds into Rust and aborts.
        fn objc_msgSend();
    }

    fn cls(name: &str) -> *mut c_void {
        unsafe { objc_getClass(CString::new(name).unwrap().as_ptr()) }
    }

    fn sel(name: &str) -> *mut c_void {
        unsafe { sel_registerName(CString::new(name).unwrap().as_ptr()) }
    }

    #[inline]
    unsafe fn send0(recv: *mut c_void, sel: *mut c_void) -> isize {
        let f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize =
            transmute(objc_msgSend as *const ());
        f(recv, sel)
    }

    #[inline]
    unsafe fn send1(recv: *mut c_void, sel: *mut c_void, a: *const c_void) -> isize {
        let f: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void) -> isize =
            transmute(objc_msgSend as *const ());
        f(recv, sel, a)
    }

    #[inline]
    unsafe fn send2(recv: *mut c_void, sel: *mut c_void, a: *const c_void, b: *const c_void) -> isize {
        let f: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *const c_void,
            *const c_void,
        ) -> isize = transmute(objc_msgSend as *const ());
        f(recv, sel, a, b)
    }

    #[inline]
    unsafe fn send3(
        recv: *mut c_void,
        sel: *mut c_void,
        a: *const c_void,
        b: usize,
        c: usize,
    ) -> isize {
        let f: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, usize, usize) -> isize =
            transmute(objc_msgSend as *const ());
        f(recv, sel, a, b, c)
    }

    /// NSUTF8StringEncoding
    const NS_UTF8: usize = 4;

    fn nsstring(text: &str) -> *mut c_void {
        unsafe {
            let s = send0(cls("NSString"), sel("alloc")) as *mut c_void;
            send3(
                s,
                sel("initWithBytes:length:encoding:"),
                text.as_ptr() as *const c_void,
                text.len(),
                NS_UTF8,
            ) as *mut c_void
        }
    }

    /// The `public.utf8-plain-text` pasteboard type, as an NSString.
    fn plain_text_type() -> *mut c_void {
        let name = CString::new("public.utf8-plain-text").unwrap();
        unsafe {
            send1(
                cls("NSString"),
                sel("stringWithUTF8String:"),
                name.as_ptr() as *const c_void,
            ) as *mut c_void
        }
    }

    fn general_pasteboard() -> *mut c_void {
        unsafe { send0(cls("NSPasteboard"), sel("generalPasteboard")) as *mut c_void }
    }

    /// `NSPasteboard.changeCount` — ticks on every pasteboard mutation.
    pub fn change_count() -> i64 {
        unsafe { send0(general_pasteboard(), sel("changeCount")) as i64 }
    }

    /// The pasteboard's plain-text contents, if any.
    pub fn get_text() -> Option<String> {
        unsafe {
            let pb = general_pasteboard();
            if pb.is_null() {
                return None;
            }
            let s = send1(pb, sel("stringForType:"), plain_text_type() as *const c_void)
                as *mut c_void;
            if s.is_null() {
                return None;
            }
            let utf8 = send0(s, sel("UTF8String")) as *const c_char;
            if utf8.is_null() {
                return None;
            }
            Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
        }
    }

    /// Write text the way Cocoa does it: clearContents + setString:forType:.
    /// Returns false when NSPasteboard refused the write.
    pub fn set_text(text: &str) -> bool {
        unsafe {
            let pb = general_pasteboard();
            if pb.is_null() {
                return false;
            }
            send0(pb, sel("clearContents"));
            let ok = send2(
                pb,
                sel("setString:forType:"),
                nsstring(text) as *const c_void,
                plain_text_type() as *const c_void,
            );
            // setString:forType: returns BOOL — only the low 32 bits count.
            (ok as i32) != 0
        }
    }
}

// ── macOS modifier state + synthetic ⌘-keypresses ─────────────────────
// A global hotkey fires *while* the user is still holding its modifiers.
// Synthesizing ⌘C at that instant is delivered as ⌃⌘C, which no app treats
// as Copy: the pasteboard never ticks and the action reports "nothing
// selected". So wait for the real keys to come up, then post the copy from a
// private event source whose flags we set explicitly.
#[cfg(target_os = "macos")]
mod keys {
    use std::os::raw::c_void;
    use std::time::Duration;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceFlagsState(state: u32) -> u64;
        fn CGEventSourceCreate(state: i32) -> *mut c_void;
        fn CGEventCreateKeyboardEvent(source: *mut c_void, key: u16, keydown: bool) -> *mut c_void;
        fn CGEventSetFlags(event: *mut c_void, flags: u64);
        fn CGEventPost(tap: u32, event: *mut c_void);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *mut c_void);
    }

    pub const VK_ANSI_C: u16 = 0x08;
    pub const VK_ANSI_V: u16 = 0x09;

    const VK_COMMAND: u16 = 0x37;
    const FLAG_SHIFT: u64 = 1 << 17; // kCGEventFlagMaskShift
    const FLAG_CONTROL: u64 = 1 << 18; // kCGEventFlagMaskControl
    const FLAG_OPTION: u64 = 1 << 19; // kCGEventFlagMaskAlternate
    const FLAG_COMMAND: u64 = 1 << 20; // kCGEventFlagMaskCommand
    /// Caps Lock (1<<16) and Fn (1<<23) are deliberately excluded — holding
    /// either of those must not stop us from sending the copy.
    const MODIFIER_MASK: u64 = FLAG_SHIFT | FLAG_CONTROL | FLAG_OPTION | FLAG_COMMAND;
    const HID_EVENT_TAP: u32 = 0; // kCGHIDEventTap
    const STATE_COMBINED_SESSION: u32 = 0; // kCGEventSourceStateCombinedSessionState
    const STATE_PRIVATE: i32 = -1; // kCGEventSourceStatePrivate

    /// True while any of ⇧⌃⌥⌘ is physically held (either side).
    pub fn modifier_down() -> bool {
        unsafe { (CGEventSourceFlagsState(STATE_COMBINED_SESSION) & MODIFIER_MASK) != 0 }
    }

    /// Block until every modifier key is physically up. Returns false if the
    /// user is still holding one after `max_ms`.
    pub fn wait_for_modifier_release(max_ms: u64) -> bool {
        let mut waited = 0;
        while waited < max_ms {
            if !modifier_down() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
            waited += 20;
        }
        false
    }

    /// Post ⌘<key> as real key events from a private event source, with the
    /// command flag set explicitly, so no held physical modifier leaks in.
    pub fn send_command_key(keycode: u16) {
        unsafe {
            let src = CGEventSourceCreate(STATE_PRIVATE);
            let source = src; // NULL is acceptable to CGEventCreateKeyboardEvent

            let post = |code: u16, down: bool| {
                let ev = CGEventCreateKeyboardEvent(source, code, down);
                if !ev.is_null() {
                    CGEventSetFlags(ev, FLAG_COMMAND);
                    CGEventPost(HID_EVENT_TAP, ev);
                    CFRelease(ev);
                }
            };

            post(VK_COMMAND, true);
            std::thread::sleep(Duration::from_millis(20));
            post(keycode, true);
            std::thread::sleep(Duration::from_millis(20));
            post(keycode, false);
            std::thread::sleep(Duration::from_millis(20));
            post(VK_COMMAND, false);

            if !src.is_null() {
                CFRelease(src);
            }
        }
    }
}

/// Show a short status text next to the tray icon — the only feedback that
/// is guaranteed visible even when macOS notifications are blocked/denied.
fn set_status(app: &tauri::AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_title(Some(text));
    }
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

fn accessibility_granted() -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        AXIsProcessTrusted()
    }
    #[cfg(not(target_os = "macos"))]
    true
}

/// Ask macOS to register this app in System Settings → Accessibility and show
/// the system prompt. Without this call the app can be missing from the list
/// entirely, leaving the user no switch to flip.
///
/// Call it at most once per launch: the prompt is modal and re-raising it on a
/// timer is what makes the Privacy pane feel broken.
#[cfg(target_os = "macos")]
fn prompt_accessibility() -> bool {
    use std::ffi::c_void;
    use std::os::raw::c_char;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const c_char,
            enc: u32,
        ) -> *const c_void;
        fn CFDictionaryCreate(
            alloc: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(cf: *mut c_void);
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: u8;
        static kCFTypeDictionaryValueCallBacks: u8;
    }

    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

    unsafe {
        let key = CFStringCreateWithCString(
            std::ptr::null(),
            c"AXTrustedCheckOptionPrompt".as_ptr(),
            UTF8,
        );
        if key.is_null() {
            return false;
        }
        let keys = [key];
        let values = [kCFBooleanTrue];
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const u8 as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const u8 as *const c_void,
        );
        let trusted = AXIsProcessTrustedWithOptions(dict);
        if !dict.is_null() {
            CFRelease(dict as *mut c_void);
        }
        CFRelease(key as *mut c_void);
        trusted
    }
}

/// Poll the Accessibility grant a few times, spaced out, then stop.
///
/// `AXIsProcessTrusted()` is an XPC round-trip to tccd. A tight loop (the old
/// code checked every 2 s for five minutes) keeps tccd busy exactly while
/// System Settings → Privacy & Security → Accessibility enumerates its client
/// list, and that pane then hangs with a beachball. Six checks spread over two
/// minutes is enough to notice the switch being flipped without competing for
/// tccd, and at most one watcher runs at a time.
fn watch_accessibility(app: tauri::AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WATCHING: AtomicBool = AtomicBool::new(false);

    if accessibility_granted() {
        set_status(&app, "");
        return;
    }
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        for delay in [2u64, 4, 8, 16, 32, 60] {
            std::thread::sleep(std::time::Duration::from_secs(delay));
            if accessibility_granted() {
                set_status(&app, "");
                log::info!("accessibility permission granted — tray warning cleared");
                break;
            }
        }
        WATCHING.store(false, Ordering::SeqCst);
    });
}

fn set_busy(app: &tauri::AppHandle, busy: bool) {
    if let Some(tray) = app.tray_by_id("main") {
        let bytes: &[u8] = if busy {
            include_bytes!("../icons/tray-busy.png")
        } else {
            include_bytes!("../icons/tray.png")
        };
        if let Ok(img) = tauri::image::Image::from_bytes(bytes) {
            let _ = tray.set_icon(Some(img));
        }
        let _ = tray.set_tooltip(Some(if busy { "Correctr — working…" } else { "Correctr" }));
    }
}

fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

#[derive(serde::Serialize, Clone)]
struct PreviewStartPayload {
    original: String,
    initial: String,
    streaming: bool,
}

fn spawn_llm(app: tauri::AppHandle, prompt: String, original_text: Option<String>, auto_paste: bool) {
    tauri::async_runtime::spawn(async move {
        set_busy(&app, true);
        set_status(&app, "⏳");
        // Capture the app we must paste back into before the preview HUD (which
        // takes focus) is shown.
        remember_target(&app);
        let (resolved, preview_mode) = {
            let state = app.state::<AppState>();
            let cfg = state.config.lock().unwrap().clone();
            (engine::resolve(&cfg), cfg.preview_before_replace)
        };

        if preview_mode {
            show_preview_window(&app);
            let _ = app.emit(
                "preview-start",
                PreviewStartPayload {
                    original: original_text.clone().unwrap_or_default(),
                    initial: String::new(),
                    streaming: true,
                },
            );
        }

        let app_clone = app.clone();
        let result = providers::generate_stream(&resolved, &prompt, move |chunk| {
            if preview_mode {
                let _ = app_clone.emit("preview-chunk", chunk);
            }
        })
        .await;

        set_busy(&app, false);
        match result {
            Ok(text) if !text.trim().is_empty() => {
                if preview_mode {
                    let _ = app.emit("preview-done", &text);
                    set_status(&app, "✨");
                } else {
                    let state = app.state::<AppState>();
                    push_undo(&state.undo_stack, &read_clipboard(&app));
                    match write_clipboard(&app, &text) {
                        Ok(()) => {
                            let _ = app.emit("llm-done", &text);
                            set_status(&app, "✓");
                            notify(&app, "Correctr", "Result is on your clipboard ✓");
                            if auto_paste {
                                paste(&app);
                            }
                        }
                        Err(e) => {
                            log::error!("clipboard write failed: {e}");
                            let short: String = e.chars().take(48).collect();
                            set_status(&app, &format!("✗ {short}"));
                            notify(&app, "Correctr — clipboard error", &e);
                            let _ = app.emit("llm-error", e);
                        }
                    }
                }
            }
            Ok(_) => {
                if preview_mode {
                    let _ = app.emit("preview-error", "Model returned empty output");
                }
                set_status(&app, "✗ empty");
                notify(&app, "Correctr", "Model returned empty output");
            }
            Err(e) => {
                log::error!("LLM error: {e}");
                if preview_mode {
                    let _ = app.emit("preview-error", &e);
                }
                let short: String = e.chars().take(48).collect();
                set_status(&app, &format!("✗ {short}"));
                notify(&app, "Correctr — error", &e);
                let _ = app.emit("llm-error", e);
            }
        }
    });
}

/// Bundle id of the app that currently owns the keyboard focus.
///
/// `lsappinfo` is the only dependency-free way to ask LaunchServices; the
/// obvious alternatives (`osascript` + System Events) need an extra Automation
/// grant and are far slower.
#[cfg(target_os = "macos")]
fn frontmost_bundle_id() -> Option<String> {
    let asn = std::process::Command::new("/usr/bin/lsappinfo")
        .arg("front")
        .output()
        .ok()?;
    let asn = String::from_utf8_lossy(&asn.stdout).trim().to_string();
    if !asn.starts_with("ASN:") {
        return None;
    }
    let info = std::process::Command::new("/usr/bin/lsappinfo")
        .args(["info", "-only", "bundleid", &asn])
        .output()
        .ok()?;
    let out = String::from_utf8_lossy(&info.stdout);
    // "CFBundleIdentifier"="com.apple.TextEdit"
    let tail = out.split("\"=\"").nth(1)?;
    let id = tail.split('"').next()?.trim().to_string();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

/// Remember where the text came from, before our own window can take focus.
fn remember_target(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        if let Some(id) = frontmost_bundle_id() {
            if id != "app.correctr" {
                log::info!("target app = {id}");
                *app.state::<AppState>().target_app.lock().unwrap() = Some(id);
                return;
            }
        }
        log::warn!("could not determine the target app");
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// Give the frontmost app back its focus, so the ⌘V we are about to send lands
/// in the editor the text came from instead of inside Correctr's own webview.
///
/// A tray-menu click and the always-on-top preview HUD both make Correctr the
/// active app; without this the paste is delivered to an empty webview and the
/// user sees the clipboard update but nothing typed anywhere.
fn release_focus(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        // If the settings window is open the user is working inside Correctr —
        // hiding the whole app would close what they are looking at.
        let settings_open = app
            .get_webview_window("settings")
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false);
        if settings_open {
            return;
        }

        let _ = app.hide();

        // `hide()` deactivates Correctr, but activation order is not guaranteed
        // to hand focus back to the app the text came from — re-activate it
        // explicitly when we know which one it was.
        let target = app.state::<AppState>().target_app.lock().unwrap().clone();
        if let Some(id) = target {
            if id != "app.correctr" {
                let _ = std::process::Command::new("/usr/bin/open")
                    .args(["-b", &id])
                    .status();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

fn paste(app: &tauri::AppHandle) {
    // Posting ⌘V requires the same Accessibility grant as the ⌘C that read the
    // selection. Say so instead of failing silently.
    if !accessibility_granted() {
        notify(
            app,
            "Correctr — cannot paste",
            "Grant Accessibility to Correctr, then press the hotkey again. The result is on your clipboard.",
        );
        return;
    }

    release_focus(app);

    #[cfg(target_os = "macos")]
    {
        // Same proven path as the ⌘C used to grab the selection: real key
        // events from a private source with the Command flag set explicitly.
        // enigo's synthetic modifier press is not reliable here.
        log::info!("paste: sending ⌘V");
        keys::send_command_key(keys::VK_ANSI_V);
        *app.state::<AppState>().target_app.lock().unwrap() = None;
    }

    #[cfg(not(target_os = "macos"))]
    if let Ok(mut enigo) = enigo::Enigo::new(&enigo::Settings::default()) {
        use enigo::{Direction, Key, Keyboard};
        let _ = enigo.key(Key::Meta, Direction::Press);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = enigo.key(Key::Other(0x09), Direction::Click);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = enigo.key(Key::Meta, Direction::Release);
    }
}

#[tauri::command]
fn correct_clipboard(app: tauri::AppHandle) {
    let text = read_clipboard(&app);
    if text.trim().is_empty() {
        set_status(&app, "✗ copy first");
        notify(&app, "Correctr", "Clipboard is empty");
        return;
    }
    let prompt = engine::correct_prompt(&text);
    let auto_paste = app.state::<AppState>().config.lock().unwrap().auto_paste;
    spawn_llm(app, prompt, Some(text), auto_paste);
}

#[tauri::command]
fn translate_clipboard(app: tauri::AppHandle, lang: String) {
    let text = read_clipboard(&app);
    if text.trim().is_empty() {
        set_status(&app, "✗ copy first");
        notify(&app, "Correctr", "Clipboard is empty — copy some text first");
        return;
    }
    let prompt = engine::translate_prompt(&text, &lang);
    let auto_paste = app.state::<AppState>().config.lock().unwrap().auto_paste;
    spawn_llm(app, prompt, Some(text), auto_paste);
}

fn run_skill(app: tauri::AppHandle, skill_id: String) {
    let clip = read_clipboard(&app);
    let (prompt, auto_paste) = {
        let state = app.state::<AppState>();
        let cfg = state.config.lock().unwrap();
        let skill = cfg.skills.iter().find(|s| s.id == skill_id);
        match skill {
            Some(s) => (config::fill_template(&s.prompt, &clip), cfg.auto_paste),
            None => return,
        }
    };
    if prompt.trim().is_empty() {
        return;
    }
    spawn_llm(app, prompt, Some(clip), auto_paste);
}

#[tauri::command]
fn undo_clipboard(app: tauri::AppHandle) {
    let prev = {
        let state = app.state::<AppState>();
        let mut stack = state.undo_stack.lock().unwrap();
        stack.pop()
    };
    match prev {
        Some(text) => match write_clipboard(&app, &text) {
            Ok(()) => notify(&app, "Correctr", "Restored previous clipboard"),
            Err(e) => notify(&app, "Correctr — clipboard error", &e),
        },
        None => notify(&app, "Correctr", "Nothing to undo"),
    }
}

fn hotkey_correct(app: tauri::AppHandle) {
    // Never block the Carbon event handler: waiting for the hotkey modifiers
    // to come up plus polling the pasteboard can take ~2 s.
    std::thread::spawn(move || {
        if !accessibility_granted() {
            set_status(&app, "⚠ Accessibility");
            notify(
                &app,
                "Correctr — permission needed",
                "Grant Accessibility in System Settings → Privacy & Security → Accessibility",
            );
            return;
        }

        set_status(&app, "⏳");

        // 1. The hotkey fires while ⌃⌘ are still physically held. Synthesizing
        //    ⌘C right now is delivered as ⌃⌘C, which is not Copy anywhere, so
        //    nothing ever reached the pasteboard and the action reported
        //    "nothing selected". Let the real modifier keys come back up first.
        #[cfg(target_os = "macos")]
        {
            let released = keys::wait_for_modifier_release(1500);
            log::info!("hotkey: modifiers released={released}");
        }
        std::thread::sleep(std::time::Duration::from_millis(80));

        #[cfg(target_os = "macos")]
        let before_count = pasteboard::change_count();

        // 2. Ask the focused app for its selection with a real ⌘C.
        #[cfg(target_os = "macos")]
        keys::send_command_key(keys::VK_ANSI_C);
        #[cfg(not(target_os = "macos"))]
        if let Ok(mut enigo) = enigo::Enigo::new(&enigo::Settings::default()) {
            use enigo::{Direction, Key, Keyboard};
            let _ = enigo.key(Key::Meta, Direction::Press);
            let _ = enigo.key(Key::Other(0x08), Direction::Click);
            let _ = enigo.key(Key::Meta, Direction::Release);
        }

        // 3. Wait for the pasteboard to tick, then read the selection. The
        //    change counter — not a text comparison — distinguishes "the user
        //    re-copied the same text" from "nothing was copied".
        let mut selected = String::new();
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            #[cfg(target_os = "macos")]
            let copied = pasteboard::change_count() != before_count;
            #[cfg(not(target_os = "macos"))]
            let copied = true;
            if copied {
                let text = read_clipboard(&app);
                if !text.trim().is_empty() {
                    selected = text;
                }
                break;
            }
        }

        if selected.trim().is_empty() {
            set_status(&app, "✗ nothing selected");
            notify(&app, "Correctr", "Nothing selected (hotkey) — select text first");
            return;
        }

        log::info!("hotkey: captured {} chars", selected.chars().count());
        let auto_paste = app.state::<AppState>().config.lock().unwrap().auto_paste;
        spawn_llm(app, engine::correct_prompt(&selected), Some(selected), auto_paste);
    });
}

// ── Commands ─────────────────────────────────────────────────────────

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Config {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn save_config(app: tauri::AppHandle, cfg: Config) -> Result<Config, String> {
    let state = app.state::<AppState>();
    {
        let mut guard = state.config.lock().unwrap();
        *guard = cfg.clone();
    }
    config::save(&cfg).map_err(|e| e.to_string())?;
    refresh_tray(&app);
    Ok(cfg)
}

#[tauri::command]
fn get_skills(state: State<'_, AppState>) -> Vec<config::Skill> {
    state.config.lock().unwrap().skills.clone()
}

#[tauri::command]
fn add_skill(app: tauri::AppHandle, name: String, description: String, prompt: String) -> Result<Vec<config::Skill>, String> {
    let skills = {
        let state = app.state::<AppState>();
        let mut cfg = state.config.lock().unwrap();
        let mut id = config::slugify(&name);
        let mut n = 2;
        while cfg.skills.iter().any(|s| s.id == id) {
            id = format!("{}-{}", config::slugify(&name), n);
            n += 1;
        }
        cfg.skills.push(config::Skill {
            id,
            name,
            description,
            prompt,
            builtin: false,
        });
        cfg.skills.clone()
    };
    {
        let cfg = app.state::<AppState>().config.lock().unwrap().clone();
        config::save(&cfg).map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(skills)
}

#[tauri::command]
fn remove_skill(app: tauri::AppHandle, id: String) -> Result<Vec<config::Skill>, String> {
    let skills = {
        let state = app.state::<AppState>();
        let mut cfg = state.config.lock().unwrap();
        cfg.skills.retain(|s| s.id != id || s.builtin);
        cfg.skills.clone()
    };
    {
        let cfg = app.state::<AppState>().config.lock().unwrap().clone();
        config::save(&cfg).map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(skills)
}

#[tauri::command]
fn get_providers() -> serde_json::Value {
    let resolved_ids: Vec<String> = Vec::new();
    serde_json::json!({
        "builtin": providers::PROVIDERS.iter().map(|p| serde_json::json!({
            "id": p.id,
            "name": p.name,
            "default_model": p.default_model,
            "known_models": p.known_models,
            "key_url": p.key_url,
            "requires_key": p.requires_key,
        })).collect::<Vec<_>>(),
        "resolved": resolved_ids,
    })
}

#[tauri::command]
async fn list_models(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let resolved = {
        let cfg = state.config.lock().unwrap().clone();
        engine::resolve(&cfg)
    };
    let models = providers::list_models(&resolved).await?;
    // Persist the fetched list so the dropdown survives restarts.
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.models.insert(resolved.id.clone(), models.clone());
        let snapshot = cfg.clone();
        drop(cfg);
        config::save(&snapshot).map_err(|e| e.to_string())?;
    }
    Ok(models)
}

#[tauri::command]
fn add_custom_provider(
    app: tauri::AppHandle,
    name: String,
    base_url: String,
    api_key: String,
    model: String,
) -> Result<Config, String> {
    let cfg = {
        let state = app.state::<AppState>();
        let mut cfg = state.config.lock().unwrap();
        engine::add_custom_provider(&mut cfg, name, base_url, api_key, model);
        cfg.clone()
    };
    config::save(&cfg).map_err(|e| e.to_string())?;
    refresh_tray(&app);
    Ok(cfg)
}

#[tauri::command]
fn remove_custom_provider(app: tauri::AppHandle, id: String) -> Result<Config, String> {
    let cfg = {
        let state = app.state::<AppState>();
        let mut cfg = state.config.lock().unwrap();
        cfg.custom_providers.retain(|p| p.id != id);
        if cfg.provider == id {
            cfg.provider = "openai".into();
        }
        cfg.clone()
    };
    config::save(&cfg).map_err(|e| e.to_string())?;
    refresh_tray(&app);
    Ok(cfg)
}

#[tauri::command]
async fn test_connection(state: State<'_, AppState>) -> Result<String, String> {
    let resolved = {
        let cfg = state.config.lock().unwrap().clone();
        engine::resolve(&cfg)
    };
    match providers::list_models(&resolved).await {
        Ok(models) => Ok(format!("{} connected — {} models", resolved.name, models.len())),
        Err(e) => Err(e),
    }
}

#[tauri::command]
fn run_action(app: tauri::AppHandle, action: String) {
    match action.as_str() {
        "correct" => correct_clipboard(app),
        _ if action.starts_with("skill:") => {
            run_skill(app, action.trim_start_matches("skill:").to_string())
        }
        _ => {}
    }
}

#[tauri::command]
fn reveal_config() -> Result<(), String> {
    let path = config::config_path();
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_hotkey(state: State<'_, AppState>) -> String {
    state.config.lock().unwrap().hotkey.clone()
}

/// Canonical form for the global-shortcut plugin: `ctrl+cmd+c` — modifiers
/// first, the key last. Returns empty when no key is present.
fn normalize_hotkey(hotkey: &str) -> String {
    let mut mods = Vec::new();
    let mut key = String::new();
    for raw in hotkey.split('+') {
        let p = raw.trim().to_lowercase();
        if p.is_empty() {
            continue;
        }
        match p.as_str() {
            "ctrl" | "control" => mods.push("ctrl"),
            "cmd" | "command" | "meta" => mods.push("cmd"),
            "alt" | "option" => mods.push("alt"),
            "shift" => mods.push("shift"),
            _ => key = p,
        }
    }
    if key.is_empty() {
        return String::new();
    }
    mods.push(key.as_str());
    mods.join("+")
}

#[tauri::command]
fn set_hotkey(app: tauri::AppHandle, hotkey: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let old = {
        let mut cfg = state.config.lock().unwrap();
        let old = cfg.hotkey.clone();
        cfg.hotkey = hotkey.clone();
        config::save(&cfg).map_err(|e| e.to_string())?;
        old
    };
    let shortcut = normalize_hotkey(&hotkey);
    if shortcut.is_empty() {
        return Err("Hotkey must include a key (e.g. Ctrl+Cmd+C)".into());
    }
    let gs = app.global_shortcut();
    let old_norm = normalize_hotkey(&old);
    if !old_norm.is_empty() && old_norm != shortcut {
        // Drop the old combo so two hotkeys don't both fire.
        let _ = gs.unregister(old_norm.as_str());
    }
    let app_h = app.clone();
    gs.on_shortcut(shortcut.as_str(), move |_a, _s, ev| {
        if ev.state() == ShortcutState::Pressed {
            hotkey_correct(app_h.clone());
        }
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn accept_preview(app: tauri::AppHandle, text: String) -> Result<(), String> {
    hide_preview_window(&app);
    let state = app.state::<AppState>();
    push_undo(&state.undo_stack, &read_clipboard(&app));
    write_clipboard(&app, &text)?;
    let _ = app.emit("llm-done", &text);
    set_status(&app, "✓");
    // Accepting the preview *is* the "replace my text" gesture (↵), so paste
    // unconditionally — `auto_paste` only governs the no-preview flow, where it
    // decides whether we type the result for the user or just leave it on the
    // clipboard.
    paste(&app);
    Ok(())
}

#[tauri::command]
fn dismiss_preview(app: tauri::AppHandle) {
    hide_preview_window(&app);
    set_status(&app, "");
}

#[tauri::command]
fn get_undo_available(state: State<'_, AppState>) -> bool {
    !state.undo_stack.lock().unwrap().is_empty()
}


// ── Verification of the raw NSPasteboard path ─────────────────────────
// Exercises exactly what read_clipboard/write_clipboard use, including that a
// *separate process* can see the text (a same-process read-back alone would
// not prove the pasteboard really changed).
#[cfg(all(test, target_os = "macos"))]
mod pasteboard_tests {
    use super::pasteboard;

    #[test]
    fn write_read_and_change_count() {
        let original = pasteboard::get_text();
        let before = pasteboard::change_count();

        let probe = "Correctr pasteboard probe \u{2713} line2\n";
        assert!(pasteboard::set_text(probe), "set_text returned false");
        assert_eq!(
            pasteboard::get_text().as_deref(),
            Some(probe),
            "read-back after set_text"
        );
        assert_ne!(
            pasteboard::change_count(),
            before,
            "changeCount must tick on write"
        );

        let out = std::process::Command::new("/usr/bin/pbpaste")
            .output()
            .expect("pbpaste");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            probe,
            "another process must see the text"
        );

        if let Some(o) = original {
            let _ = pasteboard::set_text(&o);
            assert_eq!(pasteboard::get_text().as_deref(), Some(o.as_str()));
        }
    }
}

#[cfg(test)]
mod hotkey_tests {
    use super::normalize_hotkey;

    #[test]
    fn canonicalizes_modifiers_then_key() {
        assert_eq!(normalize_hotkey("Ctrl+Cmd+C"), "ctrl+cmd+c");
        assert_eq!(normalize_hotkey("  Control + Command + X "), "ctrl+cmd+x");
        assert_eq!(normalize_hotkey("Cmd+Shift+Z"), "cmd+shift+z");
    }

    #[test]
    fn key_must_be_present() {
        assert_eq!(normalize_hotkey("Ctrl+Shift"), "");
        assert_eq!(normalize_hotkey(""), "");
    }
}

#[cfg(test)]
mod undo_tests {
    use super::{push_undo, UNDO_LIMIT};
    use std::sync::Mutex;

    #[test]
    fn dedupes_consecutive_entries() {
        let stack = Mutex::new(Vec::new());
        push_undo(&stack, "a");
        push_undo(&stack, "a");
        push_undo(&stack, "b");
        assert_eq!(stack.lock().unwrap().len(), 2);
    }

    #[test]
    fn caps_at_limit_keeping_newest() {
        let stack = Mutex::new(Vec::new());
        for i in 0..(UNDO_LIMIT + 5) {
            push_undo(&stack, &format!("v{i}"));
        }
        let s = stack.lock().unwrap();
        assert_eq!(s.len(), UNDO_LIMIT);
        assert_eq!(s[0], format!("v5")); // the five oldest were dropped
        assert_eq!(s[s.len() - 1], format!("v{}", UNDO_LIMIT + 4));
    }
}