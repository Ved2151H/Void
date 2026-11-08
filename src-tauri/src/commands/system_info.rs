// --- System metrics for status widgets ---

#[tauri::command]
pub fn get_cpu_usage() -> Result<u32, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_IDLE: AtomicU64 = AtomicU64::new(0);
    static LAST_KERNEL: AtomicU64 = AtomicU64::new(0);
    static LAST_USER: AtomicU64 = AtomicU64::new(0);

    unsafe {
        let mut idle = windows::Win32::Foundation::FILETIME::default();
        let mut kernel = windows::Win32::Foundation::FILETIME::default();
        let mut user = windows::Win32::Foundation::FILETIME::default();
        windows::Win32::System::Threading::GetSystemTimes(
            Some(&mut idle),
            Some(&mut kernel),
            Some(&mut user),
        )
        .map_err(|e| format!("GetSystemTimes failed: {e}"))?;

        let idle_u64 = (idle.dwHighDateTime as u64) << 32 | idle.dwLowDateTime as u64;
        let kernel_u64 = (kernel.dwHighDateTime as u64) << 32 | kernel.dwLowDateTime as u64;
        let user_u64 = (user.dwHighDateTime as u64) << 32 | user.dwLowDateTime as u64;

        let prev_idle = LAST_IDLE.swap(idle_u64, Ordering::Relaxed);
        let prev_kernel = LAST_KERNEL.swap(kernel_u64, Ordering::Relaxed);
        let prev_user = LAST_USER.swap(user_u64, Ordering::Relaxed);

        let idle_delta = idle_u64.saturating_sub(prev_idle);
        let kernel_delta = kernel_u64.saturating_sub(prev_kernel);
        let user_delta = user_u64.saturating_sub(prev_user);
        let total = kernel_delta + user_delta;

        if total == 0 {
            return Ok(0);
        }
        let usage = ((total - idle_delta) * 100) / total;
        Ok(usage as u32)
    }
}

#[tauri::command]
pub fn get_ram_usage() -> Result<f32, String> {
    unsafe {
        let mut mem = windows::Win32::System::SystemInformation::MEMORYSTATUSEX::default();
        mem.dwLength =
            std::mem::size_of::<windows::Win32::System::SystemInformation::MEMORYSTATUSEX>() as u32;
        windows::Win32::System::SystemInformation::GlobalMemoryStatusEx(&mut mem)
            .map_err(|e| format!("GlobalMemoryStatusEx failed: {e}"))?;
        Ok(mem.dwMemoryLoad as f32)
    }
}

/// Whether the microphone and/or camera is in use right now, read from the
/// Windows capability consent store (see `privacy`).
#[tauri::command]
pub fn get_media_usage() -> crate::privacy::MediaUsage {
    crate::privacy::media_usage()
}

/// Free and total space on the system drive, plus the used share, for the
/// status widget and the System panel's disk meter.
#[derive(serde::Serialize)]
pub struct DiskUsage {
    pub free_gb: f64,
    pub total_gb: f64,
    pub used_percent: f32,
}

#[tauri::command]
pub fn get_disk_space() -> Result<DiskUsage, String> {
    unsafe {
        let mut free_bytes = 0u64;
        let mut total_bytes = 0u64;
        let mut total_free = 0u64;
        windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            windows::core::w!("C:\\"),
            Some(&mut free_bytes),
            Some(&mut total_bytes),
            Some(&mut total_free),
        )
        .map_err(|e| format!("GetDiskFreeSpaceEx failed: {e}"))?;
        let gb = 1024.0 * 1024.0 * 1024.0;
        Ok(DiskUsage {
            free_gb: free_bytes as f64 / gb,
            total_gb: total_bytes as f64 / gb,
            used_percent: if total_bytes > 0 {
                ((total_bytes.saturating_sub(free_bytes)) as f64 / total_bytes as f64 * 100.0)
                    as f32
            } else {
                0.0
            },
        })
    }
}

#[tauri::command]
pub fn get_windows_accent_color() -> Option<String> {
    unsafe {
        use windows::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_READ, REG_BINARY,
            REG_DWORD,
        };

        // 1. Try reading AccentPalette from Explorer\Accent for the true system accent color
        let subkey =
            windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent");
        let mut hkey = windows::Win32::System::Registry::HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey, Some(0), KEY_READ, &mut hkey).is_ok() {
            let mut value_type = REG_BINARY;
            let mut palette = [0u8; 32];
            let mut value_size = 32u32;
            let val_name = windows::core::w!("AccentPalette");
            if RegQueryValueExW(
                hkey,
                val_name,
                None,
                Some(&mut value_type),
                Some(palette.as_mut_ptr()),
                Some(&mut value_size),
            )
            .is_ok()
            {
                let _ = RegCloseKey(hkey);
                if value_size >= 15 {
                    // bytes 12, 13, 14 are R, G, B of the active accent color
                    let r = palette[12];
                    let g = palette[13];
                    let b = palette[14];
                    return Some(format!("#{:02x}{:02x}{:02x}", r, g, b));
                }
            } else {
                let _ = RegCloseKey(hkey);
            }
        }

        // 2. Fallback to DWM\AccentColor
        let subkey = windows::core::w!("Software\\Microsoft\\Windows\\DWM");
        let mut hkey = windows::Win32::System::Registry::HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey, Some(0), KEY_READ, &mut hkey).is_ok() {
            let mut value_type = REG_DWORD;
            let mut color_val = 0u32;
            let mut value_size = std::mem::size_of::<u32>() as u32;
            let val_name = windows::core::w!("AccentColor");
            if RegQueryValueExW(
                hkey,
                val_name,
                None,
                Some(&mut value_type),
                Some(&mut color_val as *mut u32 as *mut u8),
                Some(&mut value_size),
            )
            .is_ok()
            {
                let _ = RegCloseKey(hkey);
                // color_val is AABBGGRR (ABGR format)
                let r = (color_val & 0xff) as u8;
                let g = ((color_val >> 8) & 0xff) as u8;
                let b = ((color_val >> 16) & 0xff) as u8;
                return Some(format!("#{:02x}{:02x}{:02x}", r, g, b));
            }
            let _ = RegCloseKey(hkey);
        }
    }
    None
}

#[tauri::command]
pub fn get_system_accent_color() -> Result<String, String> {
    // Try reading registry first for exact Windows accent color
    if let Some(color) = get_windows_accent_color() {
        return Ok(color);
    }

    // Fallback to DwmGetColorizationColor
    unsafe {
        let mut color = 0u32;
        let mut opaque = windows::core::BOOL(0);
        if windows::Win32::Graphics::Dwm::DwmGetColorizationColor(&mut color, &mut opaque).is_ok() {
            let r = ((color >> 16) & 0xff) as u8;
            let g = ((color >> 8) & 0xff) as u8;
            let b = (color & 0xff) as u8;
            let hex = format!("#{:02x}{:02x}{:02x}", r, g, b);
            Ok(hex)
        } else {
            Err("Failed to query colorization color".into())
        }
    }
}
