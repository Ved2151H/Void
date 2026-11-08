use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, Window};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::EnumWindows;
use crate::services::enum_windows_proc;
use crate::types::AppInfo;

#[tauri::command]
pub fn set_window_height(window: Window, height: f64) {
    if let Ok(scale_factor) = window.scale_factor() {
        if let Ok(physical_size) = window.inner_size() {
            let logical_width = physical_size.width as f64 / scale_factor;
            let _ = window.set_size(tauri::LogicalSize::new(logical_width, height));
        }
    }
}

#[tauri::command]
pub fn resize_settings_window(app: AppHandle, width: f64, height: f64) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.set_size(tauri::LogicalSize::new(width, height));
    }
}

#[tauri::command]
pub fn set_ignore_cursor_events(window: Window, ignore: bool) {
    let _ = window.set_ignore_cursor_events(ignore);
}

#[tauri::command]
pub async fn get_active_windows() -> Vec<AppInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
        // COM is required while enumerating: each window's AppUserModelID is read
        // through the shell property store.
        let com_initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() };

        let mut apps: Vec<AppInfo> = Vec::new();
        unsafe {
            let _ = EnumWindows(
                Some(enum_windows_proc),
                LPARAM(&mut apps as *mut Vec<AppInfo> as isize),
            );
        }

        let mut grouped: HashMap<String, AppInfo> = HashMap::new();

        for app in apps {
            let path = app.path.to_lowercase();
            let name = app.name.to_lowercase();

            // For host processes (Edge, Chrome, Brave, ApplicationFrameHost), use path + name
            // so that different PWAs/UWP apps are separate dock items.
            let key = if path.contains("msedge.exe")
                || path.contains("chrome.exe")
                || path.contains("brave.exe")
                || path.contains("applicationframehost.exe")
            {
                format!("{}:{}", path, name)
            } else if let Some(ref exe) = app.executable {
                format!("{}:{}", path, exe.to_lowercase())
            } else {
                path.clone()
            };

            if let Some(existing) = grouped.get_mut(&key) {
                if let Some(ref mut hwnds) = existing.all_hwnds {
                    hwnds.push((app.hwnd.unwrap_or(0), app.name.clone()));
                } else {
                    existing.all_hwnds = Some(vec![
                        (existing.hwnd.unwrap_or(0), existing.name.clone()),
                        (app.hwnd.unwrap_or(0), app.name.clone()),
                    ]);
                }
            } else {
                let mut new_app = app.clone();
                new_app.all_hwnds = Some(vec![(app.hwnd.unwrap_or(0), app.name.clone())]);
                grouped.insert(key, new_app);
            }
        }

        let focus_guard = if let Some(map) = crate::state::FOCUS_TIMESTAMPS.get() {
            map.lock().ok()
        } else {
            None
        };

        let mut result_apps: Vec<AppInfo> = grouped.into_values().collect();

        for app in &mut result_apps {
            if let Some(ref mut hwnds) = app.all_hwnds {
                hwnds.sort_by(|a, b| {
                    let ts_a = focus_guard
                        .as_ref()
                        .and_then(|g| g.get(&a.0))
                        .copied()
                        .unwrap_or(0);
                    let ts_b = focus_guard
                        .as_ref()
                        .and_then(|g| g.get(&b.0))
                        .copied()
                        .unwrap_or(0);
                    ts_b.cmp(&ts_a)
                });
            }
        }

        if com_initialized {
            unsafe {
                CoUninitialize();
            }
        }

        result_apps
    })
    .await
    .unwrap_or_default()
}

