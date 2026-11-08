use std::sync::atomic::Ordering;
use windows::core::Interface;
use super::shared::{friendly_process_name, process_image_path};
use crate::state::{COMMAND_SENDER, CURRENT_VOLUME, VOLUME_MIXER_RECT};
use crate::types::{AudioSessionInfo, VolumeChangeEvent};

#[tauri::command]
pub fn open_sound_settings() {
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::ShellExecuteA;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let _ = ShellExecuteA(
            Some(HWND(std::ptr::null_mut())),
            windows::core::PCSTR(c"open".as_ptr() as *const u8),
            windows::core::PCSTR(c"ms-settings:sound".as_ptr() as *const u8),
            windows::core::PCSTR::null(),
            windows::core::PCSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[tauri::command]
pub fn set_volume(volume: f32) {
    if let Some(sender) = COMMAND_SENDER.get() {
        let _ = sender.send(crate::types::SystemCommand::SetVolume(volume));
    }
}

/// Calls `f` for every live audio session on every active render endpoint.
/// Initializes COM on the calling thread and balances it before returning.
unsafe fn for_each_audio_session<F: FnMut(&windows::Win32::Media::Audio::IAudioSessionControl2)>(
    mut f: F,
) -> Result<(), String> {
    use windows::Win32::Media::Audio::{
        eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
        DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    let com_initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
    let result = (|| -> Result<(), String> {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(
            &windows::Win32::Media::Audio::MMDeviceEnumerator,
            None,
            CLSCTX_ALL,
        )
        .map_err(|e| e.to_string())?;
        // Windows' volume mixer lists sessions per output device, and per-app
        // output routing (Windows 11) can put an app on a non-default device,
        // so scan every active render endpoint instead of only the default one.
        let devices = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| e.to_string())?;
        let device_count = devices.GetCount().map_err(|e| e.to_string())?;
        for i in 0..device_count {
            let Ok(device) = devices.Item(i) else {
                continue;
            };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = manager.GetSessionEnumerator() else {
                continue;
            };
            let Ok(count) = sessions.GetCount() else {
                continue;
            };
            for j in 0..count {
                let Ok(control) = sessions.GetSession(j) else {
                    continue;
                };
                let Ok(control) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                f(&control);
            }
        }
        Ok(())
    })();
    if com_initialized {
        CoUninitialize();
    }
    result
}

/// Active windows with an audio session on the default output device,
/// de-duplicated by process and sorted with currently-playing apps first.
#[tauri::command]
pub async fn get_audio_sessions() -> Result<Vec<AudioSessionInfo>, String> {
    tauri::async_runtime::spawn_blocking(|| unsafe {
        use std::collections::HashSet;
        use windows::Win32::Media::Audio::{
            AudioSessionStateActive, AudioSessionStateExpired, ISimpleAudioVolume,
        };

        let own_pid = std::process::id();
        let mut seen: HashSet<u32> = HashSet::new();
        let mut found: Vec<(AudioSessionInfo, bool)> = Vec::new();

        for_each_audio_session(|control| {
            // Only S_OK marks the system-sounds session; S_FALSE is an ordinary app.
            if control.IsSystemSoundsSession() == windows::Win32::Foundation::S_OK {
                return;
            }
            let Ok(pid) = control.GetProcessId() else {
                return;
            };
            if pid == 0 || pid == own_pid || !seen.insert(pid) {
                return;
            }
            let state = control.GetState().unwrap_or_default();
            if state == AudioSessionStateExpired {
                seen.remove(&pid);
                return;
            }
            let Ok(volume_ctl) = control.cast::<ISimpleAudioVolume>() else {
                return;
            };
            let process_path = process_image_path(pid);
            let name = match process_path.as_deref() {
                Some(path) => friendly_process_name(path),
                None => format!("Process {pid}"),
            };
            found.push((
                AudioSessionInfo {
                    pid,
                    name,
                    process_path,
                    volume: volume_ctl.GetMasterVolume().unwrap_or(1.0),
                    is_muted: volume_ctl.GetMute().map(|m| m.as_bool()).unwrap_or(false),
                },
                state == AudioSessionStateActive,
            ));
        })?;

        found.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase()))
        });
        Ok(found.into_iter().map(|(info, _)| info).collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_app_volume(pid: u32, volume: f32) -> Result<(), String> {
    let volume = volume.clamp(0.0, 1.0);
    tauri::async_runtime::spawn_blocking(move || unsafe {
        use windows::Win32::Media::Audio::ISimpleAudioVolume;

        for_each_audio_session(|control| {
            if control.GetProcessId().ok() != Some(pid) {
                return;
            }
            if let Ok(volume_ctl) = control.cast::<ISimpleAudioVolume>() {
                let _ = volume_ctl.SetMasterVolume(volume, std::ptr::null());
                if volume > 0.0 {
                    let _ = volume_ctl.SetMute(false, std::ptr::null());
                }
            }
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_app_mute(pid: u32, muted: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || unsafe {
        use windows::Win32::Media::Audio::ISimpleAudioVolume;

        for_each_audio_session(|control| {
            if control.GetProcessId().ok() != Some(pid) {
                return;
            }
            if let Ok(volume_ctl) = control.cast::<ISimpleAudioVolume>() {
                let _ = volume_ctl.SetMute(muted, std::ptr::null());
            }
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Reports the on-screen bounds of the expanded mixer panel (overlay CSS px)
/// so the mouse hook keeps forwarding cursor events while it is open.
#[tauri::command]
pub fn set_volume_mixer_rect(x: f64, y: f64, width: f64, height: f64) {
    if let Ok(mut rect) = VOLUME_MIXER_RECT.lock() {
        *rect = Some((x, y, width, height));
    }
}

#[tauri::command]
pub fn clear_volume_mixer_rect() {
    if let Ok(mut rect) = VOLUME_MIXER_RECT.lock() {
        *rect = None;
    }
}

#[tauri::command]
pub fn get_volume() -> f32 {
    crate::state::CURRENT_VOLUME.load(std::sync::atomic::Ordering::Relaxed) as f32 / 100.0
}

/// Live volume and mute state of the default render endpoint, also refreshing
/// the cache `get_volume` reads. The system worker only emits `volume-change`
/// when the value differs from its last poll, and its first poll happens before
/// the overlay webview can register listeners, so HUDs must seed themselves
/// with this instead of the cached default.
#[tauri::command]
pub async fn get_volume_state() -> Result<VolumeChangeEvent, String> {
    tauri::async_runtime::spawn_blocking(|| unsafe {
        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
        use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator};
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
        };

        let com_initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> Result<VolumeChangeEvent, String> {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(
                &windows::Win32::Media::Audio::MMDeviceEnumerator,
                None,
                CLSCTX_ALL,
            )
            .map_err(|e| e.to_string())?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| e.to_string())?;
            let endpoint: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| e.to_string())?;
            let volume = endpoint
                .GetMasterVolumeLevelScalar()
                .map_err(|e| e.to_string())?;
            let is_muted = endpoint.GetMute().map(|m| m.as_bool()).unwrap_or(false);
            CURRENT_VOLUME.store((volume * 100.0) as u32, Ordering::Relaxed);
            Ok(VolumeChangeEvent { volume, is_muted })
        })();
        if com_initialized {
            CoUninitialize();
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Friendly name of the default audio output, for the System panel's volume
/// card. `None` means the machine has no active render endpoint.
#[tauri::command]
pub async fn get_audio_output() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| unsafe {
        use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
        use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator};
        use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
            COINIT_MULTITHREADED, STGM_READ,
        };

        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> Result<Option<String>, String> {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(
                &windows::Win32::Media::Audio::MMDeviceEnumerator,
                None,
                CLSCTX_ALL,
            )
            .map_err(|e| e.to_string())?;
            let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) else {
                return Ok(None);
            };
            let store = device.OpenPropertyStore(STGM_READ).map_err(|e| e.to_string())?;
            let value = store
                .GetValue(&PKEY_Device_FriendlyName)
                .map_err(|e| e.to_string())?;
            let text = PropVariantToStringAlloc(&value).map_err(|e| e.to_string())?;
            let name = text.to_string().ok();
            CoTaskMemFree(Some(text.0 as *const _));
            Ok(name.filter(|name| !name.trim().is_empty()))
        })();
        if initialized {
            CoUninitialize();
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}
