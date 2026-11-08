use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::Foundation::HWND;
use crate::services::{
    disable_dock_appbar, register_dock_appbar, sync_overlays, unregister_appbar_native,
};
use crate::state::*;
use crate::types::IntRect;
use crate::utils::{dock_enabled, re_assert_topmost, set_taskbar_visibility};

#[tauri::command]
pub async fn set_menu_open(open: bool, rect: Option<IntRect>) {
    MENU_IS_OPEN.store(open, Ordering::Relaxed);
    if let Ok(mut r) = MENU_RECT.lock() {
        *r = rect;
    }
}

#[tauri::command]
pub async fn set_dock_hovered(hovered: bool) {
    DOCK_IS_HOVERED.store(hovered, Ordering::Relaxed);
}

#[tauri::command]
pub async fn set_notch_hovered(app: AppHandle, hovered: bool) {
    NOTCH_IS_HOVERED.store(hovered, Ordering::Relaxed);
    // Re-evaluate right away. During a native drag the webview never sees the
    // pointer leave, so waiting for the next mouse move can leave the notch
    // (and its edge-hover state) stuck expanded in peek/smart mode.
    crate::services::refresh_main_interaction(&app);
}

#[tauri::command]
pub async fn set_notch_visible(visible: bool) {
    NOTCH_IS_VISIBLE.store(visible, Ordering::Relaxed);
}

#[tauri::command]
pub async fn update_dock_rect(rect: IntRect, overlap_rect: Option<IntRect>) {
    if let Ok(mut r) = DOCK_RECT.lock() {
        *r = Some(rect);
    }
    if let Some(rect) = overlap_rect {
        if let Ok(mut r) = DOCK_OVERLAP_RECT.lock() {
            *r = Some(rect);
        }
    }
}

#[tauri::command]
pub async fn update_notch_rect(rect: IntRect) {
    if let Ok(mut r) = NOTCH_RECT.lock() {
        *r = Some(rect);
    }
}

