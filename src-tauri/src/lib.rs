pub mod commands;
pub mod config;
pub mod db;
pub mod dna;
pub mod media;
pub mod scan;
pub mod server;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
};

use serde::Serialize;
use tauri::Manager;

#[derive(Clone, Debug)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub running: bool,
    pub phase: String,
    pub total: usize,
    pub done: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub failed: usize,
    pub current: String,
    pub error: Option<String>,
    pub eta_seconds: Option<u64>,
    pub finished_at: Option<i64>,
    #[serde(skip)]
    pub phase_started: Option<std::time::Instant>,
    #[serde(skip)]
    pub cancel: Arc<AtomicBool>,
}

impl Default for ScanStatus {
    fn default() -> Self {
        Self {
            running: false,
            phase: String::new(),
            total: 0,
            done: 0,
            added: 0,
            updated: 0,
            removed: 0,
            failed: 0,
            current: String::new(),
            error: None,
            eta_seconds: None,
            finished_at: None,
            phase_started: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Conversion {
    pub state: String,
    pub progress: f64,
    pub error: Option<String>,
}

pub struct AppState {
    pub db: Arc<Mutex<rusqlite::Connection>>,
    pub db_path: RwLock<PathBuf>,
    /// Folder that holds library.db (may be a user-chosen location).
    pub data_dir: RwLock<PathBuf>,
    /// Where the config file lives, and the fallback database folder.
    pub default_data_dir: PathBuf,
    pub config_path: PathBuf,
    pub cache_dir: PathBuf,
    pub data_warning: RwLock<Option<String>>,
    pub root: RwLock<Option<PathBuf>>,
    pub ffmpeg: RwLock<Option<Ffmpeg>>,
    pub media_base: RwLock<String>,
    pub scan: Arc<Mutex<ScanStatus>>,
    pub dna: Arc<Mutex<dna::DnaStatus>>,
    pub conversions: Arc<Mutex<HashMap<i64, Conversion>>>,
    pub thumb_locks: Arc<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>>,
    pub convert_permits: Arc<tokio::sync::Semaphore>,
    pub thumb_permits: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(
        conn: rusqlite::Connection,
        db_path: PathBuf,
        data_dir: PathBuf,
        default_data_dir: PathBuf,
        config_path: PathBuf,
        cache_dir: PathBuf,
        data_warning: Option<String>,
        root: Option<PathBuf>,
    ) -> Self {
        std::fs::create_dir_all(&cache_dir).ok();
        let state = Self {
            db: Arc::new(Mutex::new(conn)),
            db_path: RwLock::new(db_path),
            data_dir: RwLock::new(data_dir),
            default_data_dir,
            config_path,
            cache_dir,
            data_warning: RwLock::new(data_warning),
            root: RwLock::new(root),
            ffmpeg: RwLock::new(None),
            media_base: RwLock::new(String::new()),
            scan: Arc::new(Mutex::new(ScanStatus::default())),
            dna: Arc::new(Mutex::new(dna::DnaStatus::default())),
            conversions: Arc::new(Mutex::new(HashMap::new())),
            thumb_locks: Arc::new(Mutex::new(HashMap::new())),
            convert_permits: Arc::new(tokio::sync::Semaphore::new(1)),
            thumb_permits: Arc::new(tokio::sync::Semaphore::new(3)),
        };
        state.refresh_ffmpeg();
        state
    }

    pub fn db_path(&self) -> PathBuf {
        self.db_path.read().unwrap().clone()
    }

    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.read().unwrap().clone()
    }

    pub fn data_warning(&self) -> Option<String> {
        self.data_warning.read().unwrap().clone()
    }

    pub fn ffmpeg(&self) -> Option<Ffmpeg> {
        self.ffmpeg.read().unwrap().clone()
    }

    pub fn refresh_ffmpeg(&self) {
        let override_dir = self.setting("ffmpeg_dir");
        let found = media::detect_ffmpeg(override_dir.as_deref());
        *self.ffmpeg.write().unwrap() = found;
    }

    pub fn media_base(&self) -> String {
        self.media_base.read().unwrap().clone()
    }

    pub fn root_path(&self) -> Option<PathBuf> {
        self.root.read().unwrap().clone()
    }

    pub fn setting(&self, key: &str) -> Option<String> {
        let conn = self.db.lock().ok()?;
        db::get_setting(&conn, key)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.db.lock().map_err(|e| e.to_string())?;
        db::set_setting(&conn, key, value).map_err(|e| e.to_string())
    }

    pub fn video(&self, id: i64) -> Option<db::VideoDto> {
        let conn = self.db.lock().ok()?;
        db::get_video(&conn, id).ok().flatten()
    }

    pub fn thumb_lock(&self, id: i64) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.thumb_locks.lock().unwrap();
        map.entry(id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    pub fn cancel_scan(&self) {
        let s = self.scan.lock().unwrap();
        s.cancel.store(true, Ordering::SeqCst);
    }
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let default_data_dir = app.path().app_data_dir()?;
            let cache_dir = app.path().app_cache_dir()?;
            migrate_legacy_dirs(&default_data_dir, &cache_dir);
            std::fs::create_dir_all(&default_data_dir)?;
            std::fs::create_dir_all(&cache_dir)?;

            let config_path = default_data_dir.join("config.json");
            let configured = config::read(&config_path)
                .data_dir
                .map(|p| Path::new(&p).components().collect::<PathBuf>());
            let mut warning = None;

            let (conn, data_dir, db_path) = match configured {
                Some(dir) if !dir.is_dir() => {
                    warning = Some(format!(
                        "Database folder is not available: {}. Using the default location for now.",
                        dir.display()
                    ));
                    let path = default_data_dir.join("library.db");
                    (db::open(&path)?, default_data_dir.clone(), path)
                }
                Some(dir) => {
                    let path = dir.join("library.db");
                    match db::open(&path) {
                        Ok(conn) => (conn, dir, path),
                        Err(err) => {
                            warning = Some(format!(
                                "Could not open the database at {} ({err}). Using the default location.",
                                path.display()
                            ));
                            let fallback = default_data_dir.join("library.db");
                            (db::open(&fallback)?, default_data_dir.clone(), fallback)
                        }
                    }
                }
                None => {
                    let path = default_data_dir.join("library.db");
                    (db::open(&path)?, default_data_dir.clone(), path)
                }
            };

            if let Err(err) = db::normalize_stored_paths(&conn) {
                eprintln!("could not normalize stored paths: {err}");
            }
            let stored_root = db::get_setting(&conn, "root");
            if let Some(stored) = stored_root.as_deref() {
                if let Err(err) = db::backfill_rel_paths(&conn, stored) {
                    eprintln!("could not backfill relative paths: {err}");
                }
            }
            let root = stored_root
                .map(PathBuf::from)
                .filter(|p| p.is_dir());
            let state = Arc::new(AppState::new(
                conn,
                db_path,
                data_dir,
                default_data_dir,
                config_path,
                cache_dir,
                warning,
                root,
            ));

            // Bind the local media server synchronously so the port is known before
            // the UI asks for it, then hand it to the async runtime.
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
            listener.set_nonblocking(true)?;
            let port = listener.local_addr()?.port();
            *state.media_base.write().unwrap() = format!("http://127.0.0.1:{}", port);
            server::spawn(state.clone(), listener);
            let cleanup = state.clone();
            tauri::async_runtime::spawn_blocking(move || {
                media::enforce_cache_limit(&cleanup);
            });
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::pick_folder,
            commands::set_root,
            commands::start_scan,
            commands::scan_status,
            commands::cancel_scan,
            commands::stats,
            commands::list_videos,
            commands::get_video,
            commands::save_video,
            commands::rating_queue,
            commands::duplicates,
            commands::remove_duplicate,
            commands::trash_video,
            commands::mark_viewed,
            commands::prepare_playback,
            commands::playback_status,
            commands::ensure_thumbs,
            commands::all_tags,
            commands::all_toys,
            commands::rename_toy,
            commands::delete_toy,
            commands::creators,
            commands::reveal,
            commands::cache_size,
            commands::clear_cache,
            commands::set_ffmpeg_dir,
            commands::set_data_dir,
            commands::folders,
            commands::bulk_update,
            commands::rename_tag,
            commands::delete_tag,
            commands::trash_list,
            commands::restore_trash,
            commands::delete_trash,
            commands::set_hash_mode,
            commands::purge_missing,
            commands::dismiss_ffmpeg_prompt,
            commands::check_ffmpeg,
            commands::set_flag_visible,
            commands::set_cache_limit,
            commands::clean_cache,
            commands::start_dna,
            commands::dna_status,
            commands::cancel_dna,
            commands::similar_duplicates,
            commands::backup_database,
            commands::export_library,
            commands::import_library,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// One-time move of the library database and generated cache from the app's
/// previous identifier, so renaming the product does not lose an indexed
/// collection.
fn migrate_legacy_dirs(data_dir: &std::path::Path, cache_dir: &std::path::Path) {
    const LEGACY_IDENTIFIER: &str = "com.inflatool.desktop";

    // Library database (roaming app data).
    if let Some(parent) = data_dir.parent() {
        let legacy = parent.join(LEGACY_IDENTIFIER);
        if legacy.join("library.db").is_file() && !data_dir.join("library.db").exists() {
            let _ = std::fs::create_dir_all(data_dir);
            for file in ["library.db", "library.db-wal", "library.db-shm"] {
                let from = legacy.join(file);
                if from.is_file() {
                    let _ = std::fs::rename(&from, data_dir.join(file));
                }
            }
        }
    }

    // Generated preview/conversion cache (local app data). This folder also
    // holds the WebView2 profile, so only move our own subfolders.
    if let Some(parent) = cache_dir.parent() {
        let legacy = parent.join(LEGACY_IDENTIFIER);
        for sub in ["thumbs", "play"] {
            let from = legacy.join(sub);
            let to = cache_dir.join(sub);
            if from.is_dir() && !to.exists() {
                let _ = std::fs::create_dir_all(cache_dir);
                if std::fs::rename(&from, &to).is_err() {
                    let _ = copy_dir(&from, &to);
                }
            }
        }
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Headless indexing pass, used for development verification:
/// `splashhound.exe --scan <folder>`
pub fn headless_scan(dir: PathBuf, serve: bool) {
    let base = std::env::temp_dir().join("splashhound-headless");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let db_path = base.join("library.db");
    let conn = db::open(&db_path).unwrap();
    let state = Arc::new(AppState::new(
        conn,
        db_path,
        base.clone(),
        base.clone(),
        base.join("config.json"),
        base.join("cache"),
        None,
        Some(dir.clone()),
    ));
    println!("ffmpeg: {:?}", state.ffmpeg());
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(scan::run_scan(state.clone()));
    let status = state.scan.lock().unwrap().clone();
    println!(
        "scan: phase={} total={} done={} added={} updated={} removed={} failed={} error={:?}",
        status.phase,
        status.total,
        status.done,
        status.added,
        status.updated,
        status.removed,
        status.failed,
        status.error
    );
    let conn = state.db.lock().unwrap();
    let stats = db::stats(&conn).unwrap();
    println!("stats: {:?}", stats);
    let filter = db::VideoFilter {
        limit: 100,
        sort: "name_asc".into(),
        ..Default::default()
    };
    let (total, items) = db::list_videos(&conn, &filter).unwrap();
    println!("videos: {}", total);
    for v in &items {
        println!(
            "  #{} {} | {:.1}s | {}x{} | {} / {} | md5={:?} | {} | play={}",
            v.id,
            v.file_name,
            v.duration_ms.unwrap_or(0) as f64 / 1000.0,
            v.width.unwrap_or(0),
            v.height.unwrap_or(0),
            v.vcodec.as_deref().unwrap_or("?"),
            v.acodec.as_deref().unwrap_or("-"),
            v.md5,
            v.size,
            v.playback
        );
    }
    for v in &items {
        let times = rt.block_on(media::ensure_thumbs(&state, v));
        println!("  thumbs #{} -> {:?}", v.id, times);
    }
    // Exercise the conversion pipeline on every non-direct file.
    drop(conn);
    for v in &items {
        if v.playback == "direct" {
            continue;
        }
        let out = media::play_cache_path(&state, v);
        let started = std::time::Instant::now();
        match rt.block_on(media::run_conversion(&state, v, &out)) {
            Ok(()) => {
                let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
                println!(
                    "  conversion #{} ({}) -> ok in {:.1}s, {:.1} MB",
                    v.id,
                    v.playback,
                    started.elapsed().as_secs_f64(),
                    size as f64 / 1e6
                );
            }
            Err(e) => println!("  conversion #{} ({}) -> FAILED: {e:#}", v.id, v.playback),
        }
    }
    let conn = state.db.lock().unwrap();
    let dups = db::duplicates(&conn).unwrap();
    println!("duplicate groups: {}", dups.len());
    for g in &dups {
        println!("  {} ({} files)", g.md5, g.videos.len());
    }
    drop(conn);

    if serve {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        *state.media_base.write().unwrap() = format!("http://127.0.0.1:{}", port);
        println!("MEDIA_BASE=http://127.0.0.1:{}", port);
        use std::io::Write;
        std::io::stdout().flush().ok();
        let app = server::router(state.clone());
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let _ = axum::serve(listener, app).await;
        });
    }
}