/// Where to put a window whose restore rectangle is on no screen, in screen
/// coordinates: centered in `work`, at its own size if that is at least a
/// third of the work area each way (shrunk to fit), otherwise at two thirds
/// of the work area. An off-screen restore rectangle usually carries a broken
/// size too, such as an app's minimum size.
fn on_screen_restore_rect(
    normal: windows::Win32::Foundation::RECT,
    work: windows::Win32::Foundation::RECT,
) -> windows::Win32::Foundation::RECT {
    let work_w = work.right - work.left;
    let work_h = work.bottom - work.top;
    let fit = |size: i32, available: i32| {
        if size >= available / 3 {
            size.min(available)
        } else {
            available * 2 / 3
        }
    };
    let w = fit(normal.right - normal.left, work_w);
    let h = fit(normal.bottom - normal.top, work_h);
    let left = work.left + (work_w - w) / 2;
    let top = work.top + (work_h - h) / 2;
    windows::Win32::Foundation::RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

/// Before the dock restores a minimized window: if the position Windows will
/// restore it to lies on no monitor, move that position onto the monitor
/// under the cursor (the dock's), so the window doesn't come back off-screen.
/// Apps can leave the off-screen spot Windows parks minimized windows at
/// (-32000, -32000) as their restore position, and the taskbar would restore
/// them there too.
unsafe fn bring_restore_position_on_screen(hwnd: HWND) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetWindowLongW, GetWindowPlacement, SetWindowPlacement, GWL_EXSTYLE,
        WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
    };

    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    if GetWindowPlacement(hwnd, &mut placement).is_err() {
        return;
    }

    let monitor_info = |monitor| {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(monitor, &mut info)
            .as_bool()
            .then_some(info)
    };
    // The restore rectangle is in workspace coordinates, which are offset by
    // the work area of the monitor it belongs to, unless the window is a tool
    // window. An off-screen rectangle has no monitor; the nearest one is it.
    let tool_window = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
    let normal = placement.rcNormalPosition;
    let Some(source) = monitor_info(MonitorFromRect(&normal, MONITOR_DEFAULTTONEAREST)) else {
        return;
    };
    let source_offset = workspace_offset(&source, tool_window);
    let on_screen = shift_rect(normal, source_offset);
    if !MonitorFromRect(&on_screen, MONITOR_DEFAULTTONULL).is_invalid() {
        return;
    }

    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let Some(target) = monitor_info(MonitorFromPoint(cursor, MONITOR_DEFAULTTOPRIMARY)) else {
        return;
    };
    placement.rcNormalPosition = restore_rect_on_monitor(on_screen, &target, tool_window);
    let _ = SetWindowPlacement(hwnd, &placement);
}

/// How far `monitor`'s work area is inset from its top-left corner: what
/// separates screen from workspace coordinates on it. Tool windows use
/// screen coordinates, so none.
fn workspace_offset(
    monitor: &windows::Win32::Graphics::Gdi::MONITORINFO,
    tool_window: bool,
) -> (i32, i32) {
    if tool_window {
        (0, 0)
    } else {
        (
            monitor.rcWork.left - monitor.rcMonitor.left,
            monitor.rcWork.top - monitor.rcMonitor.top,
        )
    }
}

fn shift_rect(
    rect: windows::Win32::Foundation::RECT,
    (dx, dy): (i32, i32),
) -> windows::Win32::Foundation::RECT {
    windows::Win32::Foundation::RECT {
        left: rect.left + dx,
        top: rect.top + dy,
        right: rect.right + dx,
        bottom: rect.bottom + dy,
    }
}

/// The restore rectangle, in the target monitor's workspace coordinates, that
/// centers a window (sized from its screen rectangle `on_screen`) in that
/// monitor's work area.
fn restore_rect_on_monitor(
    on_screen: windows::Win32::Foundation::RECT,
    target: &windows::Win32::Graphics::Gdi::MONITORINFO,
    tool_window: bool,
) -> windows::Win32::Foundation::RECT {
    let (dx, dy) = workspace_offset(target, tool_window);
    shift_rect(on_screen_restore_rect(on_screen, target.rcWork), (-dx, -dy))
}