#[tauri::command]
pub async fn init_dock(app: AppHandle, mode: String) {
    // Backend guard: bail if dock is disabled in settings.
    // The frontend already checks this, but settings.json may have a stale
    // value if the write didn't complete before restart. Reading here too
    // makes the dock reliably stay hidden regardless of frontend timing.
    if !dock_enabled() {
        disable_dock(&app);
        return;
    }

    if let Some(dock_win) = app.get_webview_window("dock") {
        // 1. Always show first ΓÇö idempotent, required before any positioning
        if dock_win.show().is_err() {
            return;
        }
        if let Ok(hwnd) = dock_win.hwnd() {
            re_assert_topmost(hwnd);
        }

        // 2. Register as appbar (fixed) or manually position (auto-hide)
        if mode == "fixed" {
            // register_dock_appbar calls show() internally too, and handles retries
            register_dock_appbar(dock_win.clone());
        } else {
            // Auto-hide mode: position at bottom of screen.
            // Retry until primary_monitor() is available (can fail on autostart before shell).
            let dock_clone = dock_win.clone();
            tauri::async_runtime::spawn(async move {
                for attempt in 0..20 {
                    if !dock_enabled() {
                        return;
                    }
                    // Wait for monitor and window dimensions to be available.
                    // Never use a hardcoded fallback ΓÇö wrong values produce off-screen placement.
                    // Extract HWND as isize before any await (raw pointer is not Send).
                    let hwnd_val = dock_clone.hwnd().map(|h| h.0 as isize).unwrap_or(0);
                    let ph = dock_clone
                        .outer_size()
                        .map(|s| s.height as i32)
                        .unwrap_or(0);
                    let monitor_info = dock_clone.primary_monitor().ok().flatten().map(|m| {
                        let s = m.size();
                        let p = m.position();
                        (
                            tauri::PhysicalSize::new(s.width, s.height),
                            tauri::PhysicalPosition::new(p.x, p.y),
                        )
                    });

                    if hwnd_val != 0 {
                        if let Some((m_size, m_pos)) = monitor_info {
                            if ph <= 10 {
                                // outer_size() not ready yet ΓÇö retry next tick
                                if attempt < 19 {
                                    tokio::time::sleep(tokio::time::Duration::from_millis(200))
                                        .await;
                                }
                                continue;
                            }
                            let final_y = m_pos.y + m_size.height as i32 - ph;
                            let positioned = unsafe {
                                use windows::Win32::Foundation::HWND;
                                use windows::Win32::UI::WindowsAndMessaging::{
                                    GetWindowRect, SetWindowPos, SWP_FRAMECHANGED, SWP_NOACTIVATE,
                                    SWP_NOZORDER,
                                };
                                let hwnd = HWND(hwnd_val as *mut _);
                                let moved = SetWindowPos(
                                    hwnd,
                                    None,
                                    m_pos.x,
                                    final_y,
                                    m_size.width as i32,
                                    ph,
                                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                                )
                                .is_ok();
                                let mut rect = windows::Win32::Foundation::RECT::default();
                                moved
                                    && GetWindowRect(hwnd, &mut rect).is_ok()
                                    && rect.left == m_pos.x
                                    && rect.top == final_y
                                    && rect.right - rect.left == m_size.width as i32
                                    && rect.bottom - rect.top == ph
                            };
                            if !positioned {
                                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                                continue;
                            }
                            // Re-assert topmost after repositioning
                            if let Ok(hwnd) = dock_clone.hwnd() {
                                re_assert_topmost(hwnd);
                            }
                            // Ensure visible after positioning
                            if dock_enabled()
                                && dock_clone.show().is_ok()
                                && dock_clone.is_visible().unwrap_or(false)
                                && set_taskbar_visibility(false, false)
                            {
                                NATIVE_TASKBAR_HIDDEN.store(true, Ordering::Relaxed);
                            }
                            break;
                        }
                    }
                    if attempt < 19 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    }
                }
            });
        }

        // Fixed mode completes the taskbar handoff in register_dock_appbar_inner;
        // auto-hide mode completes it after SetWindowPos and show both succeed.
        // Reset overlap state so the overlap thread re-syncs cleanly.
        CURRENT_DOCK_OVERLAP.store(0, Ordering::Relaxed);
        let _ = app.emit("dock-overlap", false);
        // Sync the adaptive-dock signal so the dock expands immediately if a
        // maximized window is already in the foreground when it's enabled.
        let _ = app.emit(
            "dock-maximized",
            CURRENT_FOREGROUND_MAXIMIZED.load(Ordering::Relaxed),
        );
    }
}

fn disable_dock(app: &AppHandle) {
    // Clear this before showing the native taskbar: the WinEvent hook can run
    // during ShowWindow and would otherwise immediately hide it again.
    NATIVE_TASKBAR_HIDDEN.store(false, Ordering::Relaxed);
    // Keep Bloom's dock available if Explorer has not made its taskbar visible.
    if !set_taskbar_visibility(true, true) {
        return;
    }
    if let Some(dock_win) = app.get_webview_window("dock") {
        disable_dock_appbar(dock_win);
    }
    DOCK_APPBAR_REGISTERED.store(false, Ordering::Relaxed);
    crate::services::reconcile_main_appbar(app);
}

#[tauri::command]
pub async fn toggle_dock(app: AppHandle, enable: bool) {
    if enable {
        // Load the saved dock mode; "smart" is the fresh-install default.
        let saved_mode = crate::utils::get_setting_str(&app, "bloom-dock-mode")
            .unwrap_or_else(|| "smart".to_string());
        init_dock(app, saved_mode).await;
    } else {
        disable_dock(&app);
    }
}

#[tauri::command]
pub async fn sync_appbar(app: AppHandle) {
    // Reconcile the notch AppBar against the configured mode so a stale
    // registration cannot survive DPI/power events (issue #109).
    crate::services::reconcile_main_appbar(&app);
    if let Some(dock_win) = app.get_webview_window("dock") {
        // Skip dock re-registration if dock is disabled in settings.
        if dock_enabled() && DOCK_APPBAR_REGISTERED.load(Ordering::Relaxed) {
            register_dock_appbar(dock_win);
        } else {
            if let Ok(hwnd) = dock_win.hwnd() {
                re_assert_topmost(hwnd);
            }
        }
    }
    sync_overlays(&app);
}

