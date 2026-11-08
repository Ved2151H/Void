use std::collections::HashMap;
use crate::state::{INSTALLED_APPS_CACHE, PROCESS_NAME_CACHE};

/// Derives the package family name (`Name_PublisherId`) from a WindowsApps exe path.
/// Package folders look like `Name_Version_Architecture__PublisherId`.
pub(crate) fn package_family_from_windows_apps_path(path: &str) -> Option<String> {
    let lower = path.to_lowercase();
    let idx = lower.find("\\windowsapps\\")?;
    let rest = &path[idx + "\\windowsapps\\".len()..];
    let folder = rest.split(['\\', '/']).next()?;
    let (before_publisher, publisher) = folder.rsplit_once("__")?;
    let name = before_publisher.split('_').next()?;
    if name.is_empty() || publisher.is_empty() {
        return None;
    }
    Some(format!("{}_{}", name, publisher))
}

/// Full path of a running process, or `None` when it can't be opened.
pub(crate) unsafe fn process_image_path(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let res = QueryFullProcessImageNameW(
        handle,
        PROCESS_NAME_WIN32,
        windows::core::PWSTR(buf.as_mut_ptr()),
        &mut size,
    );
    let _ = CloseHandle(handle);
    res.ok()?;
    Some(String::from_utf16_lossy(&buf[..size as usize]))
}

/// The name an app goes by in the Start menu, from the installed-apps scan:
/// a packaged app by its AppUserModelID or package family, a desktop app by
/// its executable. Programs often leave their own description unset or set it
/// to an internal name ("WhatsApp.Root", "localsend_app"), so this is the
/// preferred label for running and tray apps. `None` when the scan has no
/// single answer, e.g. a browser executable shared by several web apps.
pub(crate) fn installed_app_name(path_or_aumid: &str) -> Option<String> {
    let apps = INSTALLED_APPS_CACHE.get()?.lock().ok()?;
    let unique = |names: Vec<&str>| -> Option<String> {
        let (first, rest) = names.split_first()?;
        rest.iter()
            .all(|n| n.eq_ignore_ascii_case(first))
            .then(|| first.to_string())
    };

    if path_or_aumid.contains('!') && !path_or_aumid.contains('\\') {
        return apps
            .iter()
            .find(|app| app.path.eq_ignore_ascii_case(path_or_aumid))
            .map(|app| app.name.clone());
    }
    if let Some(family) = package_family_from_windows_apps_path(path_or_aumid) {
        let prefix = format!("{}!", family.to_lowercase());
        if let Some(name) = unique(
            apps.iter()
                .filter(|app| app.path.to_lowercase().starts_with(&prefix))
                .map(|app| app.name.as_str())
                .collect(),
        ) {
            return Some(name);
        }
    }
    let exe = std::path::Path::new(path_or_aumid).file_name()?.to_str()?;
    unique(
        apps.iter()
            .filter(|app| {
                app.executable
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(exe))
            })
            .map(|app| app.name.as_str())
            .collect(),
    )
}

/// Label for a running or tray app: its Start menu name when known, else the
/// executable's own description, else its file name.
pub(crate) fn app_display_name(exe_path: &str, aumid: Option<&str>) -> String {
    aumid
        .filter(|id| id.contains('!'))
        .and_then(installed_app_name)
        .or_else(|| installed_app_name(exe_path))
        .unwrap_or_else(|| unsafe { friendly_process_name(exe_path) })
}

/// Friendly name for an executable: the shell's `FileDescription` from version
/// info ("Google Chrome"), falling back to the prettified file stem. Results are
/// cached by path because the mixer polls while it is open.
pub(crate) unsafe fn friendly_process_name(path: &str) -> String {
    let cache = PROCESS_NAME_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(name) = guard.get(path) {
            return name.clone();
        }
    }
    let name = friendly_process_name_uncached(path);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(path.to_string(), name.clone());
    }
    name
}

unsafe fn friendly_process_name_uncached(path: &str) -> String {
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let filename = windows::core::PCWSTR(wide.as_ptr());

    'description: {
        let size = GetFileVersionInfoSizeW(filename, None);
        if size == 0 {
            break 'description;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(filename, None, size, data.as_mut_ptr() as *mut _).is_err() {
            break 'description;
        }

        // FileDescription lives under a language/codepage pair taken from the
        // translation table; the first entry is what Explorer shows.
        let mut trans_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut trans_len = 0u32;
        if !VerQueryValueW(
            data.as_ptr() as *const _,
            windows::core::w!("\\VarFileInfo\\Translation"),
            &mut trans_ptr,
            &mut trans_len,
        )
        .as_bool()
            || trans_len < 4
            || trans_ptr.is_null()
        {
            break 'description;
        }
        // Translation entries are (LANGID, codepage) u16 pairs. Require a full
        // four-byte pair, then read it byte-wise so the pointer can't be
        // dereferenced short or with an alignment assumption.
        let translation = std::slice::from_raw_parts(trans_ptr as *const u8, 4);
        let lang = u16::from_le_bytes([translation[0], translation[1]]);
        let codepage = u16::from_le_bytes([translation[2], translation[3]]);

        let query = format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription");
        let query_wide: Vec<u16> = query.encode_utf16().chain(std::iter::once(0)).collect();
        let mut value_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut value_len = 0u32;
        if !VerQueryValueW(
            data.as_ptr() as *const _,
            windows::core::PCWSTR(query_wide.as_ptr()),
            &mut value_ptr,
            &mut value_len,
        )
        .as_bool()
            || value_len == 0
            || value_ptr.is_null()
        {
            break 'description;
        }

        let chars = std::slice::from_raw_parts(value_ptr as *const u16, value_len as usize);
        let description = String::from_utf16_lossy(chars)
            .trim_end_matches('\0')
            .trim()
            .to_string();
        if !description.is_empty() {
            return description;
        }
    }

    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|stem| {
            let mut chars = stem.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => stem.to_string(),
            }
        })
        .unwrap_or_else(|| "Unknown app".to_string())
}

pub(crate) fn set_radio_state_sync(
    kind: windows::Devices::Radios::RadioKind,
    enabled: bool,
) -> Result<(), String> {
    use windows::Devices::Radios::{Radio, RadioState};
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
    }
    let radios = Radio::GetRadiosAsync()
        .and_then(|op| op.get())
        .map_err(|e| e.to_string())?;
    for i in 0..radios.Size().unwrap_or(0) {
        if let Ok(radio) = radios.GetAt(i) {
            if radio.Kind().ok() == Some(kind) {
                let target = if enabled {
                    RadioState::On
                } else {
                    RadioState::Off
                };
                let _ = radio.SetStateAsync(target).and_then(|op| op.get());
                return Ok(());
            }
        }
    }
    Ok(())
}
