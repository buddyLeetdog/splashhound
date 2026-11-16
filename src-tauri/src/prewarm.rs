//! Background generation of grid posters.
//!
//! The grid requests one poster per card. On a mechanical disk a cold poster
//! costs ~100-140 ms and a full page of 60 makes the browser queue them, so the
//! cards stay blank for a while. Generating them after a scan (or on demand)
//! moves that cost out of the scroll.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use serde::Serialize;

use crate::{db, media, scan, AppState};

/// Poster shown on library cards (middle frame).
pub const POSTER_INDEX: usize = media::THUMB_COUNT / 2;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub total: usize,
    pub done: usize,
    pub generated: usize,
    pub failed: usize,
    pub finished_at: Option<i64>,
    #[serde(skip)]
    pub cancel: Arc<AtomicBool>,
}

pub fn new_state() -> Arc<Mutex<Status>> {
    Arc::new(Mutex::new(Status::default()))
}

pub fn snapshot(state: &AppState) -> Status {
    state.prewarm.lock().map(|s| s.clone()).unwrap_or_default()
}

pub fn cancel(state: &AppState) {
    if let Ok(status) = state.prewarm.lock() {
        status.cancel.store(true, Ordering::SeqCst);
    }
}

/// Start in the background. Returns false when a run is already in progress.
pub fn start(state: Arc<AppState>) -> bool {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let Ok(mut status) = state.prewarm.lock() else {
            return false;
        };
        if status.running {
            return false;
        }
        *status = Status {
            running: true,
            cancel: cancel.clone(),
            ..Default::default()
        };
    }
    tauri::async_runtime::spawn(async move { run(state, cancel).await });
    true
}

async fn run(state: Arc<AppState>, cancel: Arc<AtomicBool>) {
    let (videos, have_lowres) = {
        let Ok(conn) = state.db.lock() else {
            finish(&state);
            return;
        };
        (
            db::all_videos(&conn).unwrap_or_default(),
            db::lowres_ids(&conn).unwrap_or_default(),
        )
    };
    // Only the ones without a cached poster or without the placeholder image.
    let pending: Vec<_> = videos
        .into_iter()
        .filter(|v| {
            !media::thumb_path(&state, v, POSTER_INDEX).is_file() || !have_lowres.contains(&v.id)
        })
        .collect();

    if let Ok(mut status) = state.prewarm.lock() {
        status.total = pending.len();
    }
    for video in pending {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let made = media::ensure_thumb(&state, &video, POSTER_INDEX)
            .await
            .is_some();
        // Backfills the placeholder for posters that were cached before this
        // feature existed.
        let small = media::ensure_lowres(&state, &video).await;
        if let Ok(mut status) = state.prewarm.lock() {
            status.done += 1;
            if made || small {
                status.generated += 1;
            } else {
                status.failed += 1;
            }
        }
    }
    finish(&state);
}

fn finish(state: &AppState) {
    if let Ok(mut status) = state.prewarm.lock() {
        status.running = false;
        status.finished_at = Some(scan::now());
    }
}
