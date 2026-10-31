//! Whether the microphone or camera is currently in use.
//!
//! Windows records every app that opens a protected device in
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\
//! ConsentStore\<capability>`. Non-packaged apps get one subkey per executable
//! under `NonPackaged`, packaged apps one subkey per package family. While an
//! app holds the device `LastUsedTimeStop` is zero; it is stamped when the app
//! releases it. Only the registry is read - nothing is installed or hooked.

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    REG_VALUE_TYPE,
};

const CONSENT_STORE: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\";

#[derive(Clone, Copy, serde::Serialize)]
pub struct MediaUsage {
    pub microphone: bool,
    pub camera: bool,
}

pub fn media_usage() -> MediaUsage {
    MediaUsage {
        microphone: capability_in_use("microphone"),
        camera: capability_in_use("webcam"),
    }
}

/// True when any app under `capability` ("microphone" or "webcam") is holding
/// the device right now.
pub fn capability_in_use(capability: &str) -> bool {
    unsafe {
        let Some(root) = open_key(HKEY_CURRENT_USER, &format!("{CONSENT_STORE}{capability}")) else {
            return false;
        };
        let mut active = false;
        for name in subkey_names(root) {
            if active {
                break;
            }
            if name.eq_ignore_ascii_case("NonPackaged") {
                let Some(non_packaged) = open_key(root, &name) else {
                    continue;
                };
                for app in subkey_names(non_packaged) {
                    if let Some(key) = open_key(non_packaged, &app) {
                        active = entry_active(key);
                        let _ = RegCloseKey(key);
                        if active {
                            break;
                        }
                    }
                }
                let _ = RegCloseKey(non_packaged);
            } else if let Some(key) = open_key(root, &name) {
                active = entry_active(key);
                let _ = RegCloseKey(key);
            }
        }
        let _ = RegCloseKey(root);
        active
    }
}

unsafe fn open_key(parent: HKEY, name: &str) -> Option<HKEY> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut key = HKEY::default();
    let status = RegOpenKeyExW(parent, PCWSTR(wide.as_ptr()), None, KEY_READ, &mut key);
    (status == ERROR_SUCCESS).then_some(key)
}

unsafe fn subkey_names(key: HKEY) -> Vec<String> {
    let mut names = Vec::new();
    let mut index = 0;
    loop {
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let status = RegEnumKeyExW(
            key,
            index,
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
            None,
            None,
            None,
            None,
        );
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&buf[..len as usize]));
        index += 1;
    }
    names
}

unsafe fn entry_active(key: HKEY) -> bool {
    let start = read_qword(key, "LastUsedTimeStart").unwrap_or(0);
    let stop = read_qword(key, "LastUsedTimeStop").unwrap_or(0);
    start != 0 && stop == 0
}

unsafe fn read_qword(key: HKEY, name: &str) -> Option<u64> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut ty = REG_VALUE_TYPE::default();
    let mut data = [0u8; 8];
    let mut size = data.len() as u32;
    let status = RegQueryValueExW(
        key,
        PCWSTR(wide.as_ptr()),
        None,
        Some(&mut ty),
        Some(data.as_mut_ptr()),
        Some(&mut size),
    );
    if status == ERROR_SUCCESS && size >= 8 {
        Some(u64::from_le_bytes(data))
    } else if status == ERROR_SUCCESS && size >= 4 {
        Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as u64)
    } else {
        None
    }
}
