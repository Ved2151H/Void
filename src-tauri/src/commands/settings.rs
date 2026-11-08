use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager, Window};
use crate::state::SETTINGS_FILE_LOCK;

fn re_register_appbars(app: &AppHandle, settings: &HashMap<String, serde_json::Value>) {
    // Reconcile the notch from the live mode: a scale change alters the
    // reserved strip height, and peek/smart must never hold a reservation.
    crate::services::reconcile_main_appbar(app);
    if let Some(dock_win) = app.get_webview_window("dock") {
        let is_fixed = settings
            .get("bloom-dock-mode")
            .map(|v| v.as_str() == Some("fixed"))
            .unwrap_or(false);
        if is_fixed {
            crate::services::register_dock_appbar(dock_win);
        }
    }
}

#[tauri::command(async)]
pub fn save_setting(app: AppHandle, key: String, value: serde_json::Value) -> Result<(), String> {
    // Async command workers must emit in commit order: a later save must not
    // broadcast before an earlier one and leave windows on an older value.
    static SAVE_COMMAND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _save_guard = SAVE_COMMAND_LOCK.lock().map_err(|e| e.to_string())?;
    let path = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("settings.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let settings = crate::utils::save_setting_file(&path, key.clone(), value).map_err(|error| {
        crate::diagnostics::record("settings_save_failed", &format!("key={key} error={error}"));
        error
    })?;

    // Broadcast so all windows sync ΓÇö emit the key as-is (bloom-prefixed)
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({ "key": &key, "value": &settings[&key] }),
    );

    if key == "bloom-scale" {
        re_register_appbars(&app, &settings);
    }
    Ok(())
}

#[tauri::command]
pub fn load_settings(app: AppHandle) -> Result<HashMap<String, serde_json::Value>, String> {
    let path = match app.path().app_config_dir() {
        Ok(p) => p.join("settings.json"),
        Err(_) => return Ok(HashMap::new()),
    };
    crate::utils::read_settings_file(&path)
}

/// Whether settings.json exists and parses, which load_settings can't tell
/// apart from a missing file since both load as an empty map.
#[tauri::command]
pub fn settings_file_readable(app: AppHandle) -> bool {
    app.path().app_config_dir().ok().is_some_and(|dir| {
        let path = dir.join("settings.json");
        path.exists() && crate::utils::read_settings_file(&path).is_ok()
    })
}