/// The window outside Bloom that most recently became the foreground window.
fn last_focused_window() -> Option<HWND> {
    let map = crate::state::FOCUS_TIMESTAMPS.get()?.lock().ok()?;
    let (&last, _) = map.iter().max_by_key(|(_, &at)| at)?;
    let last = HWND(last as *mut _);
    unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(last)) }
        .as_bool()
        .then_some(last)
}

#[tauri::command]
pub async fn focus_window(app: AppHandle, hwnd: isize) -> bool {
    let dock = app
        .get_webview_window("dock")
        .and_then(|window| window.hwnd().ok())
        .map(|dock| dock.0 as isize);
    tauri::async_runtime::spawn_blocking(move || unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            BringWindowToTop, FlashWindow, GetAncestor, GetForegroundWindow, IsIconic, IsWindow,
            IsWindowVisible, SetForegroundWindow, ShowWindow, GA_ROOTOWNER, SW_MINIMIZE,
            SW_RESTORE, SW_SHOW,
        };
        let hwnd = HWND(hwnd as *mut _);
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }

        // A second click toggles only this exact foreground window. Another
        // window from the same process (or a recently focused target) is not
        // grounds to minimize it. A popup or dialog the window owns (a WinUI
        // flyout, a Save dialog) counts as the window itself, as on the taskbar.
        let mut foreground = GetForegroundWindow();
        // Clicking the dock can make it the foreground window for a moment.
        // The window in front is then the one focused last before the click.
        if dock == Some(foreground.0 as isize) {
            if let Some(last) = last_focused_window() {
                foreground = last;
            }
        }
        if (foreground == hwnd || GetAncestor(foreground, GA_ROOTOWNER) == hwnd)
            && IsWindowVisible(hwnd).as_bool()
            && !IsIconic(hwnd).as_bool()
        {
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
            return IsIconic(hwnd).as_bool();
        }

        if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        if IsIconic(hwnd).as_bool() {
            bring_restore_position_on_screen(hwnd);
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }

        if SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() == hwnd {
            return true;
        }
        // Foreground activation is restricted by Windows. Uncover the window,
        // retry once, then request attention instead of silently claiming focus.
        let _ = BringWindowToTop(hwnd);
        if GetForegroundWindow() == hwnd
            || (SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() == hwnd)
        {
            return true;
        }
        let _ = FlashWindow(hwnd, true);
        false
    })
    .await
    .unwrap_or(false)
}

/// Brings one window of a multi-window dock item forward, like the taskbar's
/// Win+number / Ctrl+click. `hwnds` comes most-recently-focused first: with
/// none of them in front, that one is activated; with one in front, the next
/// window in a stable order is, so repeated clicks visit every window instead
/// of flipping between the two most recent.
#[tauri::command]
pub async fn focus_app_windows(hwnds: Vec<isize>) {
    tauri::async_runtime::spawn_blocking(move || unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow,
            SW_RESTORE, SW_SHOW,
        };
        let Some(&most_recent) = hwnds.first() else {
            return;
        };
        let foreground = GetForegroundWindow().0 as isize;
        let mut ring = hwnds.clone();
        ring.sort_unstable();
        let target = match ring.iter().position(|&h| h == foreground) {
            Some(i) => ring[(i + 1) % ring.len()],
            None => most_recent,
        };

        let hwnd = HWND(target as *mut _);
        if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        if IsIconic(hwnd).as_bool() {
            bring_restore_position_on_screen(hwnd);
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    })
    .await
    .unwrap_or_default();
}

#[tauri::command]
pub fn hide_native_osd() {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            FindWindowA, IsWindowVisible, ShowWindow, SW_HIDE,
        };
        let class1 = windows::core::PCSTR(c"NativeHWNDHost".as_ptr() as *const u8);
        if let Ok(hwnd) = FindWindowA(class1, windows::core::PCSTR::null()) {
            if IsWindowVisible(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }
}

#[tauri::command]
pub fn open_settings_window(app: AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        if let Ok(hwnd) = win.hwnd() {
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{
                    SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
                };
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }
}

