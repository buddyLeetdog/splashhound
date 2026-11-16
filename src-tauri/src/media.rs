use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::SystemTime,
};

use anyhow::{anyhow, bail, Context, Result};
use md5::{Digest, Md5};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::{db::VideoDto, AppState, Conversion, Ffmpeg};

/// Spawn a process without flashing a console window. The app is built as a
/// GUI-subsystem binary, so every child process would otherwise get its own
/// console for a moment (very visible while scanning a large library).
fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    tokio::process::Command::from(command)
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInfo {
    pub duration_ms: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub has_audio: bool,
    pub format_name: Option<String>,
}

/// How a file can be handed to the WebView2 player.
/// direct = play the original file, remux/transcode = stream a cached MP4.
pub fn playback_str(ext: &str, vcodec: Option<&str>, acodec: Option<&str>) -> &'static str {
    let ext = ext.to_ascii_lowercase();
    let v = vcodec.unwrap_or("").to_ascii_lowercase();
    let a = acodec.unwrap_or("").to_ascii_lowercase();
    let browser_ext = matches!(ext.as_str(), "mp4" | "m4v" | "mov" | "webm" | "ogv" | "ogg");
    let browser_v = matches!(v.as_str(), "h264" | "vp8" | "vp9" | "av1");
    let browser_a =
        a.is_empty() || matches!(a.as_str(), "aac" | "mp3" | "opus" | "vorbis" | "flac");
    if browser_ext && browser_v && browser_a {
        "direct"
    } else if matches!(v.as_str(), "h264" | "vp9" | "av1") {
        "remux"
    } else {
        "transcode"
    }
}

pub fn detect_ffmpeg(override_dir: Option<&str>) -> Option<Ffmpeg> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = override_dir.filter(|d| !d.trim().is_empty()) {
        dirs.push(PathBuf::from(d));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
        let packages = local.join("Microsoft").join("WinGet").join("Packages");
        if let Ok(entries) = std::fs::read_dir(&packages) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name.starts_with("gyan.ffmpeg") {
                    dirs.push(entry.path().join("bin"));
                    if let Ok(inner) = std::fs::read_dir(entry.path()) {
                        for sub in inner.flatten() {
                            dirs.push(sub.path().join("bin"));
                        }
                    }
                }
            }
        }
        dirs.push(local.join("Programs").join("ffmpeg").join("bin"));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        dirs.push(PathBuf::from(pf).join("ffmpeg").join("bin"));
    }
    dirs.push(PathBuf::from(r"C:\ffmpeg\bin"));

    for dir in dirs {
        let ffmpeg = dir.join("ffmpeg.exe");
        let ffprobe = dir.join("ffprobe.exe");
        if ffmpeg.is_file() && ffprobe.is_file() {
            return Some(Ffmpeg { ffmpeg, ffprobe });
        }
    }
    None
}

