//! macOS NSStatusItem keep-alive — workaround for [tauri-apps/tauri#12060].
//! Remove this module once upstream ships a fix.
//!
//! [tauri-apps/tauri#12060]: https://github.com/tauri-apps/tauri/issues/12060

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &tauri::AppHandle) {}

#[cfg(target_os = "macos")]
pub fn install(app: &tauri::AppHandle) {
    macos::install(app);
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use block2::RcBlock;
    use objc2_app_kit::{
        NSApplicationDidChangeScreenParametersNotification, NSWorkspace,
        NSWorkspaceDidWakeNotification,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue};
    use tauri::{AppHandle, Manager};

    const DEBOUNCE: Duration = Duration::from_millis(500);

    pub fn install(app: &AppHandle) {
        let app = app.clone();
        let last = Arc::new(Mutex::new(
            Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
        ));

        let block: RcBlock<dyn Fn(NonNull<NSNotification>) + 'static> = {
            let last = Arc::clone(&last);
            RcBlock::new(move |_notif: NonNull<NSNotification>| {
                {
                    let mut guard = last.lock().unwrap();
                    if guard.elapsed() < DEBOUNCE {
                        return;
                    }
                    *guard = Instant::now();
                }
                rebuild_tray(&app);
            })
        };

        unsafe {
            let main_queue = NSOperationQueue::mainQueue();

            let workspace_center = NSWorkspace::sharedWorkspace().notificationCenter();
            workspace_center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                Some(&main_queue),
                &block,
            );

            let default_center = NSNotificationCenter::defaultCenter();
            default_center.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
                Some(&main_queue),
                &block,
            );
        }

        log::info!("tray_keepalive: registered for sleep/wake and screen-change");
    }

    // Toggle visibility off then on so tray-icon drops and recreates the
    // NSStatusItem; set_icon alone only updates the existing button image
    // and would not re-attach a detached TrayTarget subview.
    fn rebuild_tray(app: &AppHandle) {
        let Some(tray) = app.tray_by_id(crate::TRAY_ID) else {
            return;
        };
        if let Err(e) = tray.set_visible(false) {
            log::warn!("tray_keepalive: hide failed: {e}");
            return;
        }
        if let Err(e) = tray.set_visible(true) {
            log::warn!("tray_keepalive: re-show failed: {e}");
            return;
        }
        log::info!("tray_keepalive: NSStatusItem rebuilt after wake/screen-change");
    }
}
