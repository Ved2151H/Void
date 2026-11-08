use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager};
use windows::Win32::Foundation::HWND;
use super::shared::{installed_app_name, package_family_from_windows_apps_path};
use super::windows::open_settings_window;
use crate::state::{ICON_CACHE, INSTALLED_APPS_CACHE, IS_SCANNING, PINNED_FILE_LOCK, START_OPEN_AT_PRESS};
use crate::types::AppInfo;
use crate::utils::{
    get_shortcut_icon_location, icon_to_base64, image_file_to_base64, resolve_executable_path,
    resolve_shortcut,
};

pub(crate) fn get_uwp_launch_cmd(exe_path: &str) -> Option<String> {
    // A packaged app can only be started through its AppUserModelID, and the
    // application part after `!` is whatever the package declares ("Claude",
    // not always "App"). Look the real one up; guess `!App` only if that fails.
    if let Some(family) = package_family_from_windows_apps_path(exe_path) {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        if let Some(aumid) = find_aumid_by_family(&family) {
            return Some(format!("shell:AppsFolder\\{aumid}"));
        }
    }

    let path = std::path::Path::new(exe_path);
    let mut is_windows_apps = false;
    let mut package_folder = String::new();

    for component in path.components() {
        if let std::path::Component::Normal(s) = component {
            let s_str = s.to_string_lossy();
            if is_windows_apps {
                package_folder = s_str.to_string();
                break;
            }
            if s_str.eq_ignore_ascii_case("WindowsApps") {
                is_windows_apps = true;
            }
        }
    }

    if !package_folder.is_empty() {
        // package_folder format: PackageName_Version_Architecture__PublisherId
        // We want: PackageName_PublisherId!App
        if let Some(publisher_idx) = package_folder.rfind("__") {
            let publisher_id = &package_folder[publisher_idx + 2..];
            if let Some(first_underscore) = package_folder.find('_') {
                let package_name = &package_folder[..first_underscore];
                return Some(format!(
                    "shell:AppsFolder\\{}_{}!App",
                    package_name, publisher_id
                ));
            }
        }
    }
    None
}

fn send_key_tap(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, KEYBDINPUT, KEYEVENTF_KEYUP,
    };
    let down = [INPUT {
        r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: Default::default(),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }];
    let up = [INPUT {
        r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }];
    unsafe {
        SendInput(&down, std::mem::size_of::<INPUT>() as i32);
        SendInput(&up, std::mem::size_of::<INPUT>() as i32);
    }
}

/// True while the native Start menu owns the foreground window.
/// Depending on the Windows build it is hosted by StartMenuExperienceHost
/// (older) or SearchHost (Windows 11 24H2+), so match both. The dock is
/// WS_EX_NOACTIVATE, so clicking it never steals focus from an open menu.
pub fn is_start_menu_open() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid == 0 {
            return false;
        }
        if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut buf = [0u16; 260];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(process);
            if ok.is_ok() {
                let path = String::from_utf16_lossy(&buf[..len as usize]);
                let file = path.rsplit('\\').next().unwrap_or("").to_lowercase();
                return file == "startmenuexperiencehost.exe" || file == "searchhost.exe";
            }
        }
        false
    }
}

/// Open Start through Explorer, independently of the foreground app's integrity
/// level. An open menu is dismissed with Escape rather than another open request.
///
/// Start also closes itself on the press of a click outside it, the Start
/// button included, so by the time the click arrives the menu is often gone.
/// The mouse hook records whether Start was open at this click's press: if it
/// was, the press closed it and the click leaves it closed, however long the
/// button was held.
fn toggle_start_menu() {
    // Taken here, so a press is only ever used by the click it started.
    let open_at_press = START_OPEN_AT_PRESS.swap(false, Ordering::Relaxed);

    tauri::async_runtime::spawn_blocking(move || {
        use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
        use windows::Win32::UI::WindowsAndMessaging::{
            FindWindowW, PostMessageW, SC_TASKLIST, WM_SYSCOMMAND,
        };
        if is_start_menu_open() {
            send_key_tap(VK_ESCAPE);
        } else if !open_at_press {
            // Ask Explorer directly. SendInput can be blocked while an elevated
            // app owns the foreground; opening Settings happened to avoid that.
            unsafe {
                if let Ok(tray) = FindWindowW(windows::core::w!("Shell_TrayWnd"), None) {
                    let _ = PostMessageW(
                        Some(tray),
                        WM_SYSCOMMAND,
                        windows::Win32::Foundation::WPARAM(SC_TASKLIST as usize),
                        windows::Win32::Foundation::LPARAM(0),
                    );
                }
            }
        }
    });
}

#[tauri::command]
pub async fn open_app(app: AppHandle, app_name: String) {
    if app_name == "start" {
        toggle_start_menu();
        return;
    }

    if is_bloom_launch_target(&app_name, &app.config().identifier) {
        open_settings_window(app);
        return;
    }

    tauri::async_runtime::spawn_blocking(move || launch_path(&app_name));
}

fn is_bloom_launch_target(target: &str, identifier: &str) -> bool {
    target == "bloom-settings"
        || target.eq_ignore_ascii_case(identifier)
        || std::path::Path::new(target)
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("void.exe"))
}

/// Launches another instance of an app instead of focusing an existing window.
/// Running browser PWAs are relaunched through their Start Menu shortcut so the
/// new window belongs to the web app rather than the bare browser.
#[tauri::command]
pub async fn launch_new_instance(app_path: String, app_name: Option<String>) {
    tauri::async_runtime::spawn_blocking(move || {
        if app_path == "start" {
            return;
        }
        let target = pwa_launch_target(&app_path, app_name.as_deref()).unwrap_or(app_path);
        launch_path(&target);
    });
}

/// Finds the Start Menu shortcut of an installed PWA that matches a running
/// browser-host window: same web app title, hosted by the same browser.
fn pwa_launch_target(path: &str, name: Option<&str>) -> Option<String> {
    let name = name?.trim();
    if name.is_empty() || !is_browser_host_process(path) {
        return None;
    }
    let host_exe = std::path::Path::new(path)
        .file_name()?
        .to_str()?
        .to_lowercase();
    let apps = INSTALLED_APPS_CACHE.get()?.lock().ok()?;
    apps.iter()
        .find(|app| is_pwa_shortcut_for(app, name, &host_exe))
        .map(|app| app.path.clone())
}