/// Dock clicks on the Settings icon behave like a taskbar button: hide the
/// window when it is already in front, otherwise bring the single existing
/// Settings window forward. Never creates a second window.
#[tauri::command]
pub fn toggle_settings_window(app: AppHandle) {
    let Some(win) = app.get_webview_window("settings") else {
        return;
    };
    let in_front = win.is_visible().unwrap_or(false) && win.is_focused().unwrap_or(false);
    if in_front {
        let _ = win.hide();
        return;
    }
    open_settings_window(app);
}

#[tauri::command]
pub fn hide_overlay(app: AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.hide();
    }
}

/// Pins the bottom-corners strip to the bottom edge of the primary monitor.
/// The strip is only as tall as the corner masks, so its CSS `bottom: 0`
/// lands on the physical screen edge at any scale factor.
pub fn position_bottom_corners(win: &tauri::WebviewWindow) {
    if let Ok(Some(monitor)) = win.primary_monitor() {
        let scale = monitor.scale_factor();
        let position = monitor.position();
        let size = monitor.size();
        let height = (38.0 * scale).round().max(1.0) as i32;
        let _ = win.set_position(tauri::PhysicalPosition::new(
            position.x,
            position.y + size.height as i32 - height,
        ));
        let _ = win.set_size(tauri::PhysicalSize::new(size.width, height as u32));
    }
}

/// Shows the bottom corner masks only while the notch is on screen and the
/// "Screen Corners" setting is enabled.
#[tauri::command]
pub fn set_bottom_corners_visible(app: AppHandle, visible: bool) {
    let Some(win) = app.get_webview_window("bottom-corners") else {
        return;
    };
    if visible {
        position_bottom_corners(&win);
        let _ = win.show();
        if let Ok(hwnd) = win.hwnd() {
            crate::utils::re_assert_topmost(hwnd);
        }
    } else {
        let _ = win.hide();
    }
}

#[tauri::command]
pub fn set_splash_fullscreen(app: AppHandle, fullscreen: bool) {
    crate::state::OVERLAY_IN_SPLASH.store(fullscreen, Ordering::Relaxed);
    if let Some(win) = app.get_webview_window("overlay") {
        if fullscreen {
            let _ = win.hide();
            if let Ok(Some(monitor)) = win.primary_monitor() {
                let size = monitor.size();
                let pos = monitor.position();
                let _ = win.set_position(tauri::PhysicalPosition::new(pos.x, pos.y));
                let _ = win.set_size(tauri::PhysicalSize::new(size.width, size.height));
            }
            let _ = win.show();
        } else {
            let _ = win.hide();
            crate::services::sync_overlays(&app);
        }
    }
}

#[tauri::command]
pub fn sync_overlay_position(app: AppHandle) {
    crate::services::sync_overlays(&app);
}

#[tauri::command]
pub async fn close_window(hwnd: isize) {
    tauri::async_runtime::spawn_blocking(move || unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        let hwnd = HWND(hwnd as *mut _);
        let _ = PostMessageW(
            Some(hwnd),
            WM_CLOSE,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        );
    })
    .await
    .unwrap_or_default();
}

