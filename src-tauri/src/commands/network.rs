use super::shared::set_radio_state_sync;
use crate::types::WifiStatus;

#[tauri::command]
pub fn open_wifi_settings() {
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::ShellExecuteA;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let _ = ShellExecuteA(
            Some(HWND(std::ptr::null_mut())),
            windows::core::PCSTR(c"open".as_ptr() as *const u8),
            windows::core::PCSTR(c"ms-availablenetworks:".as_ptr() as *const u8),
            windows::core::PCSTR::null(),
            windows::core::PCSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

// ΓöÇΓöÇ Radio helpers ΓÇö Windows Runtime Windows.Devices.Radios ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ
// Replaces PowerShell -ExecutionPolicy Bypass scripts for WiFi/Bluetooth state.

fn get_radio_state_sync(kind: windows::Devices::Radios::RadioKind) -> Result<bool, String> {
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
                return Ok(radio.State().ok() == Some(RadioState::On));
            }
        }
    }
    Ok(false)
}

/// Wi-Fi radio and association state, so the notch quick settings tile can show
/// Off / On / the connected network. The radio state alone only says Wi-Fi is
/// on, so the WLAN API supplies the association.
#[tauri::command]
pub async fn get_wifi_status() -> Result<WifiStatus, String> {
    tauri::async_runtime::spawn_blocking(|| -> Result<WifiStatus, String> {
        let enabled = get_radio_state_sync(windows::Devices::Radios::RadioKind::WiFi)?;
        let ssid = if enabled {
            crate::wifi::connected_ssid()
        } else {
            None
        };
        Ok(WifiStatus {
            enabled,
            connected: ssid.is_some(),
            ssid,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The connection the Windows host actually routes over, for the network
/// status widget: Wi-Fi with its SSID, the LAN side, or nothing.
#[tauri::command]
pub async fn get_network_widget_state() -> Result<crate::wifi::NetworkWidgetState, String> {
    crate::wifi::network_widget_state()
}

#[tauri::command]
pub async fn set_wifi_state(enabled: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        set_radio_state_sync(windows::Devices::Radios::RadioKind::WiFi, enabled)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_bluetooth_state() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(|| {
        get_radio_state_sync(windows::Devices::Radios::RadioKind::Bluetooth)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_bluetooth_state(enabled: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        set_radio_state_sync(windows::Devices::Radios::RadioKind::Bluetooth, enabled)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn tethering_manager_sync(
) -> Result<windows::Networking::NetworkOperators::NetworkOperatorTetheringManager, String> {
    use windows::Networking::Connectivity::NetworkInformation;
    use windows::Networking::NetworkOperators::NetworkOperatorTetheringManager;
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
    }
    if let Ok(profile) = NetworkInformation::GetInternetConnectionProfile() {
        if let Ok(manager) = NetworkOperatorTetheringManager::CreateFromConnectionProfile(&profile)
        {
            return Ok(manager);
        }
    }
    if let Ok(profiles) = NetworkInformation::GetConnectionProfiles() {
        for i in 0..profiles.Size().unwrap_or(0) {
            if let Ok(profile) = profiles.GetAt(i) {
                if let Ok(manager) =
                    NetworkOperatorTetheringManager::CreateFromConnectionProfile(&profile)
                {
                    return Ok(manager);
                }
            }
        }
    }
    Err("Mobile Hotspot is not available on this device".to_string())
}

fn hotspot_state_sync() -> Result<bool, String> {
    use windows::Networking::NetworkOperators::TetheringOperationalState;
    let manager = tethering_manager_sync()?;
    let state = manager
        .TetheringOperationalState()
        .map_err(|e| e.to_string())?;
    Ok(state == TetheringOperationalState::On)
}

fn set_hotspot_state_sync(enabled: bool) -> Result<bool, String> {
    use windows::Networking::NetworkOperators::{
        TetheringOperationStatus, TetheringOperationalState,
    };
    let manager = tethering_manager_sync()?;
    let current = manager
        .TetheringOperationalState()
        .map_err(|e| e.to_string())?;
    if (current == TetheringOperationalState::On) != enabled {
        let operation = if enabled {
            manager.StartTetheringAsync()
        } else {
            manager.StopTetheringAsync()
        };
        let result = operation
            .map_err(|e| e.to_string())?
            .get()
            .map_err(|e| e.to_string())?;
        let status = result.Status().map_err(|e| e.to_string())?;
        if status != TetheringOperationStatus::Success {
            return Err(format!("Mobile Hotspot failed (status {})", status.0));
        }
    }
    let state = manager
        .TetheringOperationalState()
        .map_err(|e| e.to_string())?;
    Ok(state == TetheringOperationalState::On)
}

#[tauri::command]
pub async fn get_hotspot_state() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(hotspot_state_sync)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_hotspot(enabled: bool) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || set_hotspot_state_sync(enabled))
        .await
        .map_err(|e| e.to_string())?
}

// Settings openers ΓÇö use ShellExecuteA directly instead of spawning powershell

#[tauri::command]
pub fn open_bluetooth_settings() {
    unsafe {
        use windows::Win32::UI::Shell::ShellExecuteA;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let _ = ShellExecuteA(
            None,
            windows::core::PCSTR(c"open".as_ptr() as *const u8),
            windows::core::PCSTR(c"ms-settings:bluetooth".as_ptr() as *const u8),
            windows::core::PCSTR::null(),
            windows::core::PCSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[tauri::command]
pub fn get_network_speed() -> Result<(u64, u64), String> {
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    static LAST_SAMPLE: OnceLock<Mutex<Option<(Instant, u64, u64)>>> = OnceLock::new();

    let (mut total_in, mut total_out) = (0u64, 0u64);
    unsafe {
        // GetIfTable2 allocates the table and writes its address here.
        let mut out = std::mem::MaybeUninit::<
            *mut windows::Win32::NetworkManagement::IpHelper::MIB_IF_TABLE2,
        >::uninit();
        let ret = windows::Win32::NetworkManagement::IpHelper::GetIfTable2(out.as_mut_ptr());
        if !ret.is_ok() {
            return Err(format!("GetIfTable2 failed: {ret:?}"));
        }
        let Some(table) = std::ptr::NonNull::new(out.assume_init()) else {
            return Err("GetIfTable2 returned no table".to_string());
        };
        let table_ptr = table.as_ptr();
        // `Table` is declared with one element but holds `NumEntries` rows, so
        // read them through a pointer to the whole allocation, not the array.
        let rows = std::slice::from_raw_parts(
            std::ptr::addr_of!((*table_ptr).Table)
                .cast::<windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2>(),
            table.as_ref().NumEntries as usize,
        );
        for row in rows {
            total_in = total_in.saturating_add(row.InOctets);
            total_out = total_out.saturating_add(row.OutOctets);
        }
        windows::Win32::NetworkManagement::IpHelper::FreeMibTable(table_ptr as *const _);
    }

    let now = Instant::now();
    let mut last = LAST_SAMPLE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|e| e.to_string())?;
    let speed = if let Some((previous, sent, recv)) = *last {
        let elapsed_ms = now.duration_since(previous).as_millis().max(1);
        let rate = |current: u64, previous: u64| {
            ((current.saturating_sub(previous) as u128 * 1000) / elapsed_ms).min(u64::MAX as u128)
                as u64
        };
        (rate(total_out, sent), rate(total_in, recv))
    } else {
        (0, 0)
    };
    *last = Some((now, total_out, total_in));
    Ok(speed)
}