fn is_pwa_shortcut_for(app: &AppInfo, name: &str, host_exe: &str) -> bool {
    app.name.eq_ignore_ascii_case(name)
        && app
            .executable
            .as_deref()
            .is_some_and(|exe| exe.eq_ignore_ascii_case(host_exe))
}

/// Opens a file path, shortcut, or shell application id through the shell.
fn launch_path(path: &str) {
    unsafe {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide_open: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();

        // Shortcuts and shell app ids must be launched through the shell itself so
        // that arguments (Chrome/Edge PWAs) and package identities (Store apps) survive.
        let shell_target = if path.to_lowercase().ends_with(".lnk") {
            Some(path.to_string())
        } else if is_aumid_path(path) {
            let id = path.trim().trim_start_matches("shell:AppsFolder\\");
            Some(format!("shell:AppsFolder\\{}", id))
        } else {
            None
        };

        if let Some(target) = shell_target {
            let wide_target: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
            let res = ShellExecuteW(
                None,
                windows::core::PCWSTR(wide_open.as_ptr()),
                windows::core::PCWSTR(wide_target.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            );
            if res.0 as usize <= 32 {
                eprintln!("Failed to open {}: error code {}", target, res.0 as usize);
            }
            return;
        }

        if let Some(uwp_cmd) = crate::commands::get_uwp_launch_cmd(path) {
            let wide_cmd: Vec<u16> = uwp_cmd.encode_utf16().chain(std::iter::once(0)).collect();

            let res = ShellExecuteW(
                None,
                windows::core::PCWSTR(wide_open.as_ptr()),
                windows::core::PCWSTR(wide_cmd.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            );

            if res.0 as usize <= 32 {
                eprintln!(
                    "Failed to open UWP app {}: error code {}",
                    uwp_cmd, res.0 as usize
                );
            }
            return;
        }

        use std::path::Path;
        // AppsFolder entries of classic programs can be saved as a path under a
        // known-folder GUID (`{7C5A40EF-ΓÇª}\\Steam\\Steam.exe`), which
        // ShellExecute cannot open. Resolve it to the real file first.
        let mut final_path = if path.starts_with('{') {
            crate::tray::resolve_path(path)
                .filter(|resolved| Path::new(resolved).exists())
                .unwrap_or_else(|| path.to_string())
        } else {
            path.to_string()
        };

        // An app installed by Squirrel moves to a new version folder when it
        // updates (Discord, Slack, GitHub Desktop).
        if let Some(moved) = crate::utils::moved_squirrel_exe(&final_path) {
            final_path = moved;
        }

        let wide_path: Vec<u16> = final_path
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let wide_open: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();

        let res = ShellExecuteW(
            None,
            windows::core::PCWSTR(wide_open.as_ptr()),
            windows::core::PCWSTR(wide_path.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );

        if res.0 as usize <= 32 {
            eprintln!(
                "Failed to open app {}: error code {}",
                final_path, res.0 as usize
            );
        }
    }
}

fn get_cache_key(path: &str, name: Option<&str>) -> String {
    let path_lc = path.to_lowercase();
    let name_lc = name.map(|n| n.to_lowercase()).unwrap_or_default();
    if path_lc.contains("msedge.exe")
        || path_lc.contains("chrome.exe")
        || path_lc.contains("brave.exe")
        || path_lc.contains("applicationframehost.exe")
    {
        format!("{}:{}", path, name_lc)
    } else {
        path.to_string()
    }
}

fn get_custom_icons_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("custom_icons");
    let _ = std::fs::create_dir_all(&dir);
    Ok(dir)
}

fn sanitize_filename(key: &str) -> String {
    key.replace(
        |c: char| {
            c == ':'
                || c == '\\'
                || c == '/'
                || c == '*'
                || c == '?'
                || c == '"'
                || c == '<'
                || c == '>'
                || c == '|'
        },
        "_",
    )
}

/// Reads a `--flag=value` style argument from a command line, honoring quotes.
fn extract_arg(args: &str, key: &str) -> Option<String> {
    let idx = args.find(key)?;
    let rest = &args[idx + key.len()..];
    let value = if let Some(stripped) = rest.strip_prefix('"') {
        stripped.split('"').next().unwrap_or("")
    } else {
        rest.split_whitespace().next().unwrap_or("")
    };
    let value = value.trim().trim_matches('"');
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// True for shell application ids (`PackageFamily!App`, `Company.Product`, ...)
/// that are not real file system paths.
pub fn is_aumid_path(path: &str) -> bool {
    let p = path.trim().trim_matches('"');
    if p.is_empty() || p.len() < 3 {
        return false;
    }
    if p.to_lowercase().starts_with("shell:appsfolder\\") {
        return true;
    }
    if p.contains('\\') || p.contains('/') || p.contains(':') {
        return false;
    }
    if p.to_lowercase().ends_with(".exe") {
        return false;
    }
    if std::path::Path::new(p).exists() {
        return false;
    }
    true
}

fn is_browser_host_process(path: &str) -> bool {
    let p = path.to_lowercase();
    p.contains("msedge.exe")
        || p.contains("chrome.exe")
        || p.contains("brave.exe")
        || p.contains("vivaldi.exe")
        || p.contains("firefox.exe")
}

/// Resolves the icon of a shell application id (Store/UWP/packaged PWA) through
/// `shell:AppsFolder`. `SHGetFileInfoW` cannot handle these; `IShellItemImageFactory` can.
fn icon_from_aumid(aumid: &str) -> Option<String> {
    unsafe {
        use windows::Win32::Foundation::SIZE;
        use windows::Win32::UI::Shell::{
            IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_BIGGERSIZEOK,
            SIIGBF_ICONONLY,
        };

        let id = aumid.trim().trim_start_matches("shell:AppsFolder\\");
        if id.is_empty() {
            return None;
        }

        let parsing_name = format!("shell:AppsFolder\\{}", id);
        let wide: Vec<u16> = parsing_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(windows::core::PCWSTR(wide.as_ptr()), None).ok()?;

        let size = SIZE { cx: 256, cy: 256 };
        let hbitmap = factory
            .GetImage(size, SIIGBF(SIIGBF_ICONONLY.0 | SIIGBF_BIGGERSIZEOK.0))
            .or_else(|_| factory.GetImage(size, SIIGBF_BIGGERSIZEOK))
            .ok()?;
        if hbitmap.0.is_null() {
            return None;
        }

        let result = crate::utils::hbitmap_to_base64(hbitmap);
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(hbitmap.into());
        result
    }
}

/// Finds the exact AppUserModelID of an installed app from its package family name
/// by enumerating `shell:AppsFolder` (the `!App` suffix is not guaranteed).
fn find_aumid_by_family(family: &str) -> Option<String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_AppsFolder, IEnumIDList, ILCombine, ILFree, IShellFolder, SHGetDesktopFolder,
        SHGetKnownFolderIDList, SHGetNameFromIDList, SHCONTF_FOLDERS, SHCONTF_NONFOLDERS,
        SIGDN_DESKTOPABSOLUTEPARSING,
    };

    unsafe {
        let pidl_apps = SHGetKnownFolderIDList(&FOLDERID_AppsFolder, 0, None).ok()?;
        let result = (|| -> Option<String> {
            let desktop = SHGetDesktopFolder().ok()?;
            let apps_folder: IShellFolder = desktop.BindToObject(pidl_apps, None).ok()?;
            let mut enum_id: Option<IEnumIDList> = None;
            if apps_folder
                .EnumObjects(
                    HWND(std::ptr::null_mut()),
                    (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0) as u32,
                    &mut enum_id,
                )
                .is_err()
            {
                return None;
            }
            let enum_id = enum_id?;

            let prefix = format!("{}!", family);
            let mut pidl_buf: [*mut windows::Win32::UI::Shell::Common::ITEMIDLIST; 1] =
                [std::ptr::null_mut()];
            let mut fetched = 0;
            while enum_id.Next(&mut pidl_buf, Some(&mut fetched)).is_ok() && fetched > 0 {
                let pidl_item = pidl_buf[0];
                pidl_buf[0] = std::ptr::null_mut();
                if pidl_item.is_null() {
                    continue;
                }

                let absolute_pidl =
                    ILCombine(Some(pidl_apps as *const _), Some(pidl_item as *const _));
                if absolute_pidl.is_null() {
                    CoTaskMemFree(Some(pidl_item as *const _));
                    continue;
                }

                let mut found = None;
                if let Ok(p_ptr) = SHGetNameFromIDList(absolute_pidl, SIGDN_DESKTOPABSOLUTEPARSING)
                {
                    let s = String::from_utf16_lossy(windows::core::PCWSTR(p_ptr.0).as_wide());
                    CoTaskMemFree(Some(p_ptr.0 as *const _));
                    if s.starts_with(&prefix) {
                        found = Some(s);
                    }
                }
                ILFree(Some(absolute_pidl as *const _));
                CoTaskMemFree(Some(pidl_item as *const _));
                if found.is_some() {
                    return found;
                }
            }
            None
        })();
        CoTaskMemFree(Some(pidl_apps as *const _));
        result
    }
}

/// Icon for executables that live inside an MSIX package (`\WindowsApps\...`).
/// Those exes carry no app icon; the package manifest logo is the real icon.
fn packaged_exe_icon(path: &str) -> Option<String> {
    let family = package_family_from_windows_apps_path(path)?;
    let aumid = find_aumid_by_family(&family)?;
    icon_from_aumid(&aumid)
}

/// Command line of a process, via WMI. Used to identify PWAs hosted inside browser processes.
fn process_command_line(pid: u32) -> Option<String> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Win32Process {
        command_line: Option<String>,
    }

    let com = wmi::COMLibrary::new().ok()?;
    let connection = wmi::WMIConnection::new(com).ok()?;
    let results: Vec<Win32Process> = connection
        .raw_query(format!(
            "SELECT CommandLine FROM Win32_Process WHERE ProcessId = {}",
            pid
        ))
        .ok()?;
    results.into_iter().next().and_then(|p| p.command_line)
}