#[tauri::command]
pub async fn change_dock_mode(app: AppHandle, mode: String) {
    // A disabled dock keeps the new mode in settings (init_dock applies it on
    // re-enable) but must not be shown or replace the native taskbar now.
    if !dock_enabled() {
        return;
    }
    if let Some(dock_win) = app.get_webview_window("dock") {
        if mode == "fixed" {
            register_dock_appbar(dock_win.clone());
        } else {
            let _ = dock_win.show();
            if let Ok(hwnd) = dock_win.hwnd() {
                let hwnd_val = hwnd.0 as isize;
                tauri::async_runtime::spawn_blocking(move || {
                    unregister_appbar_native(HWND(hwnd_val as *mut _));
                });
                DOCK_APPBAR_REGISTERED.store(false, Ordering::Relaxed);

                // Retry primary_monitor() ΓÇö can fail on autostart before shell initializes
                let dock_clone = dock_win.clone();
                tauri::async_runtime::spawn(async move {
                    for attempt in 0..10 {
                        // Never fall back to a hardcoded pixel height.
                        // Extract HWND as isize before any await (raw pointer is not Send).
                        let hwnd_val = dock_clone.hwnd().map(|h| h.0 as isize).unwrap_or(0);
                        let ph = dock_clone
                            .outer_size()
                            .map(|s| s.height as i32)
                            .unwrap_or(0);
                        let monitor_info = dock_clone.primary_monitor().ok().flatten().map(|m| {
                            let s = m.size();
                            let p = m.position();
                            (
                                tauri::PhysicalSize::new(s.width, s.height),
                                tauri::PhysicalPosition::new(p.x, p.y),
                            )
                        });

                        if hwnd_val != 0 {
                            if let Some((m_size, m_pos)) = monitor_info {
                                if ph <= 10 {
                                    if attempt < 9 {
                                        tokio::time::sleep(tokio::time::Duration::from_millis(300))
                                            .await;
                                    }
                                    continue;
                                }
                                let final_y = m_pos.y + m_size.height as i32 - ph;
                                let positioned = unsafe {
                                    use windows::Win32::Foundation::HWND;
                                    use windows::Win32::UI::WindowsAndMessaging::{
                                        GetWindowRect, SetWindowPos, SWP_FRAMECHANGED,
                                        SWP_NOACTIVATE, SWP_NOZORDER,
                                    };
                                    let hwnd = HWND(hwnd_val as *mut _);
                                    let moved = SetWindowPos(
                                        hwnd,
                                        None,
                                        m_pos.x,
                                        final_y,
                                        m_size.width as i32,
                                        ph,
                                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                                    )
                                    .is_ok();
                                    let mut rect = windows::Win32::Foundation::RECT::default();
                                    moved
                                        && GetWindowRect(hwnd, &mut rect).is_ok()
                                        && rect.left == m_pos.x
                                        && rect.top == final_y
                                        && rect.right - rect.left == m_size.width as i32
                                        && rect.bottom - rect.top == ph
                                };
                                if !positioned {
                                    tokio::time::sleep(tokio::time::Duration::from_millis(300))
                                        .await;
                                    continue;
                                }
                                if let Ok(hwnd) = dock_clone.hwnd() {
                                    re_assert_topmost(hwnd);
                                }
                                if dock_enabled()
                                    && dock_clone.show().is_ok()
                                    && dock_clone.is_visible().unwrap_or(false)
                                    && set_taskbar_visibility(false, false)
                                {
                                    NATIVE_TASKBAR_HIDDEN.store(true, Ordering::Relaxed);
                                }
                                break;
                            }
                        }
                        if attempt < 9 {
                            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                        }
                    }
                });
            }
        }

        // Ensure always on top and native taskbar stays hidden
        if let Ok(hwnd) = dock_win.hwnd() {
            re_assert_topmost(hwnd);
        }
        // Sync the current overlap state immediately to the frontend
        let current = CURRENT_DOCK_OVERLAP.load(Ordering::Relaxed);
        if current != -1 {
            let _ = app.emit("dock-overlap", current == 1);
        }
        let _ = app.emit(
            "dock-maximized",
            CURRENT_FOREGROUND_MAXIMIZED.load(Ordering::Relaxed),
        );

        // Double sync after a short delay to catch any layout changes
        let dock_clone = dock_win.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            if DOCK_APPBAR_REGISTERED.load(Ordering::Relaxed) {
                register_dock_appbar(dock_clone);
            }
        });
    }
}