pub async fn probe(ffprobe: &Path, file: &Path) -> Result<ProbeInfo> {
    let output = quiet_command(ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(file)
        .stdin(Stdio::null())
        .output()
        .await
        .context("could not run ffprobe")?;
    if !output.status.success() {
        bail!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    let mut info = ProbeInfo::default();
    info.format_name = json
        .get("format")
        .and_then(|f| f.get("format_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let Some(d) = json
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
    {
        info.duration_ms = Some((d * 1000.0).round() as i64);
    }

    if let Some(streams) = json.get("streams").and_then(|s| s.as_array()) {
        for stream in streams {
            let kind = stream
                .get("codec_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let codec = stream
                .get("codec_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            match kind {
                "video" => {
                    if info.vcodec.is_none() {
                        info.vcodec = codec.clone();
                        info.width = stream.get("width").and_then(|v| v.as_i64());
                        info.height = stream.get("height").and_then(|v| v.as_i64());
                        if info.duration_ms.is_none() {
                            info.duration_ms = stream
                                .get("duration")
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse::<f64>().ok())
                                .map(|d| (d * 1000.0).round() as i64);
                        }
                    }
                }
                "audio" => {
                    info.has_audio = true;
                    if info.acodec.is_none() {
                        info.acodec = codec;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(info)
}

pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn thumb_key(v: &VideoDto) -> String {
    format!("{}_{}_{}", v.id, v.size, v.mtime)
}

pub fn thumb_dir(state: &AppState, v: &VideoDto) -> PathBuf {
    state.cache_dir.join("thumbs").join(thumb_key(v))
}

pub fn thumb_path(state: &AppState, v: &VideoDto, index: usize) -> PathBuf {
    thumb_dir(state, v).join(format!("{:02}.jpg", index))
}

pub const THUMB_COUNT: usize = 12;

pub fn thumb_times(duration_ms: Option<i64>, n: usize) -> Vec<f64> {
    let seconds = duration_ms.filter(|d| *d > 0).unwrap_or(0) as f64 / 1000.0;
    if seconds <= 0.2 {
        return (0..n).map(|i| 0.5 + i as f64).collect();
    }
    (0..n)
        .map(|i| {
            let t = seconds * (i as f64 + 0.5) / n as f64;
            t.min((seconds - 0.1).max(0.0))
        })
        .collect()
}

async fn try_extract(ffmpeg: &Path, src: &Path, t: f64, out: &Path, seek_first: bool) -> bool {
    let mut cmd = quiet_command(ffmpeg);
    cmd.args(["-hide_banner", "-v", "error", "-nostdin", "-y"]);
    if seek_first {
        cmd.arg("-ss").arg(format!("{:.3}", t));
    }
    cmd.arg("-i").arg(src);
    if !seek_first {
        cmd.arg("-ss").arg(format!("{:.3}", t));
    }
    cmd.args([
        "-frames:v",
        "1",
        "-vf",
        "scale=480:-2",
        "-q:v",
        "5",
        "-an",
        "-sn",
    ])
    .arg(out)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    match cmd.status().await {
        Ok(status) => status.success() && out.is_file(),
        Err(_) => false,
    }
}

async fn extract_frame(ffmpeg: &Path, src: &Path, t: f64, out: &Path) -> Result<()> {
    let _ = tokio::fs::remove_file(out).await;
    if try_extract(ffmpeg, src, t, out, true).await {
        return Ok(());
    }
    if try_extract(ffmpeg, src, t, out, false).await {
        return Ok(());
    }
    bail!("could not extract frame at {:.3}s", t)
}

pub async fn ensure_thumb(state: &Arc<AppState>, v: &VideoDto, index: usize) -> Option<PathBuf> {
    let out = thumb_path(state, v, index);
    if out.is_file() {
        return Some(out);
    }
    let ffmpeg = state.ffmpeg()?;
    let lock = state.thumb_lock(v.id);
    let _guard = lock.lock().await;
    if out.is_file() {
        return Some(out);
    }
    tokio::fs::create_dir_all(thumb_dir(state, v)).await.ok()?;
    let times = thumb_times(v.duration_ms, THUMB_COUNT);
    let t = *times.get(index)?;
    let _permit = state.thumb_permits.acquire().await.ok()?;
    match extract_frame(&ffmpeg.ffmpeg, Path::new(&v.path), t, &out).await {
        Ok(()) => Some(out),
        Err(_) => None,
    }
}

pub async fn ensure_thumbs(state: &Arc<AppState>, v: &VideoDto) -> Vec<f64> {
    for i in 0..THUMB_COUNT {
        let _ = ensure_thumb(state, v, i).await;
    }
    thumb_times(v.duration_ms, THUMB_COUNT)
}

pub fn play_cache_path(state: &AppState, v: &VideoDto) -> PathBuf {
    state
        .cache_dir
        .join("play")
        .join(format!("{}_{}_{}.mp4", v.id, v.size, v.mtime))
}

fn set_conversion(state: &AppState, id: i64, st: &str, progress: f64, error: Option<String>) {
    if let Ok(mut jobs) = state.conversions.lock() {
        jobs.insert(
            id,
            Conversion {
                state: st.to_string(),
                progress,
                error,
            },
        );
    }
}

pub fn spawn_conversion(state: Arc<AppState>, v: VideoDto) {
    let out = play_cache_path(&state, &v);
    if out.is_file() {
        return;
    }
    {
        let jobs = state.conversions.lock().unwrap();
        if let Some(job) = jobs.get(&v.id) {
            if job.state == "running" || job.state == "queued" {
                return;
            }
        }
    }
    set_conversion(&state, v.id, "queued", -1.0, None);

    tauri::async_runtime::spawn(async move {
        let permit = state.convert_permits.clone().acquire_owned().await.ok();
        set_conversion(&state, v.id, "running", -1.0, None);
        let result = run_conversion(&state, &v, &out).await;
        match result {
            Ok(()) => set_conversion(&state, v.id, "done", 1.0, None),
            Err(e) => set_conversion(&state, v.id, "error", 0.0, Some(format!("{e:#}"))),
        }
        drop(permit);
        enforce_cache_limit(&state);
    });
}

pub async fn run_conversion(state: &Arc<AppState>, v: &VideoDto, out: &Path) -> Result<()> {
    if let Some(parent) = out.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let ffmpeg = state.ffmpeg().ok_or_else(|| {
        anyhow!("ffmpeg was not found. Install ffmpeg or set its folder in Settings.")
    })?;

    let remux = v.playback == "remux";
    let tmp = out.with_extension("part.mp4");
    let _ = tokio::fs::remove_file(&tmp).await;

    let result = ffmpeg_convert(state, &ffmpeg, v, &tmp, remux).await;
    let result = match result {
        Ok(()) => Ok(()),
        Err(e) if remux => {
            // Remux can fail on unusual files (broken timestamps, odd codecs).
            // Fall back to a full re-encode.
            let _ = tokio::fs::remove_file(&tmp).await;
            let _ = e;
            ffmpeg_convert(state, &ffmpeg, v, &tmp, false).await
        }
        Err(e) => Err(e),
    };
    result?;

    if !tmp.is_file() {
        bail!("ffmpeg did not produce an output file");
    }
    let _ = tokio::fs::remove_file(out).await;
    tokio::fs::rename(&tmp, out).await?;
    Ok(())
}

async fn ffmpeg_convert(
    state: &Arc<AppState>,
    ffmpeg: &Ffmpeg,
    v: &VideoDto,
    out: &Path,
    remux: bool,
) -> Result<()> {
    let mut cmd = quiet_command(&ffmpeg.ffmpeg);
    cmd.args(["-hide_banner", "-v", "error", "-nostdin", "-y"]);
    cmd.arg("-i").arg(&v.path);
    cmd.args(["-map", "0:v:0", "-map", "0:a:0?"]);
    if remux {
        cmd.args(["-c:v", "copy"]);
    } else {
        cmd.args([
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "21", "-pix_fmt", "yuv420p",
        ]);
    }
    cmd.args(["-c:a", "aac", "-b:a", "160k"]);
    cmd.args(["-movflags", "+faststart", "-f", "mp4"]);
    cmd.args(["-progress", "pipe:1", "-nostats"]);
    cmd.arg(out);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("could not start ffmpeg")?;
    let stderr = child.stderr.take().unwrap();
    let err_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut buf).await;
        buf
    });
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let total_us = v.duration_ms.filter(|d| *d > 0).unwrap_or(0) as f64 * 1000.0;

    while let Some(line) = lines.next_line().await? {
        let value = line
            .strip_prefix("out_time_us=")
            .or_else(|| line.strip_prefix("out_time_ms="));
        if let (Some(raw), true) = (value, total_us > 0.0) {
            if let Ok(us) = raw.trim().parse::<f64>() {
                let pct = (us / total_us).clamp(0.0, 0.999);
                set_conversion(state, v.id, "running", pct, None);
            }
        }
    }

    let status = child.wait().await?;
    let stderr = err_task.await.unwrap_or_default();
    if !status.success() {
        let tail: String = stderr
            .trim()
            .lines()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        bail!("ffmpeg exited with {}: {}", status, tail);
    }
    Ok(())
}

/* ---------------------------- video DNA -------------------------------- */

pub const DNA_FRAMES: usize = 8;

pub async fn compute_dna(ffmpeg: &Path, file: &Path, duration_ms: Option<i64>) -> Result<Vec<u64>> {
    let duration = duration_ms.filter(|d| *d > 0).map(|d| d as f64 / 1000.0);
    let times: Vec<f64> = match duration {
        Some(seconds) if seconds > 0.5 => (0..DNA_FRAMES)
            .map(|i| seconds * (i as f64 + 0.5) / DNA_FRAMES as f64)
            .collect(),
        _ => (0..DNA_FRAMES).map(|i| 0.5 + i as f64).collect(),
    };
    let mut hashes = Vec::with_capacity(times.len());
    for time in times {
        if let Ok(hash) = frame_hash(ffmpeg, file, time).await {
            hashes.push(hash);
        }
    }
    if hashes.len() < 4 {
        bail!("could not sample enough frames");
    }
    Ok(hashes)
}

async fn frame_hash(ffmpeg: &Path, file: &Path, time: f64) -> Result<u64> {
    let output = quiet_command(ffmpeg)
        .args(["-v", "error", "-nostdin", "-ss"])
        .arg(format!("{:.3}", time))
        .arg("-i")
        .arg(file)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=9:8:flags=area,format=gray",
            "-f",
            "rawvideo",
            "-",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await?;
    if output.stdout.len() < 72 {
        bail!("no frame at {time:.1}s");
    }
    let bytes = &output.stdout;
    let mut hash = 0u64;
    let mut bit = 0;
    for y in 0..8 {
        for x in 0..8 {
            if bytes[y * 9 + x] > bytes[y * 9 + x + 1] {
                hash |= 1u64 << bit;
            }
            bit += 1;
        }
    }
    Ok(hash)
}

/* --------------------------- cache management -------------------------- */

pub fn dir_size(path: &Path) -> u64 {
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

struct CacheFile {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

fn collect_cache_files(dir: &Path, out: &mut Vec<CacheFile>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = entry.metadata() {
                if meta.is_dir() {
                    collect_cache_files(&path, out);
                } else {
                    out.push(CacheFile {
                        path,
                        size: meta.len(),
                        modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    });
                }
            }
        }
    }
}

/// Evict the oldest generated previews/conversions until the cache is back
/// under the configured limit (90% of it, to avoid thrashing).
pub fn enforce_cache_limit(state: &AppState) -> u64 {
    let limit_mb: u64 = state
        .setting("cache_limit_mb")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4096);
    if limit_mb == 0 {
        return 0;
    }
    let limit = limit_mb.saturating_mul(1024 * 1024);
    let mut files = Vec::new();
    for sub in ["thumbs", "play"] {
        collect_cache_files(&state.cache_dir.join(sub), &mut files);
    }
    let total: u64 = files.iter().map(|f| f.size).sum();
    if total <= limit {
        return 0;
    }
    files.sort_by_key(|f| f.modified);
    let now = SystemTime::now();
    let mut current = total;
    let mut freed = 0;
    for file in files {
        if current <= limit * 9 / 10 {
            break;
        }
        if let Ok(age) = now.duration_since(file.modified) {
            if age.as_secs() < 120 {
                continue; // probably still being written
            }
        }
        if std::fs::remove_file(&file.path).is_ok() {
            freed += file.size;
            current = current.saturating_sub(file.size);
        }
    }
    freed
}
