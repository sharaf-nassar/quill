#[cfg(not(target_os = "macos"))]
pub fn init<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("window-chrome").build()
}

#[cfg(target_os = "macos")]
pub fn init<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("window-chrome")
        .on_window_ready(hide_standard_buttons)
        .build()
}

#[cfg(target_os = "macos")]
fn hide_standard_buttons<R: tauri::Runtime>(window: tauri::Window<R>) {
    use objc2_app_kit::{NSWindow, NSWindowButton};

    let Ok(handle) = window.ns_window() else {
        log::warn!(
            "Could not access native window chrome for {}",
            window.label()
        );
        return;
    };
    let window: &NSWindow = unsafe { &*handle.cast() };

    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = window.standardWindowButton(kind) {
            button.setHidden(true);
        }
    }
}
