use std::time::Duration;

use gpui_kit::{AnyWindowHandle, App, Entity, Window};
use tracing::warn;
use tray_icon::{
    Icon as TrayIconImage, MouseButton, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

use crate::TestrixApp;

/// RAII guard for the primary GUI instance (enforces single-instance on Windows).
pub struct SingleInstanceGuard {
    #[cfg(target_os = "windows")]
    mutex_handle: isize,
    #[cfg(target_os = "windows")]
    event_handle: isize,
}

#[cfg(target_os = "windows")]
static SHOW_WINDOW_EVENT: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[cfg(target_os = "windows")]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

        SHOW_WINDOW_EVENT.store(0, std::sync::atomic::Ordering::Release);
        // SAFETY: Handles were created by `CreateMutexW`/`CreateEventW` and are owned by this guard.
        unsafe {
            if self.event_handle != 0 {
                CloseHandle(self.event_handle as HANDLE);
            }
            if self.mutex_handle != 0 {
                CloseHandle(self.mutex_handle as HANDLE);
            }
        }
    }
}

/// Ensures only a single GUI instance runs on Windows.
///
/// If another instance is already running, signals it to restore/focus its window and returns
/// `None` so the duplicate process can exit immediately without creating a second tray icon.
pub fn acquire_single_instance_or_activate_existing() -> Option<SingleInstanceGuard> {
    #[cfg(target_os = "windows")]
    {
        use std::ptr::null;
        use windows_sys::Win32::Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, GetLastError,
        };
        use windows_sys::Win32::System::Threading::{
            CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, SetEvent,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

        let mutex_name: Vec<u16> = "Local\\TestrixSingleInstanceMutex\0".encode_utf16().collect();
        let event_name: Vec<u16> = "Local\\TestrixShowWindowEvent\0".encode_utf16().collect();

        // SAFETY: Null-terminated UTF-16 strings are valid for the duration of the Win32 calls.
        unsafe {
            let mutex = CreateMutexW(null(), 1, mutex_name.as_ptr());
            if mutex.is_null() {
                // Fail open if Win32 mutex creation fails unexpectedly.
                return Some(SingleInstanceGuard {
                    mutex_handle: 0,
                    event_handle: 0,
                });
            }

            if GetLastError() == ERROR_ALREADY_EXISTS {
                AllowSetForegroundWindow(ASFW_ANY);
                let ev = OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr());
                if !ev.is_null() {
                    SetEvent(ev);
                    CloseHandle(ev);
                }
                CloseHandle(mutex);
                return None;
            }

            let ev = CreateEventW(null(), 0, 0, event_name.as_ptr());
            let ev_raw = if ev.is_null() { 0 } else { ev as isize };
            SHOW_WINDOW_EVENT.store(ev_raw, std::sync::atomic::Ordering::Release);
            Some(SingleInstanceGuard {
                mutex_handle: mutex as isize,
                event_handle: ev_raw,
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Some(SingleInstanceGuard {})
    }
}

fn poll_show_window_event() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        let ev = SHOW_WINDOW_EVENT.load(std::sync::atomic::Ordering::Acquire);
        if ev == 0 {
            return false;
        }
        // SAFETY: `ev` is a valid Win32 event handle owned by `SingleInstanceGuard`.
        unsafe { WaitForSingleObject(ev as HANDLE, 0) == WAIT_OBJECT_0 }
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Initializes the cross-platform system tray icon and its Show / Quit event loop.
///
/// Failures (for example, in headless Linux/CI environments) are logged and ignored so the
/// main window still launches normally.
pub fn init_system_tray(
    window_handle: AnyWindowHandle,
    app_entity: Entity<TestrixApp>,
    cx: &mut App,
) {
    let icon = match load_tray_icon() {
        Ok(icon) => icon,
        Err(err) => {
            warn!(error = %err, "failed loading tray icon");
            return;
        }
    };

    let menu = Menu::new();
    let show_item = MenuItem::with_id("show", "Show Testrix", true, None);
    let quit_item = MenuItem::with_id("quit", "Quit Testrix", true, None);
    let _ = menu.append(&show_item);
    let _ = menu.append(&quit_item);

    let tray = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Testrix")
        .with_icon(icon)
        .build()
    {
        Ok(tray) => tray,
        Err(err) => {
            warn!(error = %err, "failed creating system tray icon");
            return;
        }
    };

    cx.spawn(async move |cx| {
        // Keep the tray icon alive for the lifetime of the loop and drop it cleanly on quit.
        let _tray = tray;
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;

            let mut should_show = poll_show_window_event();
            let mut should_quit = false;

            while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                match ev {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    }
                    | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } => {
                        should_show = true;
                    }
                    _ => {}
                }
            }

            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                if ev.id.0 == "show" {
                    should_show = true;
                } else if ev.id.0 == "quit" {
                    should_quit = true;
                }
            }

            if should_quit {
                let _ = cx.update(|cx| {
                    app_entity.update(cx, |app, _| {
                        app.set_force_quit();
                    });
                    cx.quit();
                });
                break;
            }

            if should_show {
                let _ = cx.update(|cx| {
                    cx.activate(true);
                    let _ = window_handle.update(cx, |_, window, _| {
                        show_window_from_tray(window);
                        window.activate_window();
                    });
                });
            }
        }
    })
    .detach();
}

/// Hides the main window into the system tray without leaving a taskbar button on Windows.
pub(crate) fn hide_window_to_tray(window: &Window, cx: &App) {
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        cx.hide();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = cx;
        win32_set_window_visibility(window, false);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = cx;
        window.minimize_window();
    }
}

fn show_window_from_tray(window: &Window) {
    #[cfg(target_os = "windows")]
    win32_set_window_visibility(window, true);
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}

#[cfg(target_os = "windows")]
fn win32_set_window_visibility(window: &Window, show: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOW, ShowWindow};

    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::Win32(win32) = handle.as_raw()
    {
        let hwnd = win32.hwnd.get() as HWND;
        let cmd = if show { SW_SHOW } else { SW_HIDE };
        // SAFETY: `hwnd` is obtained from the live GPUI `Window` on the owning UI thread.
        unsafe {
            ShowWindow(hwnd, cmd);
        }
    }
}

fn load_tray_icon() -> Result<TrayIconImage, String> {
    let raw = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .map_err(|err| err.to_string())?
        .to_rgba8();
    let (src_w, src_h) = raw.dimensions();

    // Crop transparent padding and outer drop-shadow so the icon fills the tray slot.
    let mut min_x = src_w;
    let mut min_y = src_h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    for (x, y, px) in raw.enumerate_pixels() {
        if px[3] >= 180 {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }

    let cropped = if min_x <= max_x && min_y <= max_y {
        image::imageops::crop_imm(
            &raw,
            min_x,
            min_y,
            max_x - min_x + 1,
            max_y - min_y + 1,
        )
        .to_image()
    } else {
        raw
    };

    let resized =
        image::imageops::resize(&cropped, 64, 64, image::imageops::FilterType::Lanczos3);
    let (w, h) = resized.dimensions();
    TrayIconImage::from_rgba(resized.into_raw(), w, h).map_err(|err| err.to_string())
}