fn pwa_icon_from_command_line(process_path: &str, command_line: &str) -> Option<String> {
    // Store PWAs launched by Edge carry their package identity inline.
    if let Some(aumid) = extract_arg(command_line, "--ip-aumid=") {
        if let Some(icon) = icon_from_aumid(&aumid) {
            return Some(icon);
        }
    }
    // Chrome/Edge/Brave "installed app" windows carry the web app id.
    if command_line.contains("--app-id=") {
        if let Some(icon_path) = find_browser_pwa_icon(process_path, command_line) {
            if let Some(icon) = crate::utils::image_file_to_base64(&icon_path) {
                return Some(icon);
            }
        }
    }
    None
}

/// Browser web app ids are 32 characters from the range a-p.
pub(crate) fn browser_web_app_id_from_aumid(id: &str) -> Option<String> {
    let is_web_app_id = |s: &str| s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b));
    if let Some(segment) = id.rsplit('.').next() {
        if is_web_app_id(segment) {
            return Some(segment.to_string());
        }
    }
    if let Some(idx) = id.find("_crx_") {
        if let Some(segment) = id[idx + 5..].split('.').next() {
            if is_web_app_id(segment) {
                return Some(segment.to_string());
            }
        }
    }
    None
}

/// True when a window AppUserModelID belongs to a browser-hosted app (installed
/// PWA or Store PWA) instead of the browser itself. Regular Chromium windows
/// carry only the browser id ("Chrome", "MSEdge", "Brave", ...), while PWA
/// windows carry a web app id or a package AUMID. Browser-generated ids are not
/// always the canonical 32-character form (Brave uses `Brave._crx_<id>` with a
/// shortened id), so the `_crx_` marker is checked directly.
pub(crate) fn is_browser_pwa_aumid(id: &str) -> bool {
    id.contains('!') || id.contains("_crx_") || browser_web_app_id_from_aumid(id).is_some()
}

