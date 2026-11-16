use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use serde::Serialize;

use crate::{db, media, AppState};

#[derive(Clone, Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DnaStatus {
    pub running: bool,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub current: String,
    pub error: Option<String>,
    pub finished_at: Option<i64>,
    #[serde(skip)]
    pub cancel: Arc<AtomicBool>,
}

fn update(state: &AppState, f: impl FnOnce(&mut DnaStatus)) {
    if let Ok(mut status) = state.dna.lock() {
        f(&mut status);
    }
}

pub fn spawn(state: Arc<AppState>) {
    {
        let mut status = state.dna.lock().unwrap();
        if status.running {
            return;
        }
        *status = DnaStatus {
            running: true,
            cancel: Arc::new(AtomicBool::new(false)),
            ..Default::default()
        };
    }
    tauri::async_runtime::spawn(async move { run(state).await });
}

async fn run(state: Arc<AppState>) {
    let ffmpeg = match state.ffmpeg() {
        Some(ffmpeg) => ffmpeg,
        None => {
            fail(&state, "ffmpeg is required to analyze videos.".into());
            return;
        }
    };

    let pending = {
        let conn = match state.db.lock() {
            Ok(conn) => conn,
            Err(err) => {
                fail(&state, err.to_string());
                return;
            }
        };
        match db::dna_pending(&conn) {
            Ok(pending) => pending,
            Err(err) => {
                fail(&state, format!("{err:#}"));
                return;
            }
        }
    };

    let total = pending.len();
    update(&state, |status| status.total = total);
    if total == 0 {
        finish(&state);
        return;
    }

    let cancel = state.dna.lock().unwrap().cancel.clone();
    let mut jobs = tokio::task::JoinSet::new();
    let mut done = 0usize;
    let mut failed = 0usize;

    for (id, path, duration, width, height) in pending {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        while jobs.len() >= 3 {
            if let Some(joined) = jobs.join_next().await {
                if !record(&state, joined) {
                    failed += 1;
                }
                done += 1;
                update(&state, |status| status.done = done);
            }
        }
        let ffmpeg = ffmpeg.clone();
        jobs.spawn(async move {
            let result = media::compute_dna(&ffmpeg.ffmpeg, Path::new(&path), duration).await;
            (id, path, duration, width, height, result)
        });
    }

    while let Some(joined) = jobs.join_next().await {
        if !record(&state, joined) {
            failed += 1;
        }
        done += 1;
        update(&state, |status| status.done = done);
    }
    if cancel.load(Ordering::SeqCst) {
        jobs.abort_all();
    }

    update(&state, |status| {
        status.failed = failed;
        status.running = false;
        status.current.clear();
        status.finished_at = Some(crate::scan::now());
    });
}

type DnaResult = (
    i64,
    String,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    anyhow::Result<Vec<u64>>,
);

fn record(state: &AppState, joined: Result<DnaResult, tokio::task::JoinError>) -> bool {
    let Ok((id, path, duration, width, height, result)) = joined else {
        return false;
    };
    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    update(state, |status| status.current = name);

    match result {
        Ok(frames) => {
            let encoded = frames
                .iter()
                .map(|hash| format!("{hash:016x}"))
                .collect::<Vec<_>>()
                .join(",");
            if let Ok(conn) = state.db.lock() {
                let _ = db::set_dna(
                    &conn,
                    id,
                    &encoded,
                    duration,
                    width,
                    height,
                    crate::scan::now(),
                );
                true
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

fn finish(state: &AppState) {
    update(state, |status| {
        status.running = false;
        status.current.clear();
        status.finished_at = Some(crate::scan::now());
    });
}

fn fail(state: &AppState, message: String) {
    update(state, |status| {
        status.running = false;
        status.error = Some(message);
        status.current.clear();
        status.finished_at = Some(crate::scan::now());
    });
}
