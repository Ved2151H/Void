use tauri::AppHandle;

#[tauri::command]
pub fn setup_shelf_drop(app: AppHandle) {
    let first = app.clone();
    let _ = app.run_on_main_thread(move || crate::shelf::setup(&first));
    // WebView2 can create or recreate its render child after first paint;
    // re-register once it has settled so the drop target tracks the live window.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
        let late = app.clone();
        let _ = app.run_on_main_thread(move || crate::shelf::setup(&late));
    });
}

/// Preview for a shelf item: a picture, the opening text, or neither (the
/// frontend then shows an icon for the file type). See shelf_preview.rs.
#[tauri::command]
pub async fn shelf_thumbnail(path: String) -> crate::shelf_preview::ShelfPreview {
    tauri::async_runtime::spawn_blocking(move || crate::shelf_preview::preview(&path))
        .await
        .unwrap_or(crate::shelf_preview::ShelfPreview {
            thumb: None,
            text: None,
            folder: false,
        })
}

/// Opens a shelf file in its default app. Uses the opener plugin's Rust API
/// directly: its JS `open_path` command additionally requires a path scope,
/// which an empty capability would deny for every file.
#[tauri::command]
pub async fn shelf_open(path: String) -> Result<(), String> {
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|e| e.to_string())
}

/// Drops shelf entries whose files no longer exist (moved, deleted, drive
/// unplugged). Called once at startup so the shelf does not accumulate dead
/// references across sessions.
#[tauri::command]
pub async fn shelf_prune(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|p| std::path::Path::new(p).exists())
        .collect()
}

/// Generic document icon used as the native drag preview for shelf items that
/// have no thumbnail. Returned as a PNG data URI.
#[tauri::command]
pub fn shelf_drag_icon() -> Option<String> {
    use base64::Engine;

    let page = image::Rgba([246, 246, 246, 255]);
    let fold = image::Rgba([198, 198, 198, 255]);
    let line = image::Rgba([152, 152, 152, 255]);
    let mut img = image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));

    for y in 6..58u32 {
        for x in 14..50u32 {
            let in_fold = x >= 38 && y <= 18 && (x - 38) + (y - 6) >= 12;
            img.put_pixel(x, y, if in_fold { fold } else { page });
        }
    }
    for (i, y) in [22u32, 30, 38, 46].into_iter().enumerate() {
        let end = if i == 3 { 36 } else { 42 };
        for x in 20..end {
            img.put_pixel(x, y, line);
        }
    }

    let mut png_bytes: Vec<u8> = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Some(format!("data:image/png;base64,{}", b64))
}