unsafe fn pwa_icon_for_window(hwnd: HWND, process_path: &str) -> Option<String> {
    // The window's own AppUserModelID is the most reliable identity: it is always
    // present, unlike the process command line (Edge reuses its browser process).
    match crate::utils::get_window_app_user_model_id(hwnd) {
        Some(id) if id.contains('!') => {
            if let Some(icon) = icon_from_aumid(&id) {
                return Some(icon);
            }
        }
        Some(id) if is_browser_pwa_aumid(&id) => {
            // PWA window. The canonical app id is in the AUMID when available,
            // otherwise it must come from the command line. That is safe here
            // because this window is known to be a PWA and the profile lookup
            // requires the `--app-id=` form of the executable's arguments.
            let mut pid = 0u32;
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return None;
            }
            if let Some(app_id) = browser_web_app_id_from_aumid(&id) {
                if let Some(icon_path) =
                    find_browser_pwa_icon(process_path, &format!("--app-id={}", app_id))
                {
                    if let Some(icon) = crate::utils::image_file_to_base64(&icon_path) {
                        return Some(icon);
                    }
                }
            }
            if let Some(command_line) = process_command_line(pid) {
                if let Some(icon) = pwa_icon_from_command_line(process_path, &command_line) {
                    return Some(icon);
                }
            }
            return None;
        }
        // A regular browser window ("Chrome", "MSEdge", "Brave", ...). Chromium can
        // host PWA windows in the same process as the browser, so the process
        // command line may advertise an `--app-id` that does not belong to this
        // window. Do not consult it; let the caller fall back to the window icon.
        Some(_) => return None,
        None => {}
    }
    // Fall back to the process command line for hosts that don't stamp the window.
    let mut pid = 0u32;
    windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return None;
    }
    let command_line = process_command_line(pid)?;
    pwa_icon_from_command_line(process_path, &command_line)
}

/// Finds the icon Chromium stores for an installed web app in the browser profile.
///
/// Current layout: `<profile>/Web Applications/Manifest Resources/<app-id>/Icons/*.png`.
/// Older Chrome builds used `Web Applications/<app-id>` / `_crx_<app-id>` with an `icon_256.png`.
fn find_browser_pwa_icon(executable_path: &str, args: &str) -> Option<String> {
    let app_id = extract_arg(args, "--app-id=")?;
    let local = std::env::var("LOCALAPPDATA").ok()?;

    let chrome_base = format!("{}\\Google\\Chrome\\User Data", local);
    let edge_base = format!("{}\\Microsoft\\Edge\\User Data", local);
    let brave_base = format!("{}\\BraveSoftware\\Brave-Browser\\User Data", local);
    let vivaldi_base = format!("{}\\Vivaldi\\User Data", local);
    let browser_path = executable_path.to_lowercase();
    let bases = if browser_path.contains("msedge") {
        vec![edge_base, chrome_base]
    } else if browser_path.contains("brave") {
        vec![brave_base, chrome_base]
    } else if browser_path.contains("vivaldi") {
        vec![vivaldi_base, chrome_base]
    } else {
        vec![chrome_base, edge_base]
    };

    let profile =
        extract_arg(args, "--profile-directory=").unwrap_or_else(|| "Default".to_string());

    for base in bases {
        if !std::path::Path::new(&base).is_dir() {
            continue;
        }

        let mut profiles = vec![profile.clone()];
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !profiles.iter().any(|p| p.eq_ignore_ascii_case(&name)) {
                        profiles.push(name);
                    }
                }
            }
        }

        for prof in profiles {
            let web_apps = std::path::Path::new(&base)
                .join(&prof)
                .join("Web Applications");
            if !web_apps.is_dir() {
                continue;
            }

            let candidates = [
                web_apps.join("Manifest Resources").join(&app_id),
                web_apps.join(&app_id),
                web_apps.join(format!("_crx_{}", app_id)),
            ];
            for dir in candidates {
                if let Some(icon) = find_largest_image_in_dir(&dir) {
                    return Some(icon);
                }
            }
        }
    }
    None
}

fn find_largest_image_in_dir(dir: &std::path::Path) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    let mut best: Option<(u64, String)> = None;
    collect_images(dir, 0, &mut best);
    best.map(|(_, path)| path)
}

fn collect_images(dir: &std::path::Path, depth: u32, best: &mut Option<(u64, String)>) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_images(&path, depth + 1, best);
            continue;
        }
        let Some(ext) = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
        else {
            continue;
        };
        if !matches!(
            ext.as_str(),
            "png" | "ico" | "jpg" | "jpeg" | "bmp" | "webp"
        ) {
            continue;
        }
        let score = image_size_score(&path);
        let is_better = match best.as_ref() {
            Some((best_score, _)) => score > *best_score,
            None => true,
        };
        if is_better {
            *best = Some((score, path.to_string_lossy().to_string()));
        }
    }
}

/// Scores an icon file by the pixel dimensions encoded in its name (e.g. `256.png`, `192x192.png`).
fn image_size_score(path: &std::path::Path) -> u64 {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut score: u64 = 0;
    for token in name.split(|c: char| !c.is_ascii_digit() && c != 'x') {
        if token.is_empty() {
            continue;
        }
        if let Some(idx) = token.find('x') {
            let (w, h) = token.split_at(idx);
            let h = &h[1..];
            if let (Ok(w), Ok(h)) = (w.parse::<u64>(), h.parse::<u64>()) {
                score = score.max(w * h);
            }
        } else if let Ok(n) = token.parse::<u64>() {
            score = score.max(n * n);
        }
    }
    score
}

