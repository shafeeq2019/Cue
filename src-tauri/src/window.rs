//! Floating palette window: show on the monitor under the cursor, focus the
//! input, hide on Esc / blur.

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Runtime};

pub const MAIN: &str = "main";

pub fn show<R: Runtime>(app: &AppHandle<R>) {
    let Some(w) = app.get_webview_window(MAIN) else { return };
    if let Some(st) = app.try_state::<std::sync::Arc<crate::state::AppState>>() {
        if let Ok(mut t) = st.shown_at.lock() {
            *t = Some(std::time::Instant::now());
        }
    }
    // Place the palette on the monitor that contains the mouse cursor,
    // horizontally centred and in the upper third, like Spotlight/Raycast.
    if let Ok(cursor) = app.cursor_position() {
        if let (Ok(Some(mon)), Ok(size)) = (app.monitor_from_point(cursor.x, cursor.y), w.outer_size()) {
            let area = mon.work_area();
            let x = area.position.x + (area.size.width as i32 - size.width as i32) / 2;
            let y = area.position.y + (area.size.height as i32 / 5).max(0);
            let _ = w.set_position(PhysicalPosition::new(x, y));
        }
    }
    let _ = w.unminimize();
    let _ = w.show();
    let _ = w.set_focus();
    let _ = app.emit("cue://shown", ());
}

pub fn hide<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.hide();
    }
    // Give focus back to the previously active application.
    #[cfg(target_os = "macos")]
    let _ = app.hide();
}

pub fn toggle<R: Runtime>(app: &AppHandle<R>) {
    let visible_and_focused = app
        .get_webview_window(MAIN)
        .map(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
        .unwrap_or(false);
    if visible_and_focused {
        hide(app);
    } else {
        show(app);
    }
}

pub fn open_settings<R: Runtime>(app: &AppHandle<R>) {
    show(app);
    let _ = app.emit("cue://open-settings", ());
}
