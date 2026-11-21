use std::time::Duration;

use gpui_kit::{AnyWindowHandle, App, Entity, Window};
use tracing::warn;
use tray_icon::{
    Icon as TrayIconImage, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

use crate::TestrixApp;

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

    // Keep the tray icon alive for the lifetime of the application on the main thread.
    std::mem::forget(Box::new(tray) as Box<TrayIcon>);

    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;

            let mut should_show = false;
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

    if let Ok(handle) = window.window_handle()
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
    let img = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .map_err(|err| err.to_string())?
        .resize_exact(32, 32, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let (w, h) = img.dimensions();
    TrayIconImage::from_rgba(img.into_raw(), w, h).map_err(|err| err.to_string())
}