static LAST_ICON_SAVE_REQUEST: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn schedule_icon_cache_save(app: AppHandle) {
    let now = crate::utils::get_now_ms();
    LAST_ICON_SAVE_REQUEST.store(now, Ordering::Relaxed);

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        let last = LAST_ICON_SAVE_REQUEST.load(Ordering::Relaxed);
        if now == last || crate::utils::get_now_ms() - last >= 950 {
            if let Ok(dir) = app.path().app_config_dir() {
                let cache_path = dir.join("icons_cache.json");
                if let Some(c) = crate::state::ICON_CACHE.get() {
                    let snapshot = if let Ok(lock) = c.lock() {
                        serde_json::to_string(&*lock).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    if !snapshot.is_empty() {
                        let _ = tauri::async_runtime::spawn_blocking(move || {
                            let _ = std::fs::write(&cache_path, snapshot);
                        })
                        .await;
                    }
                }
            }
        }
    });
}

#[tauri::command]
pub async fn get_app_icon(
    app: AppHandle,
    path: String,
    name: Option<String>,
    hwnd: Option<isize>,
) -> Result<Option<String>, String> {
    let cache_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let cache_path = cache_dir.join("icons_cache.json");
    let cache_key = get_cache_key(&path, name.as_deref());

    // Strategy 0: Check for custom icon first
    let custom_icons_dir = cache_dir.join("custom_icons");
    let custom_file = custom_icons_dir.join(format!("{}.png", sanitize_filename(&cache_key)));
    if custom_file.exists() {
        if let Ok(data) = std::fs::read(&custom_file) {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
            return Ok(Some(format!("data:image/png;base64,{}", b64)));
        }
    }

    let cache = ICON_CACHE.get_or_init(|| {
        let mut map = std::collections::HashMap::new();
        if let Ok(content) = std::fs::read_to_string(&cache_path) {
            if let Ok(existing) =
                serde_json::from_str::<std::collections::HashMap<String, String>>(&content)
            {
                map = existing;
            }
        }
        std::sync::Mutex::new(map)
    });

    // Strategy 1: Check persistent in-memory cache
    if let Ok(c) = cache.lock() {
        if let Some(icon) = c.get(&cache_key) {
            return Ok(Some(icon.clone()));
        }
    }

    // Bare executable names (default pins such as "notepad.exe") are resolved first,
    // so a Store-package resolution can use the package logo instead of a System32 stub.
    let mut path = path;
    if !path.contains('\\') && !path.contains('/') {
        if let Some(resolved) = resolve_executable_path(&path) {
            path = resolved;
        }
    }
    // A pin to a Squirrel app (Discord, Slack) can point at a version folder
    // its updater has since deleted. The icon was cached under that path, so
    // this only shows once the cache is cleared; take the current version's.
    if let Some(moved) = crate::utils::moved_squirrel_exe(&path) {
        path = moved;
    }

    // Strategy 2: Shell-resolved app icons (Store/UWP/PWA packages) and executables
    // inside an MSIX package. Neither has a usable icon on disk; the shell resolves
    // the package logo through `shell:AppsFolder`.
    let aumid_opt = if is_aumid_path(&path) {
        Some(
            path.trim()
                .trim_start_matches("shell:AppsFolder\\")
                .to_string(),
        )
    } else {
        None
    };
    let packaged_exe = path.to_lowercase().contains("\\windowsapps\\");
    if aumid_opt.is_some() || packaged_exe {
        let path_owned = path.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let _ = unsafe {
                windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_MULTITHREADED,
                )
            };
            let icon = match aumid_opt {
                Some(aumid) => icon_from_aumid(&aumid),
                None => packaged_exe_icon(&path_owned),
            };
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
            icon
        })
        .await
        .unwrap_or(None);

        if let Some(base64) = result {
            if let Ok(mut c) = cache.lock() {
                c.insert(cache_key.clone(), base64.clone());
            }
            schedule_icon_cache_save(app);
            return Ok(Some(base64));
        }
    }

    // Strategy 3: Extract icon from live window HWND
    if let Some(h) = hwnd {
        let path_owned = path.clone();
        let result = tauri::async_runtime::spawn_blocking(move || unsafe {
            use windows::Win32::System::Com::{
                CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED,
            };
            use windows::Win32::UI::WindowsAndMessaging::{
                GetClassLongPtrW, SendMessageTimeoutW, GCLP_HICON, ICON_BIG, SMTO_ABORTIFHUNG,
                WM_GETICON,
            };

            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            // Browser-hosted PWAs (Store apps, installed web apps) only expose the
            // browser icon on the window itself; resolve the app identity from the
            // window's AppUserModelID (or the process command line as fallback).
            if is_browser_host_process(&path_owned) {
                if let Some(icon) = pwa_icon_for_window(HWND(h as *mut _), &path_owned) {
                    CoUninitialize();
                    return Some(icon);
                }
            }

            let h_hwnd = HWND(h as *mut _);

            let mut h_icon = windows::Win32::UI::WindowsAndMessaging::HICON(GetClassLongPtrW(
                h_hwnd, GCLP_HICON,
            )
                as *mut _);
            if h_icon.is_invalid() {
                h_icon = windows::Win32::UI::WindowsAndMessaging::HICON(GetClassLongPtrW(
                    h_hwnd,
                    windows::Win32::UI::WindowsAndMessaging::GCL_HICON,
                )
                    as *mut _);
            }

            if h_icon.is_invalid() {
                let mut res = 0usize;
                let _ = SendMessageTimeoutW(
                    h_hwnd,
                    WM_GETICON,
                    windows::Win32::Foundation::WPARAM(ICON_BIG as usize),
                    windows::Win32::Foundation::LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    250,
                    Some(&mut res),
                );
                if res != 0 {
                    h_icon = windows::Win32::UI::WindowsAndMessaging::HICON(res as *mut _);
                }
            }

            let res = if !h_icon.is_invalid() {
                icon_to_base64(h_icon)
            } else {
                None
            };

            CoUninitialize();
            res
        })
        .await
        .unwrap_or(None);

        if let Some(base64) = result {
            if let Ok(mut c) = cache.lock() {
                c.insert(cache_key.clone(), base64.clone());
            }
            schedule_icon_cache_save(app);
            return Ok(Some(base64));
        }
    }

    // Strategy 4: Extract icon from file path
    let path_clone = path.clone();
    let ck_clone = cache_key.clone();
    let file_icon = tauri::async_runtime::spawn_blocking(move || unsafe {
        use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
        use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
        use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;

        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = {
            let ((actual_path, args), is_lnk) = if path_clone.to_lowercase().ends_with(".lnk") {
                (
                    resolve_shortcut(&path_clone).unwrap_or((path_clone.clone(), String::new())),
                    true,
                )
            } else {
                ((path_clone.clone(), String::new()), false)
            };

            // A shortcut can carry its own icon file (Firefox web apps, user edits).
            let mut icon_data = if is_lnk {
                get_shortcut_icon_location(&path_clone).and_then(|p| image_file_to_base64(&p))
            } else {
                None
            };

            // Chrome/Edge/Brave installed web apps keep the site icon in the browser profile.
            if icon_data.is_none() {
                if let Some(pwa_icon_path) = find_browser_pwa_icon(&actual_path, &args) {
                    icon_data = image_file_to_base64(&pwa_icon_path);
                }
            }

            // UWP shortcuts launch `explorer.exe shell:AppsFolder\<AUMID>`.
            if icon_data.is_none() {
                if let Some(aumid) = extract_arg(&args, "shell:AppsFolder\\") {
                    icon_data = icon_from_aumid(&aumid);
                }
            }

            if icon_data.is_none() {
                let mut shfi: SHFILEINFOW = std::mem::zeroed();
                let path_u16: Vec<u16> = actual_path
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                let res = SHGetFileInfoW(
                    windows::core::PCWSTR(path_u16.as_ptr()),
                    Default::default(),
                    Some(&mut shfi),
                    std::mem::size_of::<SHFILEINFOW>() as u32,
                    SHGFI_ICON | SHGFI_LARGEICON,
                );

                if res != 0 && !shfi.hIcon.is_invalid() {
                    icon_data = icon_to_base64(shfi.hIcon);
                    let _ = DestroyIcon(shfi.hIcon);
                }
            }

            if let Some(ref base64) = icon_data {
                if let Ok(mut lock) = ICON_CACHE.get().unwrap().lock() {
                    lock.insert(ck_clone, base64.clone());
                }
            }
            icon_data
        };
        CoUninitialize();
        result
    })
    .await
    .map_err(|e| e.to_string())?;

    if file_icon.is_some() {
        schedule_icon_cache_save(app);
    }
    Ok(file_icon)
}

