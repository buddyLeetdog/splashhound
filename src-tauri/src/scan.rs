use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use walkdir::WalkDir;

use crate::{db, media, AppState, ScanStatus};

pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "wmv", "flv", "m4v", "mpg", "mpeg", "ts", "m2ts", "mts",
    "ogv", "ogg", "3gp", "asf", "divx", "vob",
];

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn spawn(state: Arc<AppState>) {
    {
        let mut s = state.scan.lock().unwrap();
        if s.running {
            return;
        }
        *s = ScanStatus {
            running: true,
            phase: "starting".into(),
            ..Default::default()
        };
    }
    tauri::async_runtime::spawn(async move { run_scan(state).await });
}

fn update(state: &AppState, f: impl FnOnce(&mut ScanStatus)) {
    if let Ok(mut s) = state.scan.lock() {
        f(&mut s);
    }
}

struct FileEntry {
    path: String,
    file_name: String,
    ext: String,
    size: i64,
    mtime: i64,
}

fn collect_files(root: &Path) -> Vec<FileEntry> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let skip = path.components().any(|c| {
            let name = c.as_os_str().to_string_lossy().to_lowercase();
            name == "_trash" || name == ".git" || name.starts_with('$')
        });
        if skip {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !VIDEO_EXTS.contains(&ext.as_str()) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(FileEntry {
            path: path.to_string_lossy().to_string(),
            file_name: path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            ext,
            size: meta.len() as i64,
            mtime,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

pub async fn run_scan(state: Arc<AppState>) {
    let root = match state.root_path() {
        Some(root) if root.is_dir() => root,
        Some(root) => {
            fail(&state, format!("Library folder does not exist: {}", root.display()));
            return;
        }
        None => {
            fail(&state, "No library folder selected yet.".into());
            return;
        }
    };
    let cancel = state.scan.lock().unwrap().cancel.clone();

    let result: Result<()> = async {
        // 1. Walk the tree --------------------------------------------------
        update(&state, |s| s.phase = "walking".into());
        let root2 = root.clone();
        let files = tokio::task::spawn_blocking(move || collect_files(&root2)).await?;
        update(&state, |s| {
            s.phase = "indexing".into();
            s.total = files.len();
            s.done = 0;
        });

        // 2. Insert / update rows ------------------------------------------
        let now_ts = now();
        let mut seen: HashSet<String> = HashSet::with_capacity(files.len());
        let mut added = 0usize;
        let mut updated = 0usize;
        let mut failed = 0usize;
        for (i, f) in files.iter().enumerate() {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            seen.insert(f.path.clone());
            let res = {
                let conn = state.db.lock().map_err(|e| anyhow!(e.to_string()))?;
                db::upsert_file(&conn, &f.path, &f.file_name, &f.ext, f.size, f.mtime, now_ts)
            };
            match res {
                Ok(u) => {
                    if u.is_new {
                        added += 1;
                    } else if u.changed {
                        updated += 1;
                    }
                }
                Err(_) => failed += 1,
            }
            if i % 10 == 0 || i + 1 == files.len() {
                update(&state, |s| {
                    s.done = i + 1;
                    s.added = added;
                    s.updated = updated;
                    s.failed = failed;
                    s.current = f.file_name.clone();
                });
            }
        }

        // 3. Flag vanished files -------------------------------------------
        if !cancel.load(Ordering::SeqCst) {
            let removed = {
                let conn = state.db.lock().map_err(|e| anyhow!(e.to_string()))?;
                db::mark_missing(&conn, &seen)?
            };
            update(&state, |s| s.removed = removed);
        }

        // 4. Probe metadata -------------------------------------------------
        let pending = {
            let conn = state.db.lock().map_err(|e| anyhow!(e.to_string()))?;
            db::pending_metadata(&conn)?
        };
        update(&state, |s| {
            s.phase = "probing".into();
            s.total = pending.len();
            s.done = 0;
        });

        match state.ffmpeg() {
            Some(ff) => {
                for (i, (id, path)) in pending.iter().enumerate() {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let name = Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());
                    update(&state, |s| {
                        s.done = i;
                        s.current = name.clone();
                    });
                    match media::probe(&ff.ffprobe, Path::new(path)).await {
                        Ok(info) => {
                            if let Ok(conn) = state.db.lock() {
                                let _ = db::set_metadata(&conn, *id, &info);
                            }
                        }
                        Err(_) => {
                            if let Ok(conn) = state.db.lock() {
                                let _ = db::mark_probe_failed(&conn, *id);
                            }
                        }
                    }
                }
                update(&state, |s| s.done = pending.len());
            }
            None => {
                update(&state, |s| {
                    s.error = Some(
                        "ffprobe was not found, so durations and codecs could not be read. \
                         Install ffmpeg (winget install Gyan.FFmpeg) and scan again."
                            .into(),
                    )
                });
            }
        }

        // 5. Hash files -----------------------------------------------------
        let pending = {
            let conn = state.db.lock().map_err(|e| anyhow!(e.to_string()))?;
            db::pending_hashes(&conn)?
        };
        update(&state, |s| {
            s.phase = "hashing".into();
            s.total = pending.len();
            s.done = 0;
        });
        for (i, (id, path)) in pending.iter().enumerate() {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            let name = Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            update(&state, |s| {
                s.done = i;
                s.current = name.clone();
            });
            let p = PathBuf::from(path);
            let hash = tokio::task::spawn_blocking(move || media::hash_file(&p)).await?;
            if let Ok(hash) = hash {
                if let Ok(conn) = state.db.lock() {
                    let _ = db::set_md5(&conn, *id, &hash);
                }
            }
        }
        update(&state, |s| s.done = pending.len());

        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            update(&state, |s| {
                s.running = false;
                s.phase = "done".into();
                s.current.clear();
                s.finished_at = Some(now());
            });
        }
        Err(e) => fail(&state, format!("{e:#}")),
    }
}

fn fail(state: &AppState, message: String) {
    update(state, |s| {
        s.running = false;
        s.phase = "error".into();
        s.current.clear();
        s.error = Some(message);
        s.finished_at = Some(now());
    });
}
