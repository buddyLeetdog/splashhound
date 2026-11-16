use std::{path::Path, path::PathBuf, sync::Arc};

use serde::Serialize;
use tauri::State;

use crate::{config, db, media, scan, AppState, ScanStatus};

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub root: Option<String>,
    pub media_base: String,
    pub db_path: String,
    pub data_dir: String,
    pub default_data_dir: String,
    pub data_warning: Option<String>,
    pub cache_dir: String,
    pub trash_dir: Option<String>,
    pub ffmpeg_ok: bool,
    pub ffmpeg_dir: Option<String>,
    pub stats: db::Stats,
    pub tags: Vec<db::TagCount>,
    pub creators: Vec<String>,
    pub scan: ScanStatus,
    pub version: String,
}

#[tauri::command]
pub async fn bootstrap(state: State<'_, Arc<AppState>>) -> Result<Bootstrap, String> {
    let st = state.inner().clone();
    let root = st.root_path();
    let ff = st.ffmpeg();
    let (stats, tags, creators) = {
        let conn = st.db.lock().map_err(err)?;
        (
            db::stats(&conn).map_err(err)?,
            db::all_tags(&conn).map_err(err)?,
            db::creators(&conn).map_err(err)?,
        )
    };
    let trash_dir = root
        .as_ref()
        .map(|r| r.join("_trash").to_string_lossy().to_string());
    let root_string = root.map(|r| r.to_string_lossy().to_string());
    let scan_state = st.scan.lock().map_err(err)?.clone();
    Ok(Bootstrap {
        trash_dir,
        root: root_string,
        media_base: st.media_base(),
        db_path: st.db_path().to_string_lossy().to_string(),
        data_dir: st.data_dir().to_string_lossy().to_string(),
        default_data_dir: st.default_data_dir.to_string_lossy().to_string(),
        data_warning: st.data_warning(),
        cache_dir: st.cache_dir.to_string_lossy().to_string(),
        ffmpeg_ok: ff.is_some(),
        ffmpeg_dir: ff.map(|f| {
            f.ffmpeg
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        }),
        stats,
        tags,
        creators,
        scan: scan_state,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[tauri::command]
pub async fn pick_folder() -> Result<Option<String>, String> {
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Select the folder that holds your video library")
            .pick_folder()
    })
    .await
    .map_err(err)?;
    Ok(picked.map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
pub async fn set_root(state: State<'_, Arc<AppState>>, path: String) -> Result<String, String> {
    let st = state.inner().clone();
    let canonical =
        std::fs::canonicalize(&path).map_err(|_| format!("Folder not found: {path}"))?;
    if !canonical.is_dir() {
        return Err("That path is not a folder.".into());
    }
    let display = canonical.to_string_lossy().to_string();
    st.set_setting("root", &display)?;
    *st.root.write().unwrap() = Some(canonical);
    Ok(display)
}

#[tauri::command]
pub async fn start_scan(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let st = state.inner().clone();
    if st.root_path().is_none() {
        return Err("Select a library folder first.".into());
    }
    if st.scan.lock().map_err(err)?.running {
        return Err("A scan is already running.".into());
    }
    scan::spawn(st);
    Ok(())
}

#[tauri::command]
pub async fn scan_status(state: State<'_, Arc<AppState>>) -> Result<ScanStatus, String> {
    Ok(state.inner().scan.lock().map_err(err)?.clone())
}

#[tauri::command]
pub async fn cancel_scan(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.inner().cancel_scan();
    Ok(())
}

#[tauri::command]
pub async fn stats(state: State<'_, Arc<AppState>>) -> Result<db::Stats, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::stats(&conn).map_err(err)
}

#[tauri::command]
pub async fn list_videos(
    state: State<'_, Arc<AppState>>,
    filter: db::VideoFilter,
) -> Result<ListResult, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    let (total, items) = db::list_videos(&conn, &filter).map_err(err)?;
    Ok(ListResult { total, items })
}

#[derive(Serialize)]
pub struct ListResult {
    pub total: i64,
    pub items: Vec<db::VideoDto>,
}

#[tauri::command]
pub async fn get_video(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> Result<Option<db::VideoDto>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::get_video(&conn, id).map_err(err)
}

#[tauri::command]
pub async fn save_video(
    state: State<'_, Arc<AppState>>,
    id: i64,
    patch: db::VideoPatch,
) -> Result<db::VideoDto, String> {
    let mut conn = state.inner().db.lock().map_err(err)?;
    db::save_video(&mut conn, id, &patch).map_err(err)
}

#[tauri::command]
pub async fn rating_queue(
    state: State<'_, Arc<AppState>>,
    limit: Option<i64>,
) -> Result<Vec<db::VideoDto>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::rating_queue(&conn, limit.unwrap_or(500)).map_err(err)
}

#[tauri::command]
pub async fn duplicates(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<db::DuplicateGroup>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::duplicates(&conn).map_err(err)
}

#[tauri::command]
pub async fn remove_duplicate(state: State<'_, Arc<AppState>>, id: i64) -> Result<String, String> {
    let st = state.inner().clone();
    let video = st.video(id).ok_or("Video not found.")?;
    let root = st.root_path().ok_or("No library folder configured.")?;
    let trash = root.join("_trash");
    tokio::fs::create_dir_all(&trash).await.map_err(err)?;

    let src = PathBuf::from(&video.path);
    let mut target = trash.join(&video.file_name);
    if target.exists() {
        let stem = Path::new(&video.file_name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "video".into());
        let ext = Path::new(&video.file_name)
            .extension()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut counter = 1;
        loop {
            let name = if ext.is_empty() {
                format!("{stem} ({counter})")
            } else {
                format!("{stem} ({counter}).{ext}")
            };
            target = trash.join(name);
            if !target.exists() {
                break;
            }
            counter += 1;
        }
    }

    if tokio::fs::rename(&src, &target).await.is_err() {
        tokio::fs::copy(&src, &target).await.map_err(err)?;
        tokio::fs::remove_file(&src).await.map_err(err)?;
    }

    {
        let mut conn = st.db.lock().map_err(err)?;
        db::delete_video(&mut conn, id).map_err(err)?;
    }
    let _ = tokio::fs::remove_dir_all(media::thumb_dir(&st, &video)).await;
    let _ = tokio::fs::remove_file(media::play_cache_path(&st, &video)).await;

    Ok(target.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn mark_viewed(state: State<'_, Arc<AppState>>, id: i64) -> Result<i64, String> {
    let st = state.inner().clone();
    let conn = st.db.lock().map_err(err)?;
    db::mark_viewed(&conn, id, scan::now()).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackInfo {
    pub mode: String,
    pub state: String,
    pub url: Option<String>,
    pub progress: f64,
    pub error: Option<String>,
}

fn playback_info(st: &Arc<AppState>, video: &db::VideoDto, start: bool) -> PlaybackInfo {
    let url = format!("{}/media/{}", st.media_base(), video.id);
    let mode = video.playback.clone();
    if mode == "direct" {
        return PlaybackInfo {
            mode,
            state: "ready".into(),
            url: Some(url),
            progress: 1.0,
            error: None,
        };
    }

    let cache = media::play_cache_path(st, video);
    if cache.is_file() {
        return PlaybackInfo {
            mode,
            state: "ready".into(),
            url: Some(url),
            progress: 1.0,
            error: None,
        };
    }
    if st.ffmpeg().is_none() {
        return PlaybackInfo {
            mode,
            state: "error".into(),
            url: None,
            progress: 0.0,
            error: Some(
                "ffmpeg was not found, so this file cannot be converted for playback. \
                 Install ffmpeg (winget install Gyan.FFmpeg) and restart the app."
                    .into(),
            ),
        };
    }
    if start {
        media::spawn_conversion(st.clone(), video.clone());
    }
    let job = st
        .conversions
        .lock()
        .ok()
        .and_then(|jobs| jobs.get(&video.id).cloned());
    match job {
        Some(job) if job.state == "error" => PlaybackInfo {
            mode,
            state: "error".into(),
            url: None,
            progress: 0.0,
            error: job.error,
        },
        Some(job) if job.state == "done" && cache.is_file() => PlaybackInfo {
            mode,
            state: "ready".into(),
            url: Some(url),
            progress: 1.0,
            error: None,
        },
        Some(job) if job.state == "done" => PlaybackInfo {
            mode,
            state: "queued".into(),
            url: None,
            progress: -1.0,
            error: None,
        },
        Some(job) => PlaybackInfo {
            mode,
            state: "working".into(),
            url: None,
            progress: job.progress,
            error: None,
        },
        None => PlaybackInfo {
            mode,
            state: "queued".into(),
            url: None,
            progress: -1.0,
            error: None,
        },
    }
}

#[tauri::command]
pub async fn prepare_playback(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> Result<PlaybackInfo, String> {
    let st = state.inner().clone();
    let video = st.video(id).ok_or("Video not found.")?;
    Ok(playback_info(&st, &video, true))
}

#[tauri::command]
pub async fn playback_status(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> Result<PlaybackInfo, String> {
    let st = state.inner().clone();
    let video = st.video(id).ok_or("Video not found.")?;
    Ok(playback_info(&st, &video, false))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbsResult {
    pub timestamps: Vec<f64>,
    pub ok: usize,
}

#[tauri::command]
pub async fn ensure_thumbs(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> Result<ThumbsResult, String> {
    let st = state.inner().clone();
    let video = st.video(id).ok_or("Video not found.")?;
    let timestamps = media::ensure_thumbs(&st, &video).await;
    let ok = (0..media::THUMB_COUNT)
        .filter(|i| media::thumb_path(&st, &video, *i).is_file())
        .count();
    Ok(ThumbsResult { timestamps, ok })
}

#[tauri::command]
pub async fn all_tags(state: State<'_, Arc<AppState>>) -> Result<Vec<db::TagCount>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::all_tags(&conn).map_err(err)
}

#[tauri::command]
pub async fn creators(state: State<'_, Arc<AppState>>) -> Result<Vec<String>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::creators(&conn).map_err(err)
}

#[tauri::command]
pub async fn reveal(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    let video = state.inner().video(id).ok_or("Video not found.")?;
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", video.path))
            .spawn()
            .map_err(err)?;
    }
    Ok(())
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_dir() {
                    total += dir_size(&entry.path());
                } else {
                    total += meta.len();
                }
            }
        }
    }
    total
}

#[tauri::command]
pub async fn cache_size(state: State<'_, Arc<AppState>>) -> Result<u64, String> {
    let dir = state.inner().cache_dir.clone();
    tokio::task::spawn_blocking(move || dir_size(&dir))
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn clear_cache(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let st = state.inner().clone();
    let thumbs = st.cache_dir.join("thumbs");
    let play = st.cache_dir.join("play");
    let _ = tokio::fs::remove_dir_all(&thumbs).await;
    let _ = tokio::fs::remove_dir_all(&play).await;
    let _ = tokio::fs::create_dir_all(&thumbs).await;
    let _ = tokio::fs::create_dir_all(&play).await;
    st.conversions.lock().unwrap().clear();
    Ok(())
}

#[tauri::command]
pub async fn set_ffmpeg_dir(
    state: State<'_, Arc<AppState>>,
    path: Option<String>,
) -> Result<bool, String> {
    let st = state.inner().clone();
    match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => st.set_setting("ffmpeg_dir", p.trim())?,
        None => {
            let conn = st.db.lock().map_err(err)?;
            conn.execute("DELETE FROM settings WHERE key = 'ffmpeg_dir'", [])
                .map_err(err)?;
        }
    }
    st.refresh_ffmpeg();
    Ok(st.ffmpeg().is_some())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataDirInfo {
    pub data_dir: String,
    pub db_path: String,
    pub backup: Option<String>,
}

/// Move the library database to another folder. The current file is left in
/// place as a backup; an existing `library.db` at the target is renamed aside.
#[tauri::command]
pub async fn set_data_dir(
    state: State<'_, Arc<AppState>>,
    path: Option<String>,
) -> Result<DataDirInfo, String> {
    let st = state.inner().clone();
    if st.scan.lock().map_err(err)?.running {
        return Err("Wait for the current scan to finish before moving the database.".into());
    }

    let target = match path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => Path::new(p).components().collect::<PathBuf>(),
        None => st.default_data_dir.clone(),
    };
    let current = st.data_dir();
    if target == current {
        return Ok(DataDirInfo {
            data_dir: current.to_string_lossy().to_string(),
            db_path: st.db_path().to_string_lossy().to_string(),
            backup: None,
        });
    }

    tokio::fs::create_dir_all(&target)
        .await
        .map_err(|e| format!("Could not create {}: {e}", target.display()))?;
    if !target.is_dir() {
        return Err("That path is not a folder.".into());
    }

    let src = st.db_path();
    if !src.is_file() {
        return Err(format!(
            "Current database file is missing: {}",
            src.display()
        ));
    }

    let target_db = target.join("library.db");
    let mut backup = None;
    if target_db.exists() {
        let stamp = scan::now();
        let dest = target.join(format!("library.db.bak-{stamp}"));
        tokio::fs::rename(&target_db, &dest).await.map_err(|e| {
            format!(
                "Could not move the existing {} aside: {e}",
                target_db.display()
            )
        })?;
        backup = Some(dest.to_string_lossy().to_string());
    }
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = target_db.clone().into_os_string();
        sidecar.push(suffix);
        let _ = tokio::fs::remove_file(PathBuf::from(sidecar)).await;
    }

    let db = st.db.clone();
    let src_copy = src.clone();
    let target_copy = target_db.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut guard = db.lock().map_err(|e| e.to_string())?;
        guard
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())?;
        std::fs::copy(&src_copy, &target_copy)
            .map_err(|e| format!("Could not copy the database: {e}"))?;
        let conn = db::open(&target_copy)
            .map_err(|e| format!("Could not open the copied database: {e}"))?;
        *guard = conn;
        Ok(())
    })
    .await
    .map_err(err)??;

    // The old connection is gone; its WAL/SHM files are empty leftovers.
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = src.clone().into_os_string();
        sidecar.push(suffix);
        let _ = tokio::fs::remove_file(PathBuf::from(sidecar)).await;
    }

    *st.db_path.write().unwrap() = target_db.clone();
    *st.data_dir.write().unwrap() = target.clone();
    *st.data_warning.write().unwrap() = None;

    let is_default = target == st.default_data_dir;
    config::write(
        &st.config_path,
        &config::Config {
            data_dir: if is_default {
                None
            } else {
                Some(target.to_string_lossy().to_string())
            },
        },
    )
    .map_err(|e| format!("Database moved, but the setting could not be saved: {e}"))?;

    Ok(DataDirInfo {
        data_dir: target.to_string_lossy().to_string(),
        db_path: target_db.to_string_lossy().to_string(),
        backup,
    })
}
