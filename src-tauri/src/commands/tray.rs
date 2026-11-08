use crate::utils::dock_enabled;

#[tauri::command]
pub fn open_notification_center() {
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::ShellExecuteA;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let _ = ShellExecuteA(
            Some(HWND(std::ptr::null_mut())),
            windows::core::PCSTR(c"open".as_ptr() as *const u8),
            windows::core::PCSTR(c"ms-actioncenter:".as_ptr() as *const u8),
            windows::core::PCSTR::null(),
            windows::core::PCSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[tauri::command]
pub fn open_system_tray() {
    // The native taskbar is already available when the dock is disabled.
    if !dock_enabled() {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || unsafe {
        use std::sync::atomic::Ordering;
        use windows::core::PCSTR;
        use windows::Win32::UI::WindowsAndMessaging::{
            FindWindowA, GetWindowLongA, IsWindowVisible, SetLayeredWindowAttributes,
            SetWindowLongA, ShowWindow, GWL_EXSTYLE, LWA_ALPHA, SW_SHOW, WS_EX_LAYERED,
            WS_EX_TRANSPARENT,
        };

        if !dock_enabled() {
            return;
        }

        let tray_class = PCSTR(c"Shell_TrayWnd".as_ptr() as *const u8);
        let hwnd = FindWindowA(tray_class, windows::core::PCSTR::null()).unwrap_or_default();
        if hwnd.0.is_null() {
            return;
        }

        let currently_hidden = crate::state::NATIVE_TASKBAR_HIDDEN.load(Ordering::Relaxed);
        if currently_hidden {
            // 1. Make the taskbar completely invisible and click-through
            let exstyle = GetWindowLongA(hwnd, GWL_EXSTYLE);
            let _ = SetWindowLongA(
                hwnd,
                GWL_EXSTYLE,
                exstyle | WS_EX_LAYERED.0 as i32 | WS_EX_TRANSPARENT.0 as i32,
            );
            let _ = SetLayeredWindowAttributes(
                hwnd,
                windows::Win32::Foundation::COLORREF(0),
                0,
                LWA_ALPHA,
            );

            // 2. Show the taskbar window WITHOUT calling ABM_SETSTATE (prevents work-area
            //    recalculation which would displace the notch/dock appbars).
            use crate::utils::ORIGINAL_TRAY_RECT;
            use windows::Win32::UI::WindowsAndMessaging::{
                SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            };
            if let Ok(guard) = ORIGINAL_TRAY_RECT.lock() {
                if let Some(rect) = *guard {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        rect.left,
                        rect.top,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
            crate::state::NATIVE_TASKBAR_HIDDEN.store(false, Ordering::Relaxed);

            // Give Windows a moment to realize the taskbar is "there"
            std::thread::sleep(std::time::Duration::from_millis(50));

            // 3. Send the Win+B and Space macro to open the tray chevron
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                SendInput, INPUT, INPUT_0, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_LWIN,
                VK_SPACE,
            };
            let b_key = VIRTUAL_KEY(0x42); // 'B' key

            let inputs = [
                // Win+B
                INPUT {
                    r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VK_LWIN,
                            wScan: 0,
                            dwFlags: Default::default(),
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
                INPUT {
                    r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: b_key,
                            wScan: 0,
                            dwFlags: Default::default(),
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
                INPUT {
                    r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: b_key,
                            wScan: 0,
                            dwFlags: KEYEVENTF_KEYUP,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
                INPUT {
                    r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VK_LWIN,
                            wScan: 0,
                            dwFlags: KEYEVENTF_KEYUP,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
            ];
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);

            std::thread::sleep(std::time::Duration::from_millis(150));

            // Space to open the popup
            let space_inputs = [
                INPUT {
                    r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VK_SPACE,
                            wScan: 0,
                            dwFlags: Default::default(),
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
                INPUT {
                    r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VK_SPACE,
                            wScan: 0,
                            dwFlags: KEYEVENTF_KEYUP,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
            ];
            SendInput(&space_inputs, std::mem::size_of::<INPUT>() as i32);

            // 4. Start monitoring for the tray popup to close
            std::thread::spawn(move || {
                let overflow_class =
                    PCSTR(c"TopLevelWindowForOverflowXamlIsland".as_ptr() as *const u8);
                let win10_overflow_class = PCSTR(c"NotifyIconOverflowWindow".as_ptr() as *const u8);

                // Wait for it to appear
                let mut found = false;
                for _ in 0..50 {
                    let h1 = FindWindowA(overflow_class, PCSTR::null()).unwrap_or_default();
                    let h2 = FindWindowA(win10_overflow_class, PCSTR::null()).unwrap_or_default();
                    if (!h1.0.is_null() && IsWindowVisible(h1).as_bool())
                        || (!h2.0.is_null() && IsWindowVisible(h2).as_bool())
                    {
                        found = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }

                if found {
                    // Wait for it to disappear
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        let h1 = FindWindowA(overflow_class, PCSTR::null()).unwrap_or_default();
                        let h2 =
                            FindWindowA(win10_overflow_class, PCSTR::null()).unwrap_or_default();
                        let visible = (!h1.0.is_null() && IsWindowVisible(h1).as_bool())
                            || (!h2.0.is_null() && IsWindowVisible(h2).as_bool());
                        if !visible {
                            break;
                        }
                    }
                }

                // Once closed, hide the taskbar only if the dock is still enabled.
                if dock_enabled() {
                    let hidden = crate::utils::set_taskbar_visibility(false, false);
                    crate::state::NATIVE_TASKBAR_HIDDEN.store(hidden, Ordering::Relaxed);
                }

                // Revert transparency
                let tray_class = PCSTR(c"Shell_TrayWnd".as_ptr() as *const u8);
                let hwnd = FindWindowA(tray_class, PCSTR::null()).unwrap_or_default();
                if !hwnd.0.is_null() {
                    let exstyle = GetWindowLongA(hwnd, GWL_EXSTYLE);
                    let _ = SetLayeredWindowAttributes(
                        hwnd,
                        windows::Win32::Foundation::COLORREF(0),
                        255,
                        LWA_ALPHA,
                    );
                    let _ = SetWindowLongA(
                        hwnd,
                        GWL_EXSTYLE,
                        exstyle & !(WS_EX_LAYERED.0 as i32) & !(WS_EX_TRANSPARENT.0 as i32),
                    );
                }
            });
        } else {
            // If already toggled on manually, toggle off
            let hidden = crate::utils::set_taskbar_visibility(false, false);
            crate::state::NATIVE_TASKBAR_HIDDEN.store(hidden, Ordering::Relaxed);

            let exstyle = GetWindowLongA(hwnd, GWL_EXSTYLE);
            let _ = SetLayeredWindowAttributes(
                hwnd,
                windows::Win32::Foundation::COLORREF(0),
                255,
                LWA_ALPHA,
            );
            let _ = SetWindowLongA(
                hwnd,
                GWL_EXSTYLE,
                exstyle & !(WS_EX_LAYERED.0 as i32) & !(WS_EX_TRANSPARENT.0 as i32),
            );
        }
    });
}

#[tauri::command]
pub async fn get_tray_apps() -> Vec<crate::tray::TrayApp> {
    tauri::async_runtime::spawn_blocking(crate::tray::enumerate)
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub async fn show_tray_context_menu(tray_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || crate::tray::interact(&tray_id, true))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemAction {
    TaskManager,
    DiskManagement,
    DeviceManager,
    ComputerManagement,
    Settings,
    TaskbarSettings,
    InstalledApps,
    PowerOptions,
    NetworkConnections,
    TaskView,
}

#[tauri::command]
pub async fn open_system_action(action: SystemAction) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || unsafe {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let mut directory = [0u16; 32768];
        let length =
            windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut directory));
        if length == 0 || length as usize >= directory.len() {
            return Err("Windows system directory is unavailable.".into());
        }
        let system = String::from_utf16_lossy(&directory[..length as usize]);
        let (file, args) = match action {
            SystemAction::TaskManager => (format!("{system}\\Taskmgr.exe"), String::new()),
            SystemAction::DiskManagement => (
                format!("{system}\\mmc.exe"),
                format!("\"{system}\\diskmgmt.msc\""),
            ),
            SystemAction::DeviceManager => (
                format!("{system}\\mmc.exe"),
                format!("\"{system}\\devmgmt.msc\""),
            ),
            SystemAction::ComputerManagement => (
                format!("{system}\\mmc.exe"),
                format!("\"{system}\\compmgmt.msc\""),
            ),
            SystemAction::Settings => ("ms-settings:".into(), String::new()),
            SystemAction::TaskbarSettings => ("ms-settings:taskbar".into(), String::new()),
            SystemAction::InstalledApps => ("ms-settings:appsfeatures".into(), String::new()),
            SystemAction::PowerOptions => ("ms-settings:powersleep".into(), String::new()),
            SystemAction::NetworkConnections => ("ms-settings:network".into(), String::new()),
            // Task View's shell folder opens Task View like the taskbar's
            // button, without sending Win+Tab, which a held Ctrl or Shift
            // would turn into a different shortcut.
            SystemAction::TaskView => (
                "explorer.exe".into(),
                "shell:::{3080F90E-D7AD-11D9-BD98-0000947B0257}".into(),
            ),
        };
        let file: Vec<u16> = file.encode_utf16().chain(Some(0)).collect();
        let args: Vec<u16> = args.encode_utf16().chain(Some(0)).collect();
        let result = ShellExecuteW(
            None,
            windows::core::w!("open"),
            windows::core::PCWSTR(file.as_ptr()),
            windows::core::PCWSTR(args.as_ptr()),
            windows::core::PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if result.0 as usize <= 32 {
            Err("Windows could not open this system tool.".into())
        } else {
            Ok(())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn activate_tray_icon(tray_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || crate::tray::interact(&tray_id, false))
        .await
        .map_err(|e| e.to_string())?
}
