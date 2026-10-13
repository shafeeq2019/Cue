//! Global hotkey manager with conflict detection.
//!
//! Two kinds of conflicts are detected:
//! 1. Known OS shortcuts (Spotlight, input-source switching, …) – reported
//!    as errors or warnings before registering.
//! 2. Shortcuts held by other applications – the OS refuses the registration
//!    (Windows `RegisterHotKey`, X11 grab, macOS Carbon), which is reported
//!    as a conflict and the previous shortcut is restored.

use serde::Serialize;
use tauri::{AppHandle, Runtime};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

#[derive(Debug, Clone, Serialize, Default)]
pub struct HotkeyCheck {
    pub ok: bool,
    /// Normalised display form, e.g. "Alt+Space".
    pub normalized: Option<String>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Severity {
    Block,
    Warn,
}

/// (modifiers, key, severity, description) per platform.
fn known_conflicts() -> Vec<(Modifiers, Code, Severity, &'static str)> {
    let (a, c, s, m) = (Modifiers::ALT, Modifiers::CONTROL, Modifiers::SHIFT, Modifiers::SUPER);
    if cfg!(target_os = "macos") {
        vec![
            (m, Code::Space, Severity::Block, "⌘ Space opens Spotlight."),
            (m | a, Code::Space, Severity::Warn, "⌥⌘ Space opens a Finder search window."),
            (c, Code::Space, Severity::Warn, "⌃ Space switches the input source on macOS."),
            (c | m, Code::Space, Severity::Warn, "⌃⌘ Space opens the emoji picker."),
            (m, Code::Tab, Severity::Block, "⌘ Tab switches applications."),
            (m, Code::KeyQ, Severity::Block, "⌘ Q quits applications."),
            (m | s, Code::Digit3, Severity::Block, "Screenshot shortcut."),
            (m | s, Code::Digit4, Severity::Block, "Screenshot shortcut."),
            (m | s, Code::Digit5, Severity::Block, "Screenshot shortcut."),
            (a, Code::Space, Severity::Warn, "⌥ Space normally types a non-breaking space; Cue will capture it in all apps."),
        ]
    } else if cfg!(target_os = "windows") {
        vec![
            (a, Code::Tab, Severity::Block, "Alt+Tab switches windows."),
            (a, Code::F4, Severity::Block, "Alt+F4 closes windows."),
            (m, Code::KeyL, Severity::Block, "Win+L locks the computer."),
            (m, Code::KeyD, Severity::Block, "Win+D shows the desktop."),
            (m, Code::Space, Severity::Block, "Win+Space switches the keyboard layout."),
            (c | s, Code::Escape, Severity::Block, "Ctrl+Shift+Esc opens Task Manager."),
            (a, Code::Space, Severity::Warn, "Alt+Space opens the window menu and is used by PowerToys Run."),
            (c, Code::Space, Severity::Warn, "Ctrl+Space toggles the input method for Chinese/Japanese/Korean keyboards."),
        ]
    } else {
        vec![
            (c, Code::Space, Severity::Warn, "Ctrl+Space may toggle the input method (IBus/Fcitx)."),
            (m, Code::Space, Severity::Warn, "Super+Space may switch keyboard layouts."),
            (a, Code::Tab, Severity::Block, "Alt+Tab switches windows."),
        ]
    }
}

/// Validate syntax and known conflicts (does not touch OS registration).
pub fn check(accelerator: &str) -> HotkeyCheck {
    let parsed: Result<Shortcut, _> = accelerator.parse();
    let sc = match parsed {
        Ok(s) => s,
        Err(e) => return HotkeyCheck { ok: false, error: Some(format!("Not a valid shortcut: {e}")), ..Default::default() },
    };
    let mut out = HotkeyCheck { ok: true, normalized: Some(display(accelerator)), ..Default::default() };
    let function_key = matches!(
        sc.key,
        Code::F1 | Code::F2 | Code::F3 | Code::F4 | Code::F5 | Code::F6 | Code::F7 | Code::F8 | Code::F9 | Code::F10 | Code::F11 | Code::F12
    );
    if sc.mods.is_empty() && !function_key {
        out.ok = false;
        out.error = Some("Use at least one modifier (Ctrl, Alt/Option, Shift or Cmd/Win).".into());
        return out;
    }
    for (mods, key, sev, why) in known_conflicts() {
        if sc.mods == mods && sc.key == key {
            match sev {
                Severity::Block => {
                    out.ok = false;
                    out.error = Some(format!("Reserved by the system: {why}"));
                }
                Severity::Warn => out.warnings.push(why.to_string()),
            }
        }
    }
    out
}

/// Human-friendly label for the current platform.
pub fn display(accelerator: &str) -> String {
    accelerator
        .split('+')
        .map(|p| match p.trim().to_ascii_lowercase().as_str() {
            "alt" | "option" => if cfg!(target_os = "macos") { "Option".into() } else { "Alt".into() },
            "control" | "ctrl" => "Ctrl".into(),
            "super" | "cmd" | "command" | "meta" => if cfg!(target_os = "macos") { "Cmd".into() } else { "Win".into() },
            "commandorcontrol" | "cmdorctrl" => if cfg!(target_os = "macos") { "Cmd".into() } else { "Ctrl".into() },
            _ => p.trim().to_string(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Register `new`, replacing `old`. On failure the old shortcut is restored.
pub fn apply<R: Runtime>(app: &AppHandle<R>, old: Option<&str>, new: &str) -> HotkeyCheck {
    let mut res = check(new);
    if !res.ok {
        return res;
    }
    let gs = app.global_shortcut();
    if let Some(o) = old {
        let _ = gs.unregister(o);
    }
    if let Err(e) = gs.register(new) {
        res.ok = false;
        res.error = Some(format!(
            "This shortcut is already in use by another application or the system ({e}). Choose a different one."
        ));
        if let Some(o) = old {
            let _ = gs.register(o);
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates() {
        assert!(check("Alt+Space").ok || cfg!(target_os = "macos"));
        assert!(!check("Space").ok);
        assert!(!check("Ctrl+Nope").ok);
        assert!(check("F9").ok);
        assert!(check("Control+Shift+K").ok);
    }

    #[test]
    fn known_conflicts_are_reported() {
        if cfg!(target_os = "macos") {
            assert!(!check("Super+Space").ok);
            assert!(!check("Alt+Space").warnings.is_empty());
        } else if cfg!(target_os = "windows") {
            assert!(!check("Alt+Tab").ok);
            assert!(!check("Control+Space").warnings.is_empty());
        } else {
            assert!(!check("Alt+Tab").ok);
            assert!(!check("Control+Space").warnings.is_empty());
        }
    }
}
