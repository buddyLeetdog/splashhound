use std::{path::Path, path::PathBuf, sync::Arc};

use rusqlite::{params, OptionalExtension};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::{
    config, convert, db, dna, media, prewarm, scan, stream, suggest, AppState, ScanStatus,
};

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
    pub ffmpeg_prompt_shown: bool,
    pub visible_flags: VisibleFlags,
    pub discreet: bool,
    pub hash_mode: String,
    pub cache_limit_mb: u64,
    pub convert_quality: String,
    pub prewarm_posters: bool,
    pub native_player: bool,
    pub stream_window: u64,
    pub stats: db::Stats,
    pub tags: Vec<db::TagCount>,
    pub toys: Vec<db::TagCount>,
    pub creators: Vec<String>,
    pub scan: ScanStatus,
    pub version: String,
    /// `std::env::consts::OS` so the UI can show platform-appropriate install
    /// commands and path placeholders.
    pub os: String,
}

#[tauri::command]
pub async fn bootstrap(state: State<'_, Arc<AppState>>) -> Result<Bootstrap, String> {
    let st = state.inner().clone();
    let root = st.root_path();
    let ff = st.ffmpeg();
    let (stats, tags, toys, creators, hash_mode) = {
        let conn = st.db.lock().map_err(err)?;
        (
            db::stats(&conn).map_err(err)?,
            db::all_tags(&conn).map_err(err)?,
            db::all_toys(&conn).map_err(err)?,
            db::creators(&conn).map_err(err)?,
            db::hash_mode(&conn),
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
        ffmpeg_prompt_shown: st.setting("ffmpeg_prompt_shown").as_deref() == Some("1"),
        visible_flags: VisibleFlags {
            nsfw: st.setting("show_nsfw").as_deref() != Some("0"),
            popping: st.setting("show_popping").as_deref() != Some("0"),
            destruction: st.setting("show_destruction").as_deref() != Some("0"),
        },
        discreet: st.setting("discreet").as_deref() == Some("1"),
        os: std::env::consts::OS.to_string(),
        ffmpeg_dir: ff.map(|f| {
            f.ffmpeg
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        }),
        hash_mode,
        cache_limit_mb: st
            .setting("cache_limit_mb")
            .and_then(|v| v.parse().ok())
            .unwrap_or(4096),
        stats,
        tags,
        toys,
        creators,
        scan: scan_state,
        convert_quality: media::quality_of(&st).as_str().to_string(),
        prewarm_posters: st.setting("prewarm_posters").as_deref() != Some("0"),
        native_player: st.setting("native_player").as_deref() != Some("0"),
        stream_window: st
            .setting("stream_window")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(30)
            .clamp(3, 120),
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootInfo {
    pub root: String,
    pub relinked: usize,
    pub switched: bool,
}

#[tauri::command]
pub async fn set_root(state: State<'_, Arc<AppState>>, path: String) -> Result<RootInfo, String> {
    let st = state.inner().clone();
    let canonical =
        std::fs::canonicalize(&path).map_err(|_| format!("Folder not found: {path}"))?;
    if !canonical.is_dir() {
        return Err("That path is not a folder.".into());
    }
    let new_root = db::display_path(&canonical.to_string_lossy());
    let old_root = st.setting("root").unwrap_or_default();

    let (relinked, switched) = {
        let mut conn = st.db.lock().map_err(err)?;
        if !old_root.is_empty() {
            let _ = db::backfill_rel_paths(&conn, &old_root);
        }
        let same_as_before = !old_root.is_empty() && db::same_path(&old_root, &new_root);
        if same_as_before {
            let _ = db::backfill_rel_paths(&conn, &new_root);
            (0, false)
        } else {
            // Only re-link when the chosen folder looks like the same
            // collection: a sample of the stored relative paths must exist.
            let sample: Vec<String> = {
                let mut stmt = conn
                    .prepare("SELECT rel_path FROM videos WHERE rel_path IS NOT NULL LIMIT 50")
                    .map_err(err)?;
                let rows = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(err)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(err)?;
                rows
            };
            let present = sample
                .iter()
                .filter(|rel| Path::new(&new_root).join(rel.as_str()).is_file())
                .count();
            let same_collection = !sample.is_empty() && present * 2 >= sample.len();
            if same_collection {
                let moved = db::repath_library(&mut conn, &new_root).map_err(err)?;
                let _ = db::repath_trash(&mut conn, &old_root, &new_root);
                let _ = db::backfill_rel_paths(&conn, &new_root);
                (moved, false)
            } else {
                (0, !old_root.is_empty())
            }
        }
    };

    st.set_setting("root", &new_root)?;
    *st.root.write().unwrap() = Some(PathBuf::from(&new_root));
    Ok(RootInfo {
        root: new_root,
        relinked,
        switched,
    })
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
    folder: Option<String>,
) -> Result<Vec<db::VideoDto>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::rating_queue(&conn, limit.unwrap_or(500), folder.as_deref()).map_err(err)
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
    trash_by_id(&st, id).await
}

#[tauri::command]
pub async fn trash_video(state: State<'_, Arc<AppState>>, id: i64) -> Result<String, String> {
    let st = state.inner().clone();
    trash_by_id(&st, id).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashManyResult {
    pub moved: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

/// Bulk variant used by the library's batch selection.
#[tauri::command]
pub async fn trash_many(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
) -> Result<TrashManyResult, String> {
    let st = state.inner().clone();
    let mut moved = 0usize;
    let mut errors = Vec::new();
    for id in ids {
        match trash_by_id(&st, id).await {
            Ok(_) => moved += 1,
            Err(e) => errors.push(e),
        }
    }
    if moved == 0 && !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    Ok(TrashManyResult {
        moved,
        failed: errors.len(),
        errors,
    })
}

/// Move a file, retrying while another process still has it open. Windows
/// refuses to rename a file that a decoder is reading, and handles close a
/// moment after playback stops.
async fn move_into_place(src: &Path, target: &Path) -> Result<(), String> {
    let mut last_error = String::new();
    for attempt in 0..20 {
        match tokio::fs::rename(src, target).await {
            Ok(()) => return Ok(()),
            Err(e) => last_error = e.to_string(),
        }
        // Copy+delete works when only the rename is blocked (read-only sharing).
        if attempt >= 4 {
            let copied = tokio::fs::copy(src, target).await.is_ok();
            if copied {
                match tokio::fs::remove_file(src).await {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        // Leave nothing behind: a copy without the original gone
                        // would duplicate the file in the library.
                        let _ = tokio::fs::remove_file(target).await;
                        last_error = e.to_string();
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    Err(format!(
        "could not move the file: {last_error}. It is still open in the player or being converted \
         — stop playback and try again."
    ))
}

/// Move a video to `<library>/_trash`, keeping its metadata for a restore.
pub(crate) async fn trash_by_id(st: &Arc<AppState>, id: i64) -> Result<String, String> {
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

    move_into_place(&src, &target).await?;

    {
        let mut conn = st.db.lock().map_err(err)?;
        let meta = serde_json::json!({
            "creator": video.creator,
            "nsfw": video.nsfw,
            "popping": video.popping,
            "destruction": video.destruction,
            "deflation": video.deflation,
            "inflation": video.inflation,
            "notes": video.notes,
            "rating": video.rating,
            "reviewed": video.reviewed,
            "tags": video.tags,
            "toys": video.toys,
        });
        let meta_json = serde_json::to_string(&meta).ok();
        let _ = db::trash_add(
            &conn,
            &video.path,
            &target.to_string_lossy(),
            &video.file_name,
            video.size,
            video.md5.as_deref(),
            meta_json.as_deref(),
            scan::now(),
        );
        db::delete_video(&mut conn, id).map_err(err)?;
    }
    let _ = tokio::fs::remove_dir_all(media::thumb_dir(st, &video)).await;
    let _ = tokio::fs::remove_file(media::play_cache_path(st, &video)).await;

    Ok(target.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn mark_viewed(state: State<'_, Arc<AppState>>, id: i64) -> Result<i64, String> {
    let st = state.inner().clone();
    let conn = st.db.lock().map_err(err)?;
    db::mark_viewed(&conn, id, scan::now()).map_err(err)
}

/* --------------------------- old codec view ----------------------------- */

/// Called once at startup: the webview reports which extra codecs it can decode.
#[tauri::command]
pub async fn set_playable_codecs(hevc8: bool, hevc10: bool) -> Result<(), String> {
    media::set_capabilities(hevc8, hevc10);
    Ok(())
}

#[tauri::command]
pub async fn playable_codecs() -> Result<serde_json::Value, String> {
    let caps = media::capabilities();
    Ok(serde_json::json!({ "hevc8": caps.hevc8, "hevc10": caps.hevc10 }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OldCodecItem {
    #[serde(flatten)]
    pub video: db::VideoDto,
    /// A converted MP4 already exists in the playback cache.
    pub cached: bool,
    /// Path of a converted MP4 next to the original, if there is one.
    pub converted: Option<String>,
}

#[tauri::command]
pub async fn old_codecs(state: State<'_, Arc<AppState>>) -> Result<Vec<OldCodecItem>, String> {
    let st = state.inner().clone();
    let videos = {
        let conn = st.db.lock().map_err(err)?;
        db::old_codec_videos(&conn).map_err(err)?
    };
    Ok(videos
        .into_iter()
        .map(|video| OldCodecItem {
            cached: media::play_cache_path(&st, &video).is_file(),
            converted: converted_path(&video),
            video,
        })
        .collect())
}

fn converted_path(video: &db::VideoDto) -> Option<String> {
    let path = Path::new(&video.path);
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let candidate = path.parent()?.join(format!("{stem}.mp4"));
    if candidate.is_file() && candidate != path {
        Some(candidate.to_string_lossy().to_string())
    } else {
        None
    }
}

#[tauri::command]
pub async fn convert_start(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    target: String,
    trash_source: bool,
) -> Result<(), String> {
    let st = state.inner().clone();
    convert::start(st, ids, target, trash_source)
}

#[tauri::command]
pub async fn convert_status(state: State<'_, Arc<AppState>>) -> Result<convert::Status, String> {
    Ok(convert::snapshot(state.inner()))
}

#[tauri::command]
pub async fn convert_cancel(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    convert::cancel(state.inner());
    Ok(())
}

#[tauri::command]
pub async fn set_convert_quality(
    state: State<'_, Arc<AppState>>,
    quality: String,
) -> Result<String, String> {
    let parsed = media::Quality::parse(Some(&quality));
    {
        let conn = state.inner().db.lock().map_err(err)?;
        db::set_setting(&conn, "convert_quality", parsed.as_str()).map_err(err)?;
    }
    Ok(parsed.as_str().to_string())
}

/* --------------------------- preview prewarm ---------------------------- */

#[tauri::command]
pub async fn prewarm_status(state: State<'_, Arc<AppState>>) -> Result<prewarm::Status, String> {
    Ok(prewarm::snapshot(state.inner()))
}

#[tauri::command]
pub async fn start_prewarm(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let st = state.inner().clone();
    if !prewarm::start(st) {
        return Err("Previews are already being generated.".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn cancel_prewarm(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    prewarm::cancel(state.inner());
    Ok(())
}

#[tauri::command]
pub async fn set_prewarm_enabled(
    state: State<'_, Arc<AppState>>,
    enabled: bool,
) -> Result<(), String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::set_setting(&conn, "prewarm_posters", if enabled { "1" } else { "0" }).map_err(err)
}

#[tauri::command]
pub async fn set_native_player(
    state: State<'_, Arc<AppState>>,
    enabled: bool,
) -> Result<(), String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::set_setting(&conn, "native_player", if enabled { "1" } else { "0" }).map_err(err)
}

/// Called by the UI when the webview's own decoder rejects a file. Stores the
/// flag so the next play goes straight through the built-in player and the file
/// is listed under Old codecs for conversion.
#[tauri::command]
pub async fn mark_native_failed(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::set_native_failed(&conn, id, true).map_err(err)
}

/// Label proposals for one video, derived from its name and folder.
#[tauri::command]
pub async fn suggest_labels(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> Result<Vec<suggest::Suggestion>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    suggest::suggest_for(&conn, id)
}

/// Which encoder the built-in player will use (`None` means libx264).
#[tauri::command]
pub async fn stream_encoder(state: State<'_, Arc<AppState>>) -> Result<Option<String>, String> {
    let ffmpeg = state.inner().ffmpeg().ok_or("ffmpeg was not found")?;
    Ok(stream::hardware_encoder(&ffmpeg.ffmpeg).map(str::to_string))
}

#[tauri::command]
pub async fn set_stream_window(
    state: State<'_, Arc<AppState>>,
    seconds: u64,
) -> Result<u64, String> {
    let seconds = seconds.clamp(3, 120);
    {
        let conn = state.inner().db.lock().map_err(err)?;
        db::set_setting(&conn, "stream_window", &seconds.to_string()).map_err(err)?;
    }
    Ok(seconds)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackInfo {
    pub mode: String,
    pub state: String,
    pub url: Option<String>,
    pub progress: f64,
    pub error: Option<String>,
    pub stream_mime: Option<String>,
}

fn playback_info(st: &Arc<AppState>, video: &db::VideoDto, start: bool) -> PlaybackInfo {
    let url = format!("{}/media/{}", st.media_base(), video.id);
    let mode = video.playback.clone();

    // The built-in stream player decodes anything ffmpeg can read and writes a
    // constant frame rate, so it also fixes stuttering VFR files. Remux cases
    // (browser codec, unusual container or audio) used to be copied into the
    // cache first, which cost several seconds per file; they stream instead.
    if st.ffmpeg().is_none() && (mode != "direct" || video.vfr || video.native_failed) {
        return PlaybackInfo {
            mode,
            state: "error".into(),
            url: None,
            progress: 0.0,
            error: Some(format!(
                "ffmpeg was not found, so this file cannot be played. \
                 Install ffmpeg ({}) and restart the app.",
                media::ffmpeg_install_hint()
            )),
            stream_mime: None,
        };
    }
    if (mode != "direct" || video.vfr || video.native_failed)
        && st.setting("native_player").as_deref() != Some("0")
    {
        let (_, codec) = stream::profile_for(video.width, video.height);
        return PlaybackInfo {
            mode: "stream".into(),
            state: "ready".into(),
            url: None,
            progress: 1.0,
            error: None,
            stream_mime: Some(format!("video/mp4; codecs=\"{codec},mp4a.40.2\"")),
        };
    }

    if mode == "direct" {
        // Keep the stream MIME around as well: if the webview's decoder rejects
        // a malformed file, the UI re-opens it through the built-in player.
        let (_, codec) = stream::profile_for(video.width, video.height);
        return PlaybackInfo {
            mode,
            state: "ready".into(),
            url: Some(url),
            progress: 1.0,
            error: None,
            stream_mime: Some(format!("video/mp4; codecs=\"{codec},mp4a.40.2\"")),
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
            stream_mime: None,
        };
    }
    if st.ffmpeg().is_none() {
        return PlaybackInfo {
            mode,
            state: "error".into(),
            url: None,
            progress: 0.0,
            error: Some(format!(
                "ffmpeg was not found, so this file cannot be converted for playback. \
                 Install ffmpeg ({}) and restart the app.",
                media::ffmpeg_install_hint()
            )),
            stream_mime: None,
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
            stream_mime: None,
        },
        Some(job) if job.state == "done" && cache.is_file() => PlaybackInfo {
            mode,
            state: "ready".into(),
            url: Some(url),
            progress: 1.0,
            error: None,
            stream_mime: None,
        },
        Some(job) if job.state == "done" => PlaybackInfo {
            mode,
            state: "queued".into(),
            url: None,
            progress: -1.0,
            error: None,
            stream_mime: None,
        },
        Some(job) => PlaybackInfo {
            mode,
            state: "working".into(),
            url: None,
            progress: job.progress,
            error: None,
            stream_mime: None,
        },
        None => PlaybackInfo {
            mode,
            state: "queued".into(),
            url: None,
            progress: -1.0,
            error: None,
            stream_mime: None,
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
pub async fn all_toys(state: State<'_, Arc<AppState>>) -> Result<Vec<db::TagCount>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::all_toys(&conn).map_err(err)
}

#[tauri::command]
pub async fn rename_toy(
    state: State<'_, Arc<AppState>>,
    old: String,
    new: String,
) -> Result<(), String> {
    let mut conn = state.inner().db.lock().map_err(err)?;
    db::rename_toy(&mut conn, &old, &new).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn delete_toy(state: State<'_, Arc<AppState>>, name: String) -> Result<(), String> {
    let mut conn = state.inner().db.lock().map_err(err)?;
    db::delete_toy(&mut conn, &name).map_err(|e| format!("{e:#}"))
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
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("explorer");
        // explorer needs /select,"<path>" with the path quoted as part of the
        // same argument; otherwise paths with spaces fall back to Documents.
        command.raw_arg(format!("/select,\"{}\"", video.path));
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        command.spawn().map_err(err)?;
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "macos")]
        let spawned = std::process::Command::new("open")
            .args(["-R", &video.path])
            .spawn();
        #[cfg(not(target_os = "macos"))]
        let spawned = {
            // Linux: open the containing folder with the desktop's handler.
            // `xdg-open` comes from xdg-utils; `gio` is always available with
            // GTK and is a useful fallback on minimal installs.
            let parent = Path::new(&video.path)
                .parent()
                .unwrap_or_else(|| Path::new("."));
            std::process::Command::new("xdg-open")
                .arg(parent)
                .spawn()
                .or_else(|_| {
                    std::process::Command::new("gio")
                        .arg("open")
                        .arg(parent)
                        .spawn()
                })
        };
        spawned.map_err(|e| format!("could not open the file manager: {e}"))?;
    }
    Ok(())
}

/// Play a file with the system's default video player instead of the built-in
/// one. Useful for formats the app can stream but the user wants to inspect in
/// their usual player.
#[tauri::command]
pub async fn open_external(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    let video = state.inner().video(id).ok_or("Video not found.")?;
    let path = Path::new(&video.path);
    if !path.is_file() {
        return Err("The file is no longer available.".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // ShellExec_RunDLL goes through ShellExecute, so it uses the default
        // handler for the extension without a console window or shell parsing.
        let mut command = std::process::Command::new("rundll32.exe");
        command
            .arg("shell32.dll,ShellExec_RunDLL")
            .arg(path)
            .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        command
            .spawn()
            .map_err(|e| format!("could not open the video: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("could not open the video: {e}"))?;
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .or_else(|_| {
                std::process::Command::new("gio")
                    .arg("open")
                    .arg(path)
                    .spawn()
            })
            .map_err(|e| format!("could not open the video: {e}"))?;
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
        let _ = db::normalize_stored_paths(&conn);
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

/* -------------------------------- folders ------------------------------- */

#[tauri::command]
pub async fn folders(state: State<'_, Arc<AppState>>) -> Result<Vec<db::FolderCount>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::folders(&conn).map_err(err)
}

/* ----------------------------- bulk editing ----------------------------- */

#[tauri::command]
pub async fn bulk_update(
    state: State<'_, Arc<AppState>>,
    edit: db::BulkEdit,
) -> Result<usize, String> {
    let mut conn = state.inner().db.lock().map_err(err)?;
    db::bulk_update(&mut conn, &edit).map_err(err)
}

/* ---------------------------- tag management ---------------------------- */

#[tauri::command]
pub async fn rename_tag(
    state: State<'_, Arc<AppState>>,
    old: String,
    new: String,
) -> Result<(), String> {
    let mut conn = state.inner().db.lock().map_err(err)?;
    db::rename_tag(&mut conn, &old, &new).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn delete_tag(state: State<'_, Arc<AppState>>, name: String) -> Result<(), String> {
    let mut conn = state.inner().db.lock().map_err(err)?;
    db::delete_tag(&mut conn, &name).map_err(|e| format!("{e:#}"))
}

/* -------------------------------- trash --------------------------------- */

#[tauri::command]
pub async fn trash_list(state: State<'_, Arc<AppState>>) -> Result<Vec<db::TrashEntry>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::trash_list(&conn).map_err(err)
}

#[tauri::command]
pub async fn restore_trash(state: State<'_, Arc<AppState>>, id: i64) -> Result<String, String> {
    let st = state.inner().clone();
    let entry = {
        let conn = st.db.lock().map_err(err)?;
        db::trash_get(&conn, id).map_err(err)?
    }
    .ok_or("Trash entry not found.")?;

    let source = PathBuf::from(&entry.trashed_path);
    if !source.is_file() {
        return Err(format!("The trashed file is missing: {}", source.display()));
    }
    let target = PathBuf::from(&entry.original_path);
    if target.exists() {
        return Err(format!("A file already exists at {}", target.display()));
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(err)?;
    }
    if tokio::fs::rename(&source, &target).await.is_err() {
        tokio::fs::copy(&source, &target).await.map_err(err)?;
        tokio::fs::remove_file(&source).await.map_err(err)?;
    }

    let path_str = target.to_string_lossy().to_string();
    let file_name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| entry.file_name.clone());
    let ext = target
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let meta = tokio::fs::metadata(&target).await.map_err(err)?;
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let new_id = {
        let mut conn = st.db.lock().map_err(err)?;
        let rel = st
            .root_path()
            .and_then(|root| db::relative_to(&path_str, &root.to_string_lossy()));
        let upsert = db::upsert_file(
            &conn,
            &path_str,
            rel.as_deref(),
            &file_name,
            &ext,
            size,
            mtime,
            scan::now(),
        )
        .map_err(err)?;
        if let Some(json) = &entry.meta_json {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
                let patch = db::VideoPatch {
                    creator: value
                        .get("creator")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    notes: value
                        .get("notes")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    nsfw: value.get("nsfw").and_then(|v| v.as_bool()),
                    popping: value.get("popping").and_then(|v| v.as_bool()),
                    destruction: value.get("destruction").and_then(|v| v.as_bool()),
                    deflation: value.get("deflation").and_then(|v| v.as_bool()),
                    inflation: value.get("inflation").and_then(|v| v.as_bool()),
                    rating: value.get("rating").and_then(|v| v.as_i64()),
                    reviewed: value.get("reviewed").and_then(|v| v.as_bool()),
                    tags: value.get("tags").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|t| t.as_str().map(str::to_string))
                            .collect()
                    }),
                    toys: value.get("toys").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|t| t.as_str().map(str::to_string))
                            .collect()
                    }),
                };
                let _ = db::save_video(&mut conn, upsert.id, &patch);
            }
        }
        if let Some(md5) = &entry.md5 {
            let _ = db::set_md5(&conn, upsert.id, md5);
        }
        db::trash_delete(&conn, id).map_err(err)?;
        upsert.id
    };

    if let Some(ffmpeg) = st.ffmpeg() {
        if let Ok(info) = media::probe(&ffmpeg.ffprobe, &target).await {
            if let Ok(conn) = st.db.lock() {
                let _ = db::set_metadata(&conn, new_id, &info);
            }
        }
    }
    Ok(path_str)
}

#[tauri::command]
pub async fn delete_trash(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    let st = state.inner().clone();
    let entry = {
        let conn = st.db.lock().map_err(err)?;
        db::trash_get(&conn, id).map_err(err)?
    }
    .ok_or("Trash entry not found.")?;
    let path = PathBuf::from(&entry.trashed_path);
    let _ = tokio::fs::remove_file(&path).await;
    let conn = st.db.lock().map_err(err)?;
    db::trash_delete(&conn, id).map_err(err)
}

/* --------------------------- indexing options --------------------------- */

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisibleFlags {
    pub nsfw: bool,
    pub popping: bool,
    pub destruction: bool,
}

#[tauri::command]
pub async fn set_flag_visible(
    state: State<'_, Arc<AppState>>,
    flag: String,
    visible: bool,
) -> Result<(), String> {
    if !matches!(flag.as_str(), "nsfw" | "popping" | "destruction") {
        return Err("Unknown flag.".into());
    }
    state
        .inner()
        .set_setting(&format!("show_{flag}"), if visible { "1" } else { "0" })
}

#[tauri::command]
pub async fn set_discreet(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    state
        .inner()
        .set_setting("discreet", if enabled { "1" } else { "0" })
}

#[tauri::command]
pub async fn overview(state: State<'_, Arc<AppState>>) -> Result<db::Overview, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::overview(&conn, scan::now()).map_err(err)
}

#[tauri::command]
pub async fn dismiss_ffmpeg_prompt(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.inner().set_setting("ffmpeg_prompt_shown", "1")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegInfo {
    pub ok: bool,
    pub dir: Option<String>,
}

#[tauri::command]
pub async fn check_ffmpeg(state: State<'_, Arc<AppState>>) -> Result<FfmpegInfo, String> {
    let st = state.inner().clone();
    st.refresh_ffmpeg();
    let ff = st.ffmpeg();
    Ok(FfmpegInfo {
        ok: ff.is_some(),
        dir: ff.map(|f| {
            f.ffmpeg
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        }),
    })
}

#[tauri::command]
pub async fn purge_missing(state: State<'_, Arc<AppState>>) -> Result<usize, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::purge_missing(&conn).map_err(err)
}

#[tauri::command]
pub async fn set_hash_mode(state: State<'_, Arc<AppState>>, mode: String) -> Result<(), String> {
    if mode != "all" && mode != "duplicates" {
        return Err("Unknown hash mode.".into());
    }
    let conn = state.inner().db.lock().map_err(err)?;
    db::set_hash_mode(&conn, &mode).map_err(err)
}

#[tauri::command]
pub async fn set_cache_limit(state: State<'_, Arc<AppState>>, mb: u64) -> Result<u64, String> {
    let st = state.inner().clone();
    st.set_setting("cache_limit_mb", &mb.to_string())?;
    Ok(media::enforce_cache_limit(&st))
}

#[tauri::command]
pub async fn clean_cache(state: State<'_, Arc<AppState>>) -> Result<u64, String> {
    let st = state.inner().clone();
    Ok(media::enforce_cache_limit(&st))
}

/* ------------------------------- video DNA ------------------------------ */

#[tauri::command]
pub async fn start_dna(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if state.inner().scan.lock().map_err(err)?.running {
        return Err("Wait for the current scan to finish.".into());
    }
    dna::spawn(state.inner().clone());
    Ok(())
}

#[tauri::command]
pub async fn dna_status(state: State<'_, Arc<AppState>>) -> Result<dna::DnaStatus, String> {
    Ok(state.inner().dna.lock().map_err(err)?.clone())
}

#[tauri::command]
pub async fn cancel_dna(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let status = state.inner().dna.lock().map_err(err)?;
    status
        .cancel
        .store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub async fn similar_duplicates(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<db::SimilarGroup>, String> {
    let conn = state.inner().db.lock().map_err(err)?;
    db::similar_groups(&conn).map_err(err)
}

/* ------------------------ export / import / backup ---------------------- */

#[derive(Serialize, Deserialize)]
struct ExportVideo {
    path: String,
    /// Path inside the library, independent of where the library root is now.
    #[serde(default)]
    rel_path: Option<String>,
    #[serde(default)]
    md5: Option<String>,
    /// Exact sampled-frame fingerprint; matches re-encoded copies whose bytes
    /// differ but whose frames are identical.
    #[serde(default)]
    dna: Option<String>,
    #[serde(default)]
    creator: Option<String>,
    #[serde(default)]
    nsfw: bool,
    #[serde(default)]
    popping: bool,
    #[serde(default)]
    destruction: bool,
    #[serde(default)]
    deflation: bool,
    #[serde(default)]
    inflation: bool,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    rating: i64,
    #[serde(default)]
    reviewed: bool,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    toys: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct ExportFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    exported_at: i64,
    #[serde(default)]
    videos: Vec<ExportVideo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub matched: usize,
    pub unmatched: usize,
    pub total: usize,
    pub by_hash: usize,
    pub by_dna: usize,
    pub by_path: usize,
}

#[tauri::command]
pub async fn export_library(state: State<'_, Arc<AppState>>) -> Result<Option<String>, String> {
    let st = state.inner().clone();
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Export library metadata")
            .set_file_name("splashhound-metadata.json")
            .add_filter("JSON", &["json"])
            .save_file()
    })
    .await
    .map_err(err)?;
    let Some(path) = picked else {
        return Ok(None);
    };

    let export = {
        let conn = st.db.lock().map_err(err)?;
        build_export(&conn)?
    };
    let text = serde_json::to_string_pretty(&export).map_err(err)?;
    tokio::fs::write(&path, text).await.map_err(err)?;
    Ok(Some(path.to_string_lossy().to_string()))
}

/// Snapshot every present video's metadata plus the matching keys (hash, DNA
/// and library-relative path) into the portable export file.
fn build_export(conn: &rusqlite::Connection) -> Result<ExportFile, String> {
    let videos = db::all_videos(conn).map_err(err)?;
    let dna = db::dna_map(conn).map_err(err)?;
    Ok(ExportFile {
        version: 3,
        exported_at: scan::now(),
        videos: videos
            .iter()
            .map(|v| ExportVideo {
                path: v.path.clone(),
                rel_path: v.rel_path.clone(),
                md5: v.md5.clone(),
                dna: dna.get(&v.id).cloned(),
                creator: v.creator.clone(),
                nsfw: v.nsfw,
                popping: v.popping,
                destruction: v.destruction,
                deflation: v.deflation,
                inflation: v.inflation,
                notes: v.notes.clone(),
                rating: v.rating,
                reviewed: v.reviewed,
                tags: v.tags.clone(),
                toys: v.toys.clone(),
            })
            .collect(),
    })
}

#[tauri::command]
pub async fn import_library(
    state: State<'_, Arc<AppState>>,
) -> Result<Option<ImportResult>, String> {
    let st = state.inner().clone();
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Import library metadata")
            .add_filter("JSON", &["json"])
            .pick_file()
    })
    .await
    .map_err(err)?;
    let Some(path) = picked else {
        return Ok(None);
    };

    let text = tokio::fs::read_to_string(&path).await.map_err(err)?;
    let data: ExportFile = serde_json::from_str(&text)
        .map_err(|e| format!("Not a Splashhound metadata export: {e}"))?;

    let mut conn = st.db.lock().map_err(err)?;
    let result = import_metadata(&mut conn, &data)?;
    Ok(Some(result))
}

/// Apply an exported metadata file to the current library. Byte-identical files
/// are matched by MD5, re-encodes by an exact DNA fingerprint, and only then by
/// the stored path.
fn import_metadata(
    conn: &mut rusqlite::Connection,
    data: &ExportFile,
) -> Result<ImportResult, String> {
    let total = data.videos.len();
    let mut matched = 0usize;
    let mut unmatched = 0usize;
    let mut by_hash_count = 0usize;
    let mut by_dna_count = 0usize;
    let mut by_path_count = 0usize;
    for item in &data.videos {
        let by_hash: Option<i64> = if let Some(md5) = item.md5.as_deref().filter(|m| !m.is_empty())
        {
            conn.query_row(
                "SELECT id FROM videos WHERE md5 = ?1 AND missing = 0 LIMIT 1",
                params![md5],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?
        } else {
            None
        };
        // Byte-identical files first. If the bytes differ (re-encode, different
        // container), fall back to an exact DNA fingerprint match: every sampled
        // frame hash has to be identical, i.e. a 100% match.
        let by_dna: Option<i64> = if by_hash.is_none() {
            match item.dna.as_deref().filter(|d| !d.is_empty()) {
                Some(frames) => db::find_by_dna(conn, frames).map_err(err)?,
                None => None,
            }
        } else {
            None
        };
        // Path is only a last resort. Prefer the library-relative path so a
        // copied collection matches even when its root moved; separators are
        // normalised because an export can come from another platform. Only
        // fall back to the old absolute path when the export has no rel_path.
        let by_path: Option<i64> = if by_hash.is_none() && by_dna.is_none() {
            let by_rel = match item.rel_path.as_deref().filter(|r| !r.is_empty()) {
                Some(rel) => conn
                    .query_row(
                        "SELECT id FROM videos
                         WHERE replace(rel_path, '\\', '/') = replace(?1, '\\', '/')
                           AND missing = 0 LIMIT 1",
                        params![rel],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(err)?,
                None => None,
            };
            match by_rel {
                Some(id) => Some(id),
                None => conn
                    .query_row(
                        "SELECT id FROM videos WHERE path = ?1 AND missing = 0 LIMIT 1",
                        params![item.path],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(err)?,
            }
        } else {
            None
        };
        let (id, source) = if let Some(id) = by_hash {
            (id, 1)
        } else if let Some(id) = by_dna {
            (id, 2)
        } else if let Some(id) = by_path {
            (id, 3)
        } else {
            unmatched += 1;
            continue;
        };
        let patch = db::VideoPatch {
            creator: item.creator.clone(),
            notes: item.notes.clone(),
            nsfw: Some(item.nsfw),
            popping: Some(item.popping),
            destruction: Some(item.destruction),
            deflation: Some(item.deflation),
            inflation: Some(item.inflation),
            rating: Some(item.rating),
            reviewed: Some(item.reviewed),
            tags: Some(item.tags.clone()),
            toys: Some(item.toys.clone()),
        };
        if db::save_video(conn, id, &patch).is_ok() {
            matched += 1;
            match source {
                1 => by_hash_count += 1,
                2 => by_dna_count += 1,
                _ => by_path_count += 1,
            }
        } else {
            unmatched += 1;
        }
    }
    Ok(ImportResult {
        matched,
        unmatched,
        total,
        by_hash: by_hash_count,
        by_dna: by_dna_count,
        by_path: by_path_count,
    })
}

#[tauri::command]
pub async fn backup_database(state: State<'_, Arc<AppState>>) -> Result<Option<String>, String> {
    let st = state.inner().clone();
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Back up library database")
            .set_file_name("splashhound-backup.db")
            .add_filter("SQLite", &["db"])
            .save_file()
    })
    .await
    .map_err(err)?;
    let Some(path) = picked else {
        return Ok(None);
    };

    let db = st.db.clone();
    let source = st.db_path();
    let target = path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let guard = db.lock().map_err(|e| e.to_string())?;
        guard
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())?;
        std::fs::copy(&source, &target).map_err(|e| format!("Could not write backup: {e}"))?;
        Ok(())
    })
    .await
    .map_err(err)??;

    Ok(Some(path.to_string_lossy().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        conn
    }

    fn add_video(
        conn: &rusqlite::Connection,
        path: &str,
        md5: Option<&str>,
        dna: Option<&str>,
    ) -> i64 {
        conn.execute(
            "INSERT INTO videos(path, file_name, added_at, md5, missing)
             VALUES (?1, ?2, 1, ?3, 0)",
            params![path, path, md5],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        if let Some(frames) = dna {
            conn.execute(
                "INSERT INTO video_dna(video_id, frames) VALUES (?1, ?2)",
                params![id, frames],
            )
            .unwrap();
        }
        id
    }

    fn entry(path: &str, md5: Option<&str>, dna: Option<&str>, creator: &str) -> ExportVideo {
        ExportVideo {
            path: path.to_string(),
            rel_path: None,
            md5: md5.map(str::to_string),
            dna: dna.map(str::to_string),
            creator: Some(creator.to_string()),
            nsfw: false,
            popping: false,
            destruction: false,
            deflation: false,
            inflation: false,
            notes: None,
            rating: 4,
            reviewed: true,
            tags: Vec::new(),
            toys: Vec::new(),
        }
    }

    fn creator(conn: &rusqlite::Connection, id: i64) -> Option<String> {
        db::get_video(conn, id).unwrap().unwrap().creator
    }

    #[test]
    fn import_matches_by_hash_dna_and_path() {
        let mut conn = test_conn();
        let hash_id = add_video(&conn, "/lib/a.mp4", Some("aaa"), None);
        let dna_id = add_video(&conn, "/lib/b.mp4", Some("bbb"), Some("1,2,3,4"));
        let path_id = add_video(&conn, "/lib/c.mp4", None, None);

        let export = ExportFile {
            version: 3,
            exported_at: 0,
            videos: vec![
                entry("/elsewhere/a.mp4", Some("aaa"), None, "hash"),
                entry("/elsewhere/b.mp4", Some("zzz"), Some("1,2,3,4"), "dna"),
                entry("/lib/c.mp4", None, None, "path"),
                entry("/elsewhere/missing.mp4", None, None, "none"),
            ],
        };

        let result = import_metadata(&mut conn, &export).unwrap();
        assert_eq!(result.matched, 3);
        assert_eq!(result.unmatched, 1);
        assert_eq!(result.by_hash, 1);
        assert_eq!(result.by_dna, 1);
        assert_eq!(result.by_path, 1);
        assert_eq!(creator(&conn, hash_id).as_deref(), Some("hash"));
        assert_eq!(creator(&conn, dna_id).as_deref(), Some("dna"));
        assert_eq!(creator(&conn, path_id).as_deref(), Some("path"));
    }

    #[test]
    fn import_matches_by_relative_path_when_root_moved() {
        let mut conn = test_conn();
        conn.execute(
            "INSERT INTO videos(path, rel_path, file_name, added_at)
             VALUES ('/new/root/sub/c.mp4', 'sub/c.mp4', 'c.mp4', 1)",
            [],
        )
        .unwrap();
        let id = conn.last_insert_rowid();

        let mut item = entry("/old/root/sub/c.mp4", None, None, "rel");
        // Windows-style separator in the export, Linux-style in the database.
        item.rel_path = Some("sub\\c.mp4".into());
        let export = ExportFile {
            version: 3,
            exported_at: 0,
            videos: vec![item],
        };

        let result = import_metadata(&mut conn, &export).unwrap();
        assert_eq!(result.by_path, 1);
        assert_eq!(creator(&conn, id).as_deref(), Some("rel"));
    }

    #[test]
    fn export_includes_hash_dna_and_relative_path() {
        let conn = test_conn();
        let id = add_video(&conn, "/lib/a.mp4", Some("aaa"), Some("1,2,3,4"));
        conn.execute(
            "UPDATE videos SET rel_path = 'sub/a.mp4' WHERE id = ?1",
            params![id],
        )
        .unwrap();

        let export = build_export(&conn).unwrap();
        assert_eq!(export.version, 3);
        assert_eq!(export.videos.len(), 1);
        assert_eq!(export.videos[0].md5.as_deref(), Some("aaa"));
        assert_eq!(export.videos[0].dna.as_deref(), Some("1,2,3,4"));
        assert_eq!(export.videos[0].rel_path.as_deref(), Some("sub/a.mp4"));
    }
}
