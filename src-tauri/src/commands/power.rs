use std::sync::atomic::Ordering;
use super::shared::set_radio_state_sync;
use tauri::{AppHandle, Emitter};
use crate::types::BrightnessChangeEvent;

#[tauri::command]
pub fn get_brightness() -> u32 {
    crate::state::CURRENT_BRIGHTNESS.load(std::sync::atomic::Ordering::Relaxed)
}

type SavedRadios = Vec<(windows::Devices::Radios::RadioKind, bool)>;

static FLIGHT_MODE_SAVED_RADIOS: std::sync::Mutex<Option<SavedRadios>> =
    std::sync::Mutex::new(None);

fn radio_states_sync() -> Result<SavedRadios, String> {
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
    let mut states = Vec::new();
    for i in 0..radios.Size().unwrap_or(0) {
        if let Ok(radio) = radios.GetAt(i) {
            if let (Ok(kind), Ok(state)) = (radio.Kind(), radio.State()) {
                states.push((kind, state == RadioState::On));
            }
        }
    }
    Ok(states)
}

fn flight_mode_state_sync() -> Result<bool, String> {
    let radios = radio_states_sync()?;
    if radios.is_empty() {
        return Err("No radios found".to_string());
    }
    Ok(radios.iter().all(|(_, on)| !*on))
}

fn set_flight_mode_sync(enabled: bool) -> Result<bool, String> {
    let radios = radio_states_sync()?;
    if radios.is_empty() {
        return Err("No radios found".to_string());
    }
    if enabled {
        let snapshot = radios.clone();
        for (kind, _) in &radios {
            let _ = set_radio_state_sync(*kind, false);
        }
        let mut saved = FLIGHT_MODE_SAVED_RADIOS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *saved = Some(snapshot);
    } else {
        let previous = FLIGHT_MODE_SAVED_RADIOS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        match previous {
            Some(previous) => {
                for (kind, was_on) in previous {
                    let _ = set_radio_state_sync(kind, was_on);
                }
            }
            None => {
                for (kind, _) in &radios {
                    let _ = set_radio_state_sync(*kind, true);
                }
            }
        }
    }
    flight_mode_state_sync()
}

mod quiet_hours {
    use std::ffi::c_void;
    use windows::core::{GUID, HRESULT, Interface, IUnknown};

    const CLSID_QUIET_HOURS_SETTINGS: GUID =
        GUID::from_u128(0xF53321FA_34F8_4B7F_B9A3_361877CB94CF);
    const IID_IQUIET_HOURS_SETTINGS: GUID =
        GUID::from_u128(0x6BFF4732_81EC_4FFB_AE67_B6C1BC29631F);

    pub const PROFILE_UNRESTRICTED: &str = "Microsoft.QuietHoursProfile.Unrestricted";
    pub const PROFILE_PRIORITY_ONLY: &str = "Microsoft.QuietHoursProfile.PriorityOnly";

    #[repr(C)]
    struct QuietHoursSettingsVtable {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_user_selected_profile: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> HRESULT,
        put_user_selected_profile: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    }

    #[repr(transparent)]
    #[derive(Clone)]
    struct IQuietHoursSettings(IUnknown);

    unsafe impl Interface for IQuietHoursSettings {
        type Vtable = QuietHoursSettingsVtable;
        const IID: GUID = IID_IQUIET_HOURS_SETTINGS;
    }

    fn create() -> Result<IQuietHoursSettings, String> {
        unsafe {
            windows::Win32::System::Com::CoCreateInstance(
                &CLSID_QUIET_HOURS_SETTINGS,
                None,
                windows::Win32::System::Com::CLSCTX_LOCAL_SERVER,
            )
        }
        .map_err(|e| format!("Do Not Disturb interface unavailable: {e}"))
    }

    pub fn read_profile() -> Result<String, String> {
        let settings = create()?;
        let mut profile: *mut u16 = std::ptr::null_mut();
        unsafe { (settings.vtable().get_user_selected_profile)(settings.as_raw(), &mut profile) }
            .ok()
            .map_err(|e| e.to_string())?;
        if profile.is_null() {
            return Ok(String::new());
        }
        let mut len = 0usize;
        while unsafe { *profile.add(len) } != 0 {
            len += 1;
        }
        let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(profile, len) });
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(profile as *const c_void)) };
        Ok(value)
    }

    pub fn write_profile(profile: &str) -> Result<(), String> {
        let settings = create()?;
        let wide: Vec<u16> = profile.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { (settings.vtable().put_user_selected_profile)(settings.as_raw(), wide.as_ptr()) }
            .ok()
            .map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub async fn get_flight_mode_state() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(flight_mode_state_sync)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_flight_mode(enabled: bool) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || set_flight_mode_sync(enabled))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_dnd_state() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(|| {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let profile = quiet_hours::read_profile()?;
        Ok(!profile.is_empty() && profile != quiet_hours::PROFILE_UNRESTRICTED)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_dnd(enabled: bool) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let target = if enabled {
            quiet_hours::PROFILE_PRIORITY_ONLY
        } else {
            quiet_hours::PROFILE_UNRESTRICTED
        };
        let current = quiet_hours::read_profile()?;
        if current != target {
            quiet_hours::write_profile(target)?;
        }
        let mut actual = quiet_hours::read_profile()?;
        if actual != target {
            std::thread::sleep(std::time::Duration::from_millis(150));
            actual = quiet_hours::read_profile()?;
        }
        Ok(!actual.is_empty() && actual != quiet_hours::PROFILE_UNRESTRICTED)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn open_airplane_mode_settings() {
    unsafe {
        use windows::Win32::UI::Shell::ShellExecuteA;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let _ = ShellExecuteA(
            None,
            windows::core::PCSTR(c"open".as_ptr() as *const u8),
            windows::core::PCSTR(c"ms-settings:network-airplanemode".as_ptr() as *const u8),
            windows::core::PCSTR::null(),
            windows::core::PCSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[tauri::command]
pub fn set_brightness(app: AppHandle, brightness: u32) {
    let val = brightness.min(100);
    crate::state::CURRENT_BRIGHTNESS.store(val, Ordering::Relaxed);
    crate::state::LAST_BRIGHTNESS_CHANGE.store(crate::utils::get_now_ms(), Ordering::Relaxed);
    let _ = app.emit(
        "brightness-change",
        BrightnessChangeEvent { brightness: val },
    );
    if let Some(tx) = crate::state::BRIGHTNESS_SENDER.get() {
        let _ = tx.send(val);
    }
}

#[tauri::command]
pub async fn get_battery_saver_state() -> Result<bool, String> {
    // Uses Windows Runtime PowerManager ΓÇö no PowerShell required.
    tauri::async_runtime::spawn_blocking(|| {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        use windows::System::Power::{EnergySaverStatus, PowerManager};
        match PowerManager::EnergySaverStatus() {
            Ok(status) => Ok(status == EnergySaverStatus::On),
            Err(_) => Ok(false), // No battery / not supported on this device
        }
    })
    .await
    .map_err(|e| e.to_string())?
}