#[tauri::command]
pub fn save_pinned_apps(app: AppHandle, apps: Vec<AppInfo>) -> Result<(), String> {
    let path = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("pinned_apps.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let _guard = PINNED_FILE_LOCK.lock().map_err(|e| e.to_string())?;
    if path.exists() {
        let old = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str::<Vec<AppInfo>>(&old)
            .map_err(|e| format!("Pinned apps file is invalid; refusing to overwrite it: {e}"))?;
    }
    let content = serde_json::to_string(&apps).map_err(|e| e.to_string())?;
    crate::utils::atomic_write_file(&path, content.as_bytes())?;
    Ok(())
}

/// Apps pinned while running were saved under their executable's name
/// ("WhatsApp.Root", "localsend_app"). Gives those their Start menu name, once
/// the installed-apps scan has results: the dock loads its pins again when the
/// scan emits `installed-apps-updated`, then saves the corrected list. Names
/// that differ from the file name are kept as they are.
fn refresh_pinned_names(mut apps: Vec<AppInfo>) -> Vec<AppInfo> {
    for app in &mut apps {
        let stem = std::path::Path::new(&app.path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if stem.is_empty() || !app.name.eq_ignore_ascii_case(stem) {
            continue;
        }
        if let Some(name) = installed_app_name(&app.path) {
            app.name = name;
        }
    }
    apps
}

#[tauri::command]
pub async fn load_pinned_apps(app: AppHandle) -> Vec<AppInfo> {
    let path = app
        .path()
        .app_config_dir()
        .unwrap_or_default()
        .join("pinned_apps.json");
    if let Ok(content) = std::fs::read_to_string(path) {
        if let Ok(apps) = serde_json::from_str::<Vec<AppInfo>>(&content) {
            let apps: Vec<AppInfo> = apps
                .into_iter()
                .filter(|a| a.path != "bloom-settings")
                .collect();
            return tauri::async_runtime::spawn_blocking(move || refresh_pinned_names(apps))
                .await
                .unwrap_or_default();
        }
    }
    vec![
        AppInfo {
            name: "File Explorer".into(),
            path: "C:\\Windows\\explorer.exe".into(),
            icon: None,
            is_running: false,
            hwnd: None,
            executable: Some("explorer.exe".into()),
            all_hwnds: None,
        },
        AppInfo {
            name: "Microsoft Edge".into(),
            path: "msedge".into(),
            icon: None,
            is_running: false,
            hwnd: None,
            executable: Some("msedge.exe".into()),
            all_hwnds: None,
        },
        AppInfo {
            name: "Notepad".into(),
            path: "notepad.exe".into(),
            icon: None,
            is_running: false,
            hwnd: None,
            executable: Some("notepad.exe".into()),
            all_hwnds: None,
        },
    ]
}

#[tauri::command]
pub async fn clear_icon_cache(app: AppHandle) -> Result<(), String> {
    let cache_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let cache_path = cache_dir.join("icons_cache.json");
    let _ = std::fs::remove_file(&cache_path);
    if let Some(c) = ICON_CACHE.get() {
        if let Ok(mut lock) = c.lock() {
            lock.clear();
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn set_custom_icon(
    app: AppHandle,
    cache_key: String,
    icon_data: String,
) -> Result<String, String> {
    use base64::Engine;
    use image::GenericImageView;

    // Validate and decode the base64 data URI
    let b64_str = if icon_data.starts_with("data:") {
        icon_data.split(',').nth(1).ok_or("Invalid data URI")?
    } else {
        &icon_data
    };

    let raw_bytes = base64::engine::general_purpose::STANDARD
        .decode(b64_str)
        .map_err(|e| format!("Invalid base64: {}", e))?;

    // Decode image to validate and get dimensions
    let img = image::load_from_memory(&raw_bytes).map_err(|e| format!("Invalid image: {}", e))?;

    let (w, h) = img.dimensions();
    if w < 16 || h < 16 {
        return Err("Icon must be at least 16x16 pixels".into());
    }

    // Resize to fit within 256x256 preserving aspect ratio, then center on transparent canvas
    let target = 256u32;
    let final_img = if w > target || h > target {
        let scaled = img.resize(target, target, image::imageops::FilterType::Lanczos3);
        let (sw, sh) = scaled.dimensions();
        let mut canvas = image::RgbaImage::new(target, target);
        let ox = (target - sw) / 2;
        let oy = (target - sh) / 2;
        image::imageops::overlay(&mut canvas, &scaled, ox as i64, oy as i64);
        image::DynamicImage::ImageRgba8(canvas)
    } else {
        img
    };

    // Encode to PNG bytes
    let mut png_bytes: Vec<u8> = Vec::new();
    final_img
        .write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("Failed to encode PNG: {}", e))?;

    // Save to custom_icons directory
    let icons_dir = get_custom_icons_dir(&app)?;
    let filename = format!("{}.png", sanitize_filename(&cache_key));
    let file_path = icons_dir.join(&filename);
    std::fs::write(&file_path, &png_bytes).map_err(|e| e.to_string())?;

    // Update manifest mapping sanitized filename -> original cache key
    let manifest_path = icons_dir.join("custom_icons.json");
    let mut manifest: std::collections::HashMap<String, String> =
        if let Ok(content) = std::fs::read_to_string(&manifest_path) {
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };
    manifest.insert(sanitize_filename(&cache_key), cache_key);
    let _ = std::fs::write(
        &manifest_path,
        serde_json::to_string(&manifest).unwrap_or_default(),
    );

    // Return the data URI for immediate use
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Ok(format!("data:image/png;base64,{}", b64))
}

#[tauri::command]
pub async fn remove_custom_icon(
    app: AppHandle,
    path: String,
    name: Option<String>,
) -> Result<(), String> {
    let cache_key = get_cache_key(&path, name.as_deref());
    let icons_dir = get_custom_icons_dir(&app)?;
    let sanitized = sanitize_filename(&cache_key);
    let filename = format!("{}.png", sanitized);
    let file_path = icons_dir.join(&filename);
    if file_path.exists() {
        std::fs::remove_file(&file_path).map_err(|e| e.to_string())?;
    }
    // Remove from manifest
    let manifest_path = icons_dir.join("custom_icons.json");
    if let Ok(content) = std::fs::read_to_string(&manifest_path) {
        if let Ok(mut manifest) =
            serde_json::from_str::<std::collections::HashMap<String, String>>(&content)
        {
            manifest.remove(&sanitized);
            let _ = std::fs::write(
                &manifest_path,
                serde_json::to_string(&manifest).unwrap_or_default(),
            );
        }
    }
    // Also remove from persistent cache so the original icon is re-fetched
    if let Some(c) = ICON_CACHE.get() {
        if let Ok(mut lock) = c.lock() {
            lock.remove(&cache_key);
        }
    }
    schedule_icon_cache_save(app);
    Ok(())
}

#[tauri::command]
pub async fn get_custom_icons(app: AppHandle) -> Result<HashMap<String, String>, String> {
    let icons_dir = get_custom_icons_dir(&app)?;
    let manifest_path = icons_dir.join("custom_icons.json");
    let manifest: std::collections::HashMap<String, String> =
        if let Ok(content) = std::fs::read_to_string(&manifest_path) {
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };
    let mut result = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(&icons_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("png") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    // Use original cache key from manifest, fallback to sanitized stem
                    let key = manifest
                        .get(stem)
                        .cloned()
                        .unwrap_or_else(|| stem.to_string());
                    if let Ok(data) = std::fs::read(&path) {
                        use base64::Engine;
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                        result.insert(key, format!("data:image/png;base64,{}", b64));
                    }
                }
            }
        }
    }
    Ok(result)
}

#[tauri::command]
pub async fn get_installed_apps() -> Vec<AppInfo> {
    let cache = INSTALLED_APPS_CACHE.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    let is_empty = if let Ok(lock) = cache.lock() {
        lock.is_empty()
    } else {
        true
    };
    if is_empty && !IS_SCANNING.load(Ordering::Relaxed) {
        crate::services::trigger_app_scan();
    }
    let start = std::time::Instant::now();
    while IS_SCANNING.load(Ordering::Relaxed) && start.elapsed() < std::time::Duration::from_secs(5)
    {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if let Ok(cache_lock) = cache.lock() {
        cache_lock.clone()
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod pwa_icon_tests {
    use super::*;

    #[test]
    fn bloom_launch_targets_reuse_the_running_settings_window() {
        let identifier = "com.sehaz.bloom";
        assert!(is_bloom_launch_target(identifier, identifier));
        assert!(is_bloom_launch_target("bloom-settings", identifier));
        assert!(is_bloom_launch_target(
            "C:\\Users\\test\\AppData\\Local\\Void\\Void.exe",
            identifier
        ));
        assert!(!is_bloom_launch_target("C:\\Apps\\Discord.exe", identifier));
        assert!(!is_bloom_launch_target("com.other.app", identifier));
    }

    #[test]
    fn aumid_detection() {
        assert!(is_aumid_path("4DF9E0F8.Netflix_mcm4njqhnhss8!Netflix.App"));
        assert!(is_aumid_path("Microsoft.VisualStudioCode"));
        assert!(is_aumid_path(
            "shell:AppsFolder\\4DF9E0F8.Netflix_mcm4njqhnhss8!Netflix.App"
        ));
        assert!(!is_aumid_path("C:\\Windows\\explorer.exe"));
        assert!(!is_aumid_path("msedge.exe"));
        assert!(!is_aumid_path(""));
    }

    #[test]
    fn command_line_arg_parsing() {
        let args = "--profile-directory=\"Profile 1\" --app-id=abcdef --ip-aumid=Package_Pub!App";
        assert_eq!(extract_arg(args, "--app-id="), Some("abcdef".into()));
        assert_eq!(
            extract_arg(args, "--profile-directory="),
            Some("Profile 1".into())
        );
        assert_eq!(
            extract_arg(args, "--ip-aumid="),
            Some("Package_Pub!App".into())
        );
        assert_eq!(extract_arg(args, "--missing="), None);
        assert_eq!(
            extract_arg(
                "shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
                "shell:AppsFolder\\"
            ),
            Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into())
        );
    }

    #[test]
    fn package_family_from_paths() {
        assert_eq!(
            package_family_from_windows_apps_path("C:\\Program Files\\WindowsApps\\5319275A.WhatsAppDesktop_2.2634.101.0_x64__cv1g1gvanyjgm\\WhatsApp.Root.exe").as_deref(),
            Some("5319275A.WhatsAppDesktop_cv1g1gvanyjgm")
        );
        assert_eq!(
            package_family_from_windows_apps_path("C:\\Windows\\explorer.exe"),
            None
        );
        assert_eq!(
            package_family_from_windows_apps_path("C:\\Program Files\\App\\app.exe"),
            None
        );
    }

    #[test]
    fn browser_web_app_ids_from_aumids() {
        assert_eq!(
            browser_web_app_id_from_aumid("Chrome.edhbnieanoeijlkpgkminebadpibapgm").as_deref(),
            Some("edhbnieanoeijlkpgkminebadpibapgm")
        );
        assert_eq!(
            browser_web_app_id_from_aumid("Chrome._crx_edhbnieanoeijlkpgkminebadpibapgm")
                .as_deref(),
            Some("edhbnieanoeijlkpgkminebadpibapgm")
        );
        assert_eq!(
            browser_web_app_id_from_aumid("4DF9E0F8.Netflix_mcm4njqhnhss8!Netflix.App"),
            None
        );
        assert_eq!(browser_web_app_id_from_aumid("MSEdge"), None);
    }

    #[test]
    fn browser_pwa_aumid_detection() {
        assert!(is_browser_pwa_aumid(
            "Chrome.edhbnieanoeijlkpgkminebadpibapgm"
        ));
        assert!(is_browser_pwa_aumid(
            "Brave._crx_edhbnieanoeijlkpgkminebadpibapgm"
        ));
        assert!(is_browser_pwa_aumid(
            "Brave._crx_agimnkijcamfeangaknmldooml"
        ));
        assert!(is_browser_pwa_aumid(
            "4DF9E0F8.Netflix_mcm4njqhnhss8!Netflix.App"
        ));
        assert!(!is_browser_pwa_aumid("MSEdge"));
        assert!(!is_browser_pwa_aumid("Chrome"));
        assert!(!is_browser_pwa_aumid("Brave"));
    }

    #[test]
    fn image_size_scoring_prefers_larger_dimensions() {
        use std::path::Path;
        assert!(
            image_size_score(Path::new("C:\\x\\Icons\\256.png"))
                > image_size_score(Path::new("C:\\x\\Icons\\64.png"))
        );
        assert!(
            image_size_score(Path::new("C:\\x\\512x512.png"))
                > image_size_score(Path::new("C:\\x\\192x192.png"))
        );
        assert_eq!(image_size_score(Path::new("C:\\x\\icon.png")), 0);
    }

    #[test]
    fn pwa_shortcut_matching() {
        let app = AppInfo {
            name: "YouTube".into(),
            path: "C:\\Users\\x\\Start Menu\\Programs\\YouTube.lnk".into(),
            icon: None,
            is_running: false,
            hwnd: None,
            executable: Some("brave.exe".into()),
            all_hwnds: None,
        };
        assert!(is_pwa_shortcut_for(&app, "youtube", "brave.exe"));
        assert!(is_pwa_shortcut_for(&app, "YouTube", "BRAVE.EXE"));
        assert!(!is_pwa_shortcut_for(&app, "YouTube", "msedge.exe"));
        assert!(!is_pwa_shortcut_for(&app, "YouTube Music", "brave.exe"));
    }

    #[test]
    fn pwa_launch_target_resolution() {
        // Non-browser paths and missing titles never resolve through the cache.
        assert_eq!(
            pwa_launch_target("C:\\Windows\\notepad.exe", Some("Notepad")),
            None
        );
        assert_eq!(
            pwa_launch_target(
                "C:\\Program Files\\BraveSoftware\\Brave-Browser\\Application\\brave.exe",
                None
            ),
            None
        );
        assert_eq!(
            pwa_launch_target(
                "C:\\Program Files\\BraveSoftware\\Brave-Browser\\Application\\brave.exe",
                Some("   ")
            ),
            None
        );

        // With the installed-apps cache populated, a running PWA resolves to its
        // Start Menu shortcut and a same-titled app under another browser does not.
        let _ = INSTALLED_APPS_CACHE.set(std::sync::Mutex::new(vec![
            AppInfo {
                name: "YouTube".into(),
                path: "C:\\Start Menu\\YouTube.lnk".into(),
                icon: None,
                is_running: false,
                hwnd: None,
                executable: Some("brave.exe".into()),
                all_hwnds: None,
            },
            AppInfo {
                name: "Netflix".into(),
                path: "C:\\Start Menu\\Netflix.lnk".into(),
                icon: None,
                is_running: false,
                hwnd: None,
                executable: Some("msedge.exe".into()),
                all_hwnds: None,
            },
        ]));
        assert_eq!(
            pwa_launch_target(
                "C:\\Program Files\\BraveSoftware\\Brave-Browser\\Application\\brave.exe",
                Some("YouTube")
            ),
            Some("C:\\Start Menu\\YouTube.lnk".into())
        );
        assert_eq!(
            pwa_launch_target(
                "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
                Some("YouTube")
            ),
            None
        );
        assert_eq!(
            pwa_launch_target(
                "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
                Some("Netflix")
            ),
            Some("C:\\Start Menu\\Netflix.lnk".into())
        );
    }
}
