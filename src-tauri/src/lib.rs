pub mod commands;
pub mod db;
pub mod media;
pub mod scan;
pub mod server;

use std::{
    collections::HashMap,
    path::PathBuf,
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
    pub finished_at: Option<i64>,
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
            finished_at: None,
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
    pub db_path: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub root: RwLock<Option<PathBuf>>,
    pub ffmpeg: RwLock<Option<Ffmpeg>>,
    pub media_base: RwLock<String>,
    pub scan: Arc<Mutex<ScanStatus>>,
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
        cache_dir: PathBuf,
        root: Option<PathBuf>,
    ) -> Self {
        std::fs::create_dir_all(&cache_dir).ok();
        let state = Self {
            db: Arc::new(Mutex::new(conn)),
            db_path,
            data_dir,
            cache_dir,
            root: RwLock::new(root),
            ffmpeg: RwLock::new(None),
            media_base: RwLock::new(String::new()),
            scan: Arc::new(Mutex::new(ScanStatus::default())),
            conversions: Arc::new(Mutex::new(HashMap::new())),
            thumb_locks: Arc::new(Mutex::new(HashMap::new())),
            convert_permits: Arc::new(tokio::sync::Semaphore::new(1)),
            thumb_permits: Arc::new(tokio::sync::Semaphore::new(3)),
        };
        state.refresh_ffmpeg();
        state
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
            let data_dir = app.path().app_data_dir()?;
            let cache_dir = app.path().app_cache_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            std::fs::create_dir_all(&cache_dir)?;
            let db_path = data_dir.join("library.db");
            let conn = db::open(&db_path)?;
            let root = db::get_setting(&conn, "root")
                .map(PathBuf::from)
                .filter(|p| p.is_dir());
            let state = Arc::new(AppState::new(conn, db_path, data_dir, cache_dir, root));

            // Bind the local media server synchronously so the port is known before
            // the UI asks for it, then hand it to the async runtime.
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
            listener.set_nonblocking(true)?;
            let port = listener.local_addr()?.port();
            *state.media_base.write().unwrap() = format!("http://127.0.0.1:{}", port);
            server::spawn(state.clone(), listener);
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
            commands::mark_viewed,
            commands::prepare_playback,
            commands::playback_status,
            commands::ensure_thumbs,
            commands::all_tags,
            commands::creators,
            commands::reveal,
            commands::cache_size,
            commands::clear_cache,
            commands::set_ffmpeg_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Headless indexing pass, used for development verification:
/// `inflatool.exe --scan <folder>`
pub fn headless_scan(dir: PathBuf, serve: bool) {
    let base = std::env::temp_dir().join("inflatool-headless");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let db_path = base.join("library.db");
    let conn = db::open(&db_path).unwrap();
    let state = Arc::new(AppState::new(
        conn,
        db_path,
        base.clone(),
        base.join("cache"),
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