#[tauri::command]
pub async fn change_notch_mode(app: AppHandle, mode: String) {
    if let Some(main_win) = app.get_webview_window("main") {
        if mode == "fixed" {
            crate::services::apply_main_appbar_mode(&app, true);
        } else {
            let _ = main_win.show();
            // Locked helper so a concurrent display/power event cannot
            // interleave between the mode decision and the AppBar call.
            crate::services::apply_main_appbar_mode(&app, false);

            let main_clone = main_win.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                if !MAIN_APPBAR_REGISTERED.load(Ordering::Relaxed) {
                    if let Ok(hwnd) = main_clone.hwnd() {
                        re_assert_topmost(hwnd);
                    }
                }
            });
        }
        // Reposition window to span the full primary monitor so CSS justify-content:center works
        if let Ok(Some(monitor)) = main_win.primary_monitor() {
            let m_pos = monitor.position();
            let m_size = monitor.size();
            let scale = monitor.scale_factor();
            let bloom_scale = crate::utils::get_bloom_scale(&app);
            let target_height = (420.0 * bloom_scale * scale) as u32;
            let _ = main_win.set_position(tauri::PhysicalPosition::new(m_pos.x, m_pos.y));
            let _ = main_win.set_size(tauri::PhysicalSize::new(m_size.width, target_height));
        }

        let current = CURRENT_NOTCH_OVERLAP.load(Ordering::Relaxed);
        if current != -1 {
            let _ = app.emit("notch-overlap", current == 1);
        }
    }
}

/// Unregisters the notch and dock AppBars so Windows gives their work area back.
pub fn release_appbars(handle: &AppHandle) {
    if let Some(w) = handle.get_webview_window("main") {
        if let Ok(hwnd) = w.hwnd() {
            unregister_appbar_native(hwnd);
        }
    }
    if let Some(w) = handle.get_webview_window("dock") {
        if let Ok(hwnd) = w.hwnd() {
            unregister_appbar_native(hwnd);
        }
    }
    MAIN_APPBAR_REGISTERED.store(false, Ordering::Relaxed);
    DOCK_APPBAR_REGISTERED.store(false, Ordering::Relaxed);
}

/// Restore the native taskbar, unregister Bloom's appbars, and exit gracefully.
/// Shared by the tray menu, the in-app Quit button, and the window CloseRequested
/// handlers so that any shutdown path (including Task Manager's WM_CLOSE) behaves
/// identically.
pub fn restore_taskbar_and_exit(handle: &AppHandle) {
    crate::diagnostics::record("shutdown", "restoring taskbar and releasing AppBars");
    SHUTTING_DOWN.store(true, Ordering::Relaxed);
    release_appbars(handle);
    set_taskbar_visibility(true, true);
    NATIVE_TASKBAR_HIDDEN.store(false, Ordering::Relaxed);
    crate::services::set_native_osd_suppressed(false);
    // Destroy all webview windows before exiting so Chromium's UnregisterClass for
    // Chrome_WidgetWin_0 finds no open windows (avoids the harmless Error=1412 log).
    for (_, w) in handle.webview_windows() {
        let _ = w.destroy();
    }
    handle.exit(0);
}

#[tauri::command]
pub async fn quit_bloom(handle: AppHandle) {
    restore_taskbar_and_exit(&handle);
}

#[tauri::command]
pub async fn restart_bloom(handle: AppHandle) {
    crate::diagnostics::record("restart", "restoring taskbar and releasing AppBars");
    SHUTTING_DOWN.store(true, Ordering::Relaxed);
    release_appbars(&handle);
    if let Some(w) = handle.get_webview_window("settings") {
        let _ = w.destroy();
    }
    set_taskbar_visibility(true, true);
    NATIVE_TASKBAR_HIDDEN.store(false, Ordering::Relaxed);
    crate::services::set_native_osd_suppressed(false);
    close_single_instance_handles();
    handle.restart();
}
