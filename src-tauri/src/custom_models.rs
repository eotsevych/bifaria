use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

use crate::settings::get_data_dir;

const DOWNLOAD_PROGRESS_EVENT: &str = "model-download-progress";

pub fn get_models_dir() -> PathBuf {
    let dir = get_data_dir().join("models");
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

fn sanitize_filename(name: &str) -> String {
    let n = name.trim();
    // Take last path segment if URL-like
    let base = n.rsplit('/').next().unwrap_or(n);
    let base = base.split('?').next().unwrap_or(base);
    let base = base.split('#').next().unwrap_or(base);
    let base = base.trim();
    if base.is_empty() {
        return "custom-model.bin".to_string();
    }
    // Ensure .bin extension
    if base.to_ascii_lowercase().ends_with(".bin") {
        base.to_string()
    } else if base.contains('.') {
        base.to_string()
    } else {
        format!("{}.bin", base)
    }
}

/// Opens a folder in the platform's file manager (Finder, Explorer, or the XDG default).
pub fn open_in_file_manager(dir: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";

    std::process::Command::new(program)
        .arg(dir)
        .spawn()
        .map_err(|e| format!("Failed to open file manager: {}", e))?;
    Ok(())
}

#[tauri::command]
pub fn open_models_folder() -> Result<String, String> {
    let dir = get_models_dir();
    open_in_file_manager(&dir)?;
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn get_models_dir_path() -> String {
    get_models_dir().to_string_lossy().to_string()
}

#[tauri::command]
pub fn import_model_file(source_path: String) -> Result<crate::transcribe::ModelCatalogItem, String> {
    let src = PathBuf::from(&source_path);
    if !src.exists() {
        return Err(format!("Source file not found: {}", source_path));
    }
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext != "bin" {
        return Err("Only .bin files are supported (GGML format)".to_string());
    }
    if let Some(why) = crate::transcribe::not_a_whisper_model(&src) {
        return Err(format!("This file can't be used as a Whisper model: {why}"));
    }
    let filename = src.file_name().and_then(|n| n.to_str()).ok_or("Invalid file name")?.to_string();
    let dest = get_models_dir().join(&filename);

    // If same file, just return catalog entry
    if src.canonicalize().ok() == dest.canonicalize().ok() {
        let size = src.metadata().map(|m| m.len()).unwrap_or(0);
        return Ok(make_custom_item(&filename, size));
    }

    if dest.exists() {
        // Overwrite with copy
        let _ = fs::remove_file(&dest);
    }
    fs::copy(&src, &dest).map_err(|e| format!("Failed to copy model: {}", e))?;
    let size = dest.metadata().map(|m| m.len()).unwrap_or(0);
    Ok(make_custom_item(&filename, size))
}

#[tauri::command]
pub fn pick_and_import_model() -> Result<crate::transcribe::ModelCatalogItem, String> {
    let file = rfd::FileDialog::new()
        .add_filter("GGML model", &["bin"])
        .set_title("Import GGML Model (.bin)")
        .pick_file()
        .ok_or_else(|| "No file selected".to_string())?;

    let path_str = file.to_string_lossy().to_string();
    import_model_file(path_str)
}

fn make_custom_item(filename: &str, size: u64) -> crate::transcribe::ModelCatalogItem {
    let stem = std::path::Path::new(filename).file_stem().and_then(|s| s.to_str()).unwrap_or(filename);
    let pretty = stem.replace(['_', '-'], " ");
    let name = format!("{} [Custom]", pretty);
    let size_desc = if size >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if size >= 1024 * 1024 {
        format!("{:.0} MB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{:.0} KB", size as f64 / 1024.0)
    } else {
        format!("{} B", size)
    };
    crate::transcribe::ModelCatalogItem {
        id: filename.to_string(),
        name,
        size_desc,
        speed_desc: "Custom GGML".to_string(),
        filename: filename.to_string(),
        downloaded: size > 0,
        file_size_bytes: size,
        family: "custom".to_string(),
        languages: crate::transcribe::whisper_languages(),
        tags: crate::transcribe::whisper_tags(),
        curated: true,
    }
}

#[tauri::command]
pub async fn download_custom_model(app_handle: AppHandle, url: String, file_name: Option<String>) -> Result<String, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("URL is empty".to_string());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("URL must start with http:// or https://".to_string());
    }

    let inferred = url.rsplit('/').next().unwrap_or("custom-model.bin").split('?').next().unwrap_or("custom-model.bin").to_string();
    let raw_name = file_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&inferred);
    let filename = sanitize_filename(raw_name);
    let dest = get_models_dir().join(&filename);
    let temp_path = dest.with_extension("downloading");

    // Ensure parent exists
    if let Some(parent) = dest.parent() {
        if !parent.exists() {
            let _ = fs::create_dir_all(parent);
        }
    }

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let resp = client.get(&url).send().await.map_err(|e| format!("Network error: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("Download failed with HTTP {}", resp.status()));
    }

    let total_size = resp.content_length().unwrap_or(0);

    let _ = app_handle.emit(
        "assistant-state-changed",
        serde_json::json!({
            "state": "transcribing",
            "title": "Downloading custom model…",
            "subtitle": format!("Downloading {} (0%)", filename)
        }),
    );

    let mut file = fs::File::create(&temp_path).map_err(|e| format!("Failed to create temp file: {}", e))?;
    let mut downloaded: u64 = 0;
    let mut last_pct: u32 = 0;
    let mut stream = resp.bytes_stream();
    use futures_util::StreamExt;

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| format!("Chunk error: {}", e))?;
        file.write_all(&chunk).map_err(|e| format!("Write error: {}", e))?;
        downloaded += chunk.len() as u64;
        let pct = if total_size > 0 {
            (downloaded as f64 / total_size as f64 * 100.0) as u32
        } else {
            0
        };
        // Emit at most every 1%
        if pct != last_pct || downloaded == total_size {
            last_pct = pct;
            let _ = app_handle.emit(
                DOWNLOAD_PROGRESS_EVENT,
                serde_json::json!({
                    "status": "downloading",
                    "model": filename,
                    "percent": pct.min(99),
                    "downloaded_bytes": downloaded,
                    "total_bytes": total_size
                }),
            );
            let _ = app_handle.emit(
                "assistant-state-changed",
                serde_json::json!({
                    "state": "transcribing",
                    "title": "Downloading custom model…",
                    "subtitle": format!("Downloading {} ({}%)", filename, pct.min(99))
                }),
            );
        }
    }

    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    // Basic validation: must be non-empty and ideally > 1k
    let meta = fs::metadata(&temp_path).map_err(|e| format!("Temp file missing: {}", e))?;
    if meta.len() < 1024 {
        let _ = fs::remove_file(&temp_path);
        return Err("Downloaded file is too small to be a valid model".to_string());
    }

    if let Some(why) = crate::transcribe::not_a_whisper_model(&temp_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("The downloaded file can't be used as a Whisper model: {why}"));
    }

    // Ensure .bin extension after sanitization already handled
    fs::rename(&temp_path, &dest).map_err(|e| format!("Failed to finalize file: {}", e))?;

    let _ = app_handle.emit(
        DOWNLOAD_PROGRESS_EVENT,
        serde_json::json!({
            "status": "complete",
            "model": filename,
            "percent": 100,
            "downloaded_bytes": downloaded,
            "total_bytes": if total_size > 0 { total_size } else { downloaded }
        }),
    );

    // Also emit complete for generic refresh
    let _ = app_handle.emit(
        "assistant-state-changed",
        serde_json::json!({
            "state": "idle",
            "title": "Ready",
            "subtitle": null
        }),
    );

    Ok(dest.to_string_lossy().to_string())
}