/// Export and import both pick their file in a native dialog opened from
/// Rust, so the path a webview can influence never reaches the filesystem
/// commands. A script injection in any Bloom window would otherwise turn
/// these into arbitrary file read/write as the user.
#[tauri::command]
pub async fn export_settings_to_file(
    app: AppHandle,
    window: Window,
) -> Result<Option<String>, String> {
    if window.label() != "settings" {
        return Err("Settings export is only available from the Settings window".into());
    }
    let source = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("settings.json");
    if !source.exists() {
        return Err("No settings file found".into());
    }
    let content = std::fs::read_to_string(&source).map_err(|e| e.to_string())?;
    // Validate it's valid JSON before offering it for export.
    crate::utils::read_settings_file(&source)?;

    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("Export Void Settings")
            .set_file_name("void-settings.json")
            .add_filter("JSON", &["json"])
            .blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(file_path) = picked else {
        return Ok(None);
    };
    let path = file_path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write file: {}", e))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub async fn import_settings_from_file(
    app: AppHandle,
    window: Window,
) -> Result<Option<String>, String> {
    if window.label() != "settings" {
        return Err("Settings import is only available from the Settings window".into());
    }
    let picked = tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        move || {
            use tauri_plugin_dialog::DialogExt;
            app.dialog()
                .file()
                .set_parent(&window)
                .set_title("Import Void Settings")
                .add_filter("JSON", &["json"])
                .blocking_pick_file()
        }
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(file_path) = picked else {
        return Ok(None);
    };
    let path = file_path.into_path().map_err(|e| e.to_string())?;
    let file = std::fs::File::open(&path).map_err(|e| format!("Failed to open file: {}", e))?;
    let size = file
        .metadata()
        .map_err(|e| format!("Failed to inspect file: {}", e))?
        .len();
    if size > MAX_SETTINGS_IMPORT_BYTES {
        return Err("Settings file exceeds the 1 MiB import limit".into());
    }
    let content = read_settings_import(file)?;
    apply_imported_settings(&app, &content)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

const MAX_SETTINGS_IMPORT_BYTES: u64 = 1024 * 1024;

fn read_settings_import(reader: impl std::io::Read) -> Result<String, String> {
    use std::io::Read;

    // Bound the read as well as checking metadata, in case the file grows.
    let mut bytes = Vec::new();
    reader
        .take(MAX_SETTINGS_IMPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read file: {}", e))?;
    if bytes.len() as u64 > MAX_SETTINGS_IMPORT_BYTES {
        return Err("Settings file exceeds the 1 MiB import limit".into());
    }
    String::from_utf8(bytes).map_err(|e| format!("Settings file is not valid UTF-8: {}", e))
}

fn apply_imported_settings(app: &AppHandle, settings: &str) -> Result<(), String> {
    let imported: HashMap<String, serde_json::Value> =
        serde_json::from_str(settings.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("Invalid JSON: {}", e))?;

    let path = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("settings.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let _guard = SETTINGS_FILE_LOCK.lock().map_err(|e| e.to_string())?;
    let previous = crate::utils::read_settings_file(&path)?;
    crate::utils::write_settings_file(&path, &imported)?;
    crate::utils::replace_settings_cache(imported.clone());
    drop(_guard);

    // Broadcast changes for every imported key so all windows re-sync
    for (key, value) in &imported {
        let _ = app.emit(
            "settings-changed",
            serde_json::json!({ "key": key, "value": value }),
        );
    }

    // Keys the imported file doesn't have must leave each window's localStorage
    // mirror too, or they come back from it on the next start. Same sentinels
    // as reset_settings are kept.
    let keep = ["bloom-first-run", "bloom-app-version"];
    for key in previous.keys().filter(|key| {
        key.starts_with("bloom-") && !keep.contains(&key.as_str()) && !imported.contains_key(*key)
    }) {
        let _ = app.emit(
            "settings-external-changed",
            serde_json::json!({ "key": key, "value": null }),
        );
    }

    if imported.get("bloom-scale").is_some() {
        re_register_appbars(app, &imported);
    }

    Ok(())
}

#[tauri::command]
pub fn reset_settings(app: AppHandle) -> Result<(), String> {
    let path = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("settings.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Keep non-Bloom keys (the file is user-editable) and the first-run/version
    // sentinels (lifecycle markers, not preferences ΓÇö keeping them avoids
    // replaying the splash after a reset). Everything else falls back to
    // defaults in both the backend and the frontends.
    let keep = ["bloom-first-run", "bloom-app-version"];
    let _guard = SETTINGS_FILE_LOCK.lock().map_err(|e| e.to_string())?;
    let mut settings = crate::utils::read_settings_file(&path)?;
    let removed: Vec<String> = settings
        .keys()
        .filter(|key| key.starts_with("bloom-") && !keep.contains(&key.as_str()))
        .cloned()
        .collect();
    for key in &removed {
        settings.remove(key);
    }

    crate::utils::write_settings_file(&path, &settings)?;
    crate::utils::replace_settings_cache(settings);
    drop(_guard);

    // Mirror the external-edit watcher so every open window drops the removed
    // keys from localStorage. The caller restarts Bloom right after, but this
    // keeps the reset correct even if a window lingers.
    for key in removed {
        let _ = app.emit(
            "settings-external-changed",
            serde_json::json!({ "key": key, "value": null }),
        );
    }
    Ok(())
}

pub fn setup_settings_watcher(app: AppHandle) {
    use tauri::Manager;

    let config_dir = match app.path().app_config_dir() {
        Ok(p) => p,
        Err(_) => return,
    };
    let settings_path = config_dir.join("settings.json");

    // Initialize SETTINGS_CACHE if not yet set (backup for race with init_settings_cache)
    let _ = crate::state::SETTINGS_CACHE.set(std::sync::Mutex::new(HashMap::new()));

    std::thread::spawn(move || {
        use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
            FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, OPEN_EXISTING,
        };

        unsafe {
            let dir_path: Vec<u16> = config_dir
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            let dir_handle = match CreateFileW(
                windows::core::PCWSTR(dir_path.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                windows::Win32::Storage::FileSystem::FILE_SHARE_READ
                    | windows::Win32::Storage::FileSystem::FILE_SHARE_WRITE
                    | windows::Win32::Storage::FileSystem::FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            ) {
                Ok(h) if h != INVALID_HANDLE_VALUE => h,
                _ => return,
            };

            // ReadDirectoryChangesW requires DWORD-aligned storage.
            let mut notify_buffer = [0u32; 1024];
            let mut bytes_returned = 0u32;

            loop {
                let success = ReadDirectoryChangesW(
                    dir_handle,
                    notify_buffer.as_mut_ptr() as *mut _,
                    std::mem::size_of_val(&notify_buffer) as u32,
                    false,
                    FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME,
                    Some(&mut bytes_returned),
                    None,
                    None,
                );

                if success.is_err() {
                    break;
                }

                // The directory was opened synchronously: ReadDirectoryChangesW
                // already waited. An OVERLAPPED event on this handle is ignored
                // and waiting on it would stall the watcher after its first change.
                std::thread::sleep(std::time::Duration::from_millis(200));

                // Keep the file read and cache comparison in the same
                // transaction as writes from the four webviews. Otherwise a
                // delayed watcher read can roll the cache back after a save.
                let changes = {
                    let Ok(_file_guard) = SETTINGS_FILE_LOCK.lock() else {
                        continue;
                    };
                    crate::utils::read_settings_file(&settings_path)
                        .ok()
                        .map(|new_settings| {
                            let mut cache = crate::state::SETTINGS_CACHE
                                .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
                                .lock()
                                .unwrap();

                            let mut changed = Vec::new();
                            for (key, value) in &new_settings {
                                if cache.get(key) != Some(value) {
                                    changed.push((key.clone(), value.clone()));
                                }
                            }

                            let removed: Vec<String> = cache
                                .keys()
                                .filter(|k| !new_settings.contains_key(*k))
                                .cloned()
                                .collect();

                            *cache = new_settings.clone();
                            (changed, removed)
                        })
                };
                if let Some((changed, removed)) = changes {
                    // Both file and cache locks are released before emitting.
                    for (key, value) in &changed {
                        let _ = app.emit(
                            "settings-external-changed",
                            serde_json::json!({ "key": key, "value": value }),
                        );
                    }
                    for key in removed {
                        let _ = app.emit(
                            "settings-external-changed",
                            serde_json::json!({ "key": key, "value": null }),
                        );
                    }
                }
            }

            let _ = CloseHandle(dir_handle);
        }
    });
}

#[cfg(test)]
mod settings_import_tests {
    use super::{read_settings_import, MAX_SETTINGS_IMPORT_BYTES};
    use std::io::{Cursor, Read};

    #[test]
    fn accepts_utf8_settings_at_the_size_limit() {
        let mut content = String::from("{\"bloom-weather-city\":\"Montr├⌐al\"}");
        content.extend(std::iter::repeat_n(
            ' ',
            MAX_SETTINGS_IMPORT_BYTES as usize - content.len(),
        ));
        assert_eq!(read_settings_import(content.as_bytes()).unwrap(), content);
    }

    #[test]
    fn rejects_oversized_input_without_reading_the_rest() {
        let mut reader = Cursor::new(vec![b' '; MAX_SETTINGS_IMPORT_BYTES as usize + 100]);
        let error = read_settings_import(&mut reader).unwrap_err();
        assert!(error.contains("1 MiB"));
        assert_eq!(reader.position(), MAX_SETTINGS_IMPORT_BYTES + 1);
    }

    #[test]
    fn rejects_invalid_utf8_and_read_errors() {
        assert!(read_settings_import(&[0xff][..])
            .unwrap_err()
            .contains("UTF-8"));
        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("read failed"))
            }
        }
        assert!(read_settings_import(FailedReader)
            .unwrap_err()
            .contains("read failed"));
    }
}
