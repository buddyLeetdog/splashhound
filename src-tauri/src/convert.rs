//! Batch conversion of files the player cannot decode.
//!
//! Two targets: `cache` fills the playback cache (nothing on disk changes),
//! `file` writes a real MP4 next to the original and indexes it in the library.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use serde::Serialize;

use crate::{commands, db, media, scan, AppState};

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    /// "cache" or "file"
    pub target: String,
    pub total: usize,
    pub done: usize,
    pub ok: usize,
    pub failed: usize,
    pub current: String,
    pub current_id: Option<i64>,
    pub progress: f64,
    pub errors: Vec<String>,
    pub finished_at: Option<i64>,
    #[serde(skip)]
    pub cancel: Arc<AtomicBool>,
}

pub fn new_state() -> Arc<Mutex<Status>> {
    Arc::new(Mutex::new(Status::default()))
}

/// Status snapshot, with the live progress of the file being converted.
pub fn snapshot(state: &AppState) -> Status {
    let mut status = match state.convert.lock() {
        Ok(s) => s.clone(),
        Err(_) => Status::default(),
    };
    if let Some(id) = status.current_id {
        if let Ok(jobs) = state.conversions.lock() {
            if let Some(job) = jobs.get(&id) {
                if job.progress > 0.0 {
                    status.progress = job.progress;
                }
            }
        }
    }
    status
}

pub fn cancel(state: &AppState) {
    if let Ok(status) = state.convert.lock() {
        status.cancel.store(true, Ordering::SeqCst);
    }
}

pub fn start(
    state: Arc<AppState>,
    ids: Vec<i64>,
    target: String,
    trash_source: bool,
) -> Result<(), String> {
    if ids.is_empty() {
        return Err("Select at least one file first.".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut status = state.convert.lock().map_err(|e| e.to_string())?;
        if status.running {
            return Err("A conversion is already running.".into());
        }
        *status = Status {
            running: true,
            target: target.clone(),
            total: ids.len(),
            cancel: cancel.clone(),
            ..Default::default()
        };
    }
    tauri::async_runtime::spawn(async move { run(state, ids, target, trash_source, cancel).await });
    Ok(())
}

async fn run(
    state: Arc<AppState>,
    ids: Vec<i64>,
    target: String,
    trash_source: bool,
    cancel: Arc<AtomicBool>,
) {
    for id in ids {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let Some(video) = state.video(id) else {
            bump(
                &state,
                false,
                format!("video {id} is no longer in the library"),
            );
            continue;
        };
        set_current(&state, id, &video.file_name);

        let result = if target == "cache" {
            let out = media::play_cache_path(&state, &video);
            media::run_conversion(&state, &video, &out)
                .await
                .map_err(|e| format!("{e:#}"))
        } else {
            convert_one(&state, &video).await
        };

        match result {
            Ok(()) => {
                if target != "cache" && trash_source {
                    if let Err(e) = commands::trash_by_id(&state, id).await {
                        bump(
                            &state,
                            true,
                            format!(
                                "{} was converted, but moving the original to trash failed: {e}",
                                video.file_name
                            ),
                        );
                        continue;
                    }
                }
                bump(&state, true, String::new());
            }
            Err(e) => bump(&state, false, format!("{}: {e}", video.file_name)),
        }
    }

    if let Ok(mut status) = state.convert.lock() {
        status.running = false;
        status.current.clear();
        status.current_id = None;
        status.progress = 1.0;
        status.finished_at = Some(scan::now());
    }
    if target == "cache" {
        media::enforce_cache_limit(&state);
    }
}

/// Transcode one video to a real MP4 and add it to the library.
async fn convert_one(state: &Arc<AppState>, video: &db::VideoDto) -> Result<(), String> {
    let dest = target_path(&video.path);
    media::convert_to_file(state, video, &dest)
        .await
        .map_err(|e| format!("{e:#}"))?;

    let ffmpeg = state
        .ffmpeg()
        .ok_or_else(|| "ffmpeg was not found".to_string())?;
    let probe = media::probe(&ffmpeg.ffprobe, &dest)
        .await
        .map_err(|e| format!("could not read the converted file: {e:#}"))?;
    let root = state.root_path().map(|r| r.to_string_lossy().to_string());
    let mut conn = state.db.lock().map_err(|e| e.to_string())?;
    db::import_file(
        &mut conn,
        &dest.to_string_lossy(),
        root.as_deref(),
        &probe,
        Some(video.id),
    )
    .map_err(|e| format!("could not add the converted file to the library: {e:#}"))?;
    Ok(())
}

/// `<dir>/<stem>.mp4`, never overwriting the source or an existing conversion.
fn target_path(src: &str) -> PathBuf {
    let path = Path::new(src);
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "video".into());
    let mut candidate = dir.join(format!("{stem}.mp4"));
    if candidate == path {
        candidate = dir.join(format!("{stem} (converted).mp4"));
    }
    let mut counter = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} (converted {counter}).mp4"));
        counter += 1;
    }
    candidate
}

fn set_current(state: &AppState, id: i64, name: &str) {
    if let Ok(mut status) = state.convert.lock() {
        status.current_id = Some(id);
        status.current = name.to_string();
        status.progress = 0.0;
    }
}

fn bump(state: &AppState, ok: bool, error: String) {
    if let Ok(mut status) = state.convert.lock() {
        status.done += 1;
        if ok {
            status.ok += 1;
        } else {
            status.failed += 1;
        }
        if !error.is_empty() && status.errors.len() < 25 {
            status.errors.push(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::target_path;

    #[test]
    fn conversion_target_never_overwrites_the_source() {
        let target = target_path(r"D:\Lib\clip.wmv");
        assert_eq!(target, std::path::PathBuf::from(r"D:\Lib\clip.mp4"));
    }
}