#[tauri::command]
pub async fn capture_window_thumbnail(
    hwnd: isize,
    max_width: u32,
    max_height: u32,
) -> Result<Option<(String, i64)>, String> {
    tauri::async_runtime::spawn_blocking(move || unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow};

        let hwnd = HWND(hwnd as *mut _);
        if !IsWindow(Some(hwnd)).as_bool() {
            return None;
        }

        let hwnd_key = hwnd.0 as isize;
        let is_minimized = IsIconic(hwnd).as_bool();

        let mut focus_time = 0i64;
        if let Some(map) = crate::state::FOCUS_TIMESTAMPS.get() {
            if let Ok(guard) = map.lock() {
                focus_time = guard.get(&hwnd_key).copied().unwrap_or(0);
            }
        }

        if is_minimized {
            // Never restore a minimized window to capture it: that briefly
            // un-minimizes it on screen. Thumbnails are kept warm by the
            // minimize/focus window hooks, so minimized windows normally have
            // a cached image already; without one we simply show no preview.
            let cached_img = if let Some(cache) = crate::state::THUMBNAIL_CACHE.get() {
                cache
                    .lock()
                    .ok()
                    .and_then(|g| g.get(&hwnd_key).map(|(img, _)| img.clone()))
            } else {
                None
            };
            return cached_img.map(|img| (img, focus_time));
        }

        let result = crate::utils::capture_hwnd_to_base64(hwnd, max_width, max_height);

        if let Some(ref img) = result {
            crate::services::cache_thumbnail(hwnd_key, img.clone());
        }

        result.map(|img| (img, focus_time))
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod restore_rect_tests {
    use super::*;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::MONITORINFO;

    fn rect(left: i32, top: i32, w: i32, h: i32) -> RECT {
        RECT {
            left,
            top,
            right: left + w,
            bottom: top + h,
        }
    }

    fn monitor(screen: RECT, work: RECT) -> MONITORINFO {
        MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: screen,
            rcWork: work,
            dwFlags: 0,
        }
    }

    #[test]
    fn converts_with_each_monitors_own_reservation() {
        // Primary with no reservation; a secondary to its right with a 60px
        // bar along its top, so its workspace origin is 60px below its screen
        // origin.
        let primary = monitor(rect(0, 0, 2880, 1800), rect(0, 0, 2880, 1800));
        let secondary = monitor(rect(2880, 0, 1920, 1080), rect(2880, 60, 1920, 1020));
        assert_eq!(workspace_offset(&primary, false), (0, 0));
        assert_eq!(workspace_offset(&secondary, false), (0, 60));

        // A 945x200 stub restored on the secondary keeps its width (over a
        // third of 1920) and gets two thirds of the height, 680. Centered in
        // the work area that is (3367, 230) on screen, (3367, 170) in that
        // monitor's workspace coordinates.
        let stub = rect(-32000, -32000, 945, 200);
        assert_eq!(
            restore_rect_on_monitor(stub, &secondary, false),
            rect(3367, 170, 945, 680)
        );
        // On the primary, screen and workspace coordinates coincide.
        assert_eq!(
            restore_rect_on_monitor(stub, &primary, false),
            rect(480, 300, 1920, 1200)
        );
    }

    #[test]
    fn keeps_tool_windows_in_screen_coordinates() {
        let secondary = monitor(rect(2880, 0, 1920, 1080), rect(2960, 0, 1840, 1080));
        assert_eq!(workspace_offset(&secondary, true), (0, 0));
        let stub = rect(-32000, -32000, 945, 200);
        // Centered in the work area right of an 80px bar along the left edge,
        // and returned in screen coordinates.
        assert_eq!(
            restore_rect_on_monitor(stub, &secondary, true),
            rect(3407, 180, 945, 720)
        );
    }

    #[test]
    fn keeps_a_usable_size_and_centers_it() {
        let work = rect(0, 0, 2880, 1800);
        assert_eq!(
            on_screen_restore_rect(rect(-32000, -32000, 1200, 800), work),
            rect(840, 500, 1200, 800)
        );
    }

    #[test]
    fn replaces_a_stub_size_with_two_thirds_of_the_work_area() {
        // A Windows Terminal left at its minimum size at the parking spot.
        let work = rect(0, 0, 2880, 1800);
        assert_eq!(
            on_screen_restore_rect(rect(-32000, -32000, 945, 200), work),
            rect(480, 300, 1920, 1200)
        );
    }

    #[test]
    fn shrinks_an_oversized_window_to_the_work_area_of_its_monitor() {
        let work = rect(2880, 100, 1920, 980);
        assert_eq!(
            on_screen_restore_rect(rect(-32000, -32000, 3000, 2000), work),
            work
        );
    }
}
