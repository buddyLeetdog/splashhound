//! Built-in stream player.
//!
//! Files the webview cannot decode (and files with variable frame rate) are
//! decoded by ffmpeg and re-muxed as a fragmented MP4 that is fed to a
//! `MediaSource` over a WebSocket. That gives full codec coverage, constant
//! frame rate output and normal A/V sync, using only the ffmpeg build the app
//! already depends on — no extra libraries and the same path on Windows/Linux.

use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;

use crate::AppState;

/// H.264 encoders in order of preference. Each is test-encoded once against a
/// small synthetic clip, because builds often list encoders the machine cannot
/// actually use (missing GPU, driver or session limit).
const HW_CANDIDATES: &[&str] = &["h264_nvenc", "h264_qsv", "h264_amf", "h264_vaapi"];

/// How far ahead of the playhead the decoder may run by default. Everything
/// inside this window can be seeked without restarting the encoder, so it trades
/// memory for instant seeking. Configurable in Settings.
const DEFAULT_AHEAD_MS: u64 = 30_000;

/// Cap for the configured window, so a typo cannot buffer minutes of video.
const MAX_WINDOW_MS: u64 = 120_000;

static HW_ENCODER: OnceLock<Option<&'static str>> = OnceLock::new();

/// Encoder level derived from the source resolution: the MSE codec string on
/// the frontend has to match it, so both sides use the same rule.
pub fn profile_for(width: Option<i64>, height: Option<i64>) -> (&'static str, &'static str) {
    let _ = width;
    let h = height.unwrap_or(0);
    if h > 1080 {
        ("5.1", "avc1.640033")
    } else if h > 720 {
        ("4.1", "avc1.640029")
    } else {
        ("4.0", "avc1.640028")
    }
}

/// The best usable hardware encoder, or `None` for libx264.
pub fn hardware_encoder(ffmpeg: &Path) -> Option<&'static str> {
    *HW_ENCODER.get_or_init(|| {
        for candidate in HW_CANDIDATES {
            if encoder_works(ffmpeg, candidate) {
                return Some(*candidate);
            }
        }
        None
    })
}

/// First usable VAAPI render node. Most systems expose `renderD128`, but some
/// drivers enumerate it under another number.
fn vaapi_device() -> String {
    for index in 128..136 {
        let node = format!("/dev/dri/renderD{index}");
        if Path::new(&node).exists() {
            return node;
        }
    }
    "/dev/dri/renderD128".to_string()
}

fn encoder_works(ffmpeg: &Path, name: &str) -> bool {
    let mut cmd = std::process::Command::new(ffmpeg);
    // 640x360: some hardware encoders reject very small frames.
    cmd.args([
        "-hide_banner",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=640x360:r=30:d=0.3",
    ]);
    if name == "h264_vaapi" {
        cmd.arg("-vaapi_device").arg(vaapi_device());
    }
    cmd.args(["-c:v", name]);
    if name == "h264_vaapi" {
        cmd.args(["-vf", "format=nv12,hwupload"]);
    }
    cmd.args(["-f", "null", "-"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// One way of producing the stream: encoder plus optional hardware decoding.
#[derive(Clone, Copy)]
struct Attempt {
    encoder: &'static str,
    /// (hwaccel name, output format) for GPU decoding of the source.
    hwaccel: Option<(&'static str, &'static str)>,
}

/// Codecs that hardware decoders actually handle; everything else is decoded by
/// ffmpeg on the CPU so a failed attempt does not delay playback.
fn codec_supports_hwaccel(codec: Option<&str>) -> bool {
    matches!(
        codec.map(|c| c.to_ascii_lowercase()).as_deref(),
        Some("h264") | Some("hevc") | Some("h265") | Some("vp9") | Some("av1") | Some("mpeg2video")
    )
}

/// Encoder/decoder combinations to try in order: hardware decode first, then the
/// same encoder on a CPU-decoded frame, then libx264.
fn attempts(hardware: Option<&'static str>, codec: Option<&str>) -> Vec<Attempt> {
    let mut out = Vec::new();
    if let Some(encoder) = hardware {
        let hw = match encoder {
            "h264_nvenc" => Some(("cuda", "cuda")),
            "h264_qsv" => Some(("qsv", "qsv")),
            "h264_vaapi" => Some(("vaapi", "vaapi")),
            "h264_amf" => Some(("d3d11va", "d3d11")),
            _ => None,
        };
        if let Some((accel, format)) = hw {
            if codec_supports_hwaccel(codec) {
                out.push(Attempt {
                    encoder,
                    hwaccel: Some((accel, format)),
                });
            }
        }
        out.push(Attempt {
            encoder,
            hwaccel: None,
        });
    }
    out.push(Attempt {
        encoder: "libx264",
        hwaccel: None,
    });
    out
}

/// Encoder arguments for the chosen codec, matching the codec string the
/// frontend asked for (high profile, level from the resolution).
fn encoder_args(name: &str, level: &str, gop: i64, software_frames: bool) -> Vec<String> {
    let mut args: Vec<String> = match name {
        "h264_nvenc" => vec![
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p1",
            "-tune",
            "ll",
            "-rc",
            "vbr",
            "-cq",
            "23",
            "-b:v",
            "0",
        ],
        "h264_qsv" => vec![
            "-c:v",
            "h264_qsv",
            "-preset",
            "veryfast",
            "-global_quality",
            "23",
            "-look_ahead",
            "0",
        ],
        "h264_amf" => vec![
            "-c:v", "h264_amf", "-quality", "speed", "-rc", "cqp", "-qp_i", "23", "-qp_p", "23",
        ],
        "h264_vaapi" => vec!["-c:v", "h264_vaapi", "-qp", "23"],
        _ => vec![
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-crf",
            "23",
        ],
    }
    .into_iter()
    .map(str::to_string)
    .collect();

    // The render node varies between systems, so insert the detected one.
    if name == "h264_vaapi" {
        args.insert(0, vaapi_device());
        args.insert(0, "-vaapi_device".to_string());
        // Hardware-decoded frames are already on the GPU; only software frames
        // have to be uploaded. Adding the filter to a VAAPI-to-VAAPI pipeline
        // makes ffmpeg fail to negotiate the format.
        if software_frames {
            args.push("-vf".to_string());
            args.push("format=nv12,hwupload".to_string());
        }
    }

    // VAAPI uploads frames to the GPU and hardware decoding keeps them there;
    // forcing a raw pixel format in those cases makes ffmpeg stall.
    if name != "h264_vaapi" {
        args.extend(
            ["-profile:v", "high", "-level:v", level]
                .into_iter()
                .map(str::to_string),
        );
        if software_frames {
            args.push("-pix_fmt".into());
            args.push("yuv420p".into());
        }
    }
    args.extend([
        "-g".to_string(),
        gop.to_string(),
        "-keyint_min".to_string(),
        gop.to_string(),
    ]);
    if name == "libx264" {
        args.push("-sc_threshold".into());
        args.push("0".into());
    }
    args
}

pub async fn upgrade(
    state: axum::extract::State<std::sync::Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let start = query
        .get("t")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0)
        .max(0.0);
    ws.on_upgrade(move |socket| pump(state.0, id, start, socket))
}

/// Kills the decoder whenever it goes out of scope, including when the socket
/// task is dropped because the player was closed.
struct Decoder(tokio::process::Child);

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn quiet_command(program: &std::path::Path) -> tokio::process::Command {
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    tokio::process::Command::from(cmd)
}

fn build_command(
    ffmpeg: &Path,
    path: &str,
    start: f64,
    fps: f64,
    level: &str,
    gop: i64,
    attempt: Attempt,
) -> tokio::process::Command {
    let mut cmd = quiet_command(ffmpeg);
    cmd.args(["-hide_banner", "-v", "error", "-nostdin"]);
    // Hardware decoding keeps the source frames on the GPU; the encoder consumes
    // them directly when both sides agree on the format.
    if let Some((accel, format)) = attempt.hwaccel {
        if accel == "vaapi" {
            cmd.arg("-vaapi_device").arg(vaapi_device());
        }
        cmd.args(["-hwaccel", accel, "-hwaccel_output_format", format]);
    }
    if start > 0.5 {
        cmd.arg("-ss").arg(format!("{start:.3}"));
    }
    cmd.arg("-i").arg(path);
    cmd.args(["-map", "0:v:0", "-map", "0:a:0?"]);
    cmd.args(encoder_args(
        attempt.encoder,
        level,
        gop,
        attempt.hwaccel.is_none(),
    ));
    cmd.args([
        // Constant frame rate output fixes variable-rate sources.
        "-fps_mode",
        "cfr",
        "-r",
        &format!("{fps:.3}"),
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-ac",
        "2",
        // 100 ms fragments keep the first frame and seeks short.
        "-movflags",
        "empty_moov+default_base_moof",
        "-frag_duration",
        "50000",
        "-flush_packets",
        "1",
        "-f",
        "mp4",
        "pipe:1",
    ]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

async fn pump(state: std::sync::Arc<AppState>, id: i64, start: f64, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    // How much video the client currently has buffered, in milliseconds. Used to
    // throttle the decoder: it runs ahead of the playhead instead of in realtime,
    // so seeking inside that window needs no new encoder run.
    let ahead = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let ahead_reader = ahead.clone();
    let last_report = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let last_report_reader = last_report.clone();
    // Watches for the player going away. Closing the modal or switching video
    // must stop the encoder even while it is blocked writing.
    let mut closed = tokio::spawn(async move {
        while let Some(message) = stream.next().await {
            match message {
                Ok(Message::Close(_)) | Err(_) => return,
                Ok(Message::Text(text)) => {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(ms) = value.get("ahead").and_then(|v| v.as_f64()) {
                            ahead_reader
                                .store(ms.max(0.0) as u64, std::sync::atomic::Ordering::Relaxed);
                            last_report_reader
                                .store(now_ms(), std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                _ => {}
            }
        }
    });

    let Some(video) = state.video(id) else {
        let _ = sink.send(Message::Text("no such video".into())).await;
        return;
    };
    let Some(ffmpeg) = state.ffmpeg() else {
        let _ = sink
            .send(Message::Text("ffmpeg is required for this player".into()))
            .await;
        return;
    };

    let (level, _) = profile_for(video.width, video.height);
    let fps = video
        .fps
        .filter(|f| (10.0..=120.0).contains(f))
        .unwrap_or(30.0);
    let gop = fps.round().clamp(1.0, 60.0) as i64;

    let hardware = hardware_encoder(&ffmpeg.ffmpeg);
    let window_ms = state
        .setting("stream_window")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|seconds| (seconds.saturating_mul(1000)).clamp(3_000, MAX_WINDOW_MS))
        .unwrap_or(DEFAULT_AHEAD_MS);

    let mut sent = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut child = None;
    let mut reached_end = false;

    for attempt in attempts(hardware, video.vcodec.as_deref()) {
        let label = match attempt.hwaccel {
            Some((accel, _)) => format!("{}+{}", attempt.encoder, accel),
            None => attempt.encoder.to_string(),
        };
        let mut spawned =
            match build_command(&ffmpeg.ffmpeg, &video.path, start, fps, level, gop, attempt)
                .spawn()
            {
                Ok(child) => child,
                Err(err) => {
                    errors.push(format!("{label}: {err}"));
                    continue;
                }
            };
        let mut stderr = spawned.stderr.take().unwrap();
        let mut stdout = spawned.stdout.take().unwrap();
        let guard = Decoder(spawned);

        // Hand the socket to the first attempt that produces data; if it dies
        // early, fall back to the next one (hardware decode -> CPU decode ->
        // software encoder).
        match stream_with(
            &mut sink,
            &mut closed,
            &mut stdout,
            &mut stderr,
            &label,
            &ahead,
            &last_report,
            window_ms,
        )
        .await
        {
            Ok((bytes, eof)) => {
                sent = bytes;
                reached_end = eof;
                child = Some(guard);
                break;
            }
            Err((reason, bytes)) => {
                if bytes > 0 {
                    // It produced something, so do not start over with another
                    // encoder mid-stream.
                    sent = bytes;
                    child = Some(guard);
                    break;
                }
                errors.push(reason);
            }
        }
    }

    if sent == 0 {
        let message = if errors.is_empty() {
            "the decoder produced no video".to_string()
        } else {
            format!("decoder failed: {}", errors.join(" | "))
        };
        let _ = sink.send(Message::Text(message.into())).await;
    } else if reached_end {
        // Tell the player the source is done but there is still buffered video:
        // it finishes playing the buffer before showing "finished".
        let _ = sink.send(Message::Text("{\"eof\":true}".into())).await;
    }
    let _ = sink.send(Message::Close(None)).await;
    closed.abort();
    drop(child);
}

/// Streams one encoder's output. Returns the byte count, or an error plus the
/// byte count when it stopped part way through.
#[allow(clippy::too_many_arguments)]
async fn stream_with(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    closed: &mut tokio::task::JoinHandle<()>,
    stdout: &mut tokio::process::ChildStdout,
    stderr: &mut tokio::process::ChildStderr,
    encoder: &str,
    ahead: &std::sync::atomic::AtomicU64,
    last_report: &std::sync::atomic::AtomicU64,
    window_ms: u64,
) -> Result<(usize, bool), (String, usize)> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut sent = 0usize;
    let opened_at = now_ms();
    loop {
        let ahead_ms = ahead.load(std::sync::atomic::Ordering::Relaxed);
        let last = last_report.load(std::sync::atomic::Ordering::Relaxed);
        // Safety net: a player that stopped reporting is gone (closed window,
        // crashed renderer). Without this the encoder could idle forever when the
        // socket never delivers a close frame.
        let silent = if last == 0 {
            now_ms().saturating_sub(opened_at) > 8_000
        } else {
            now_ms().saturating_sub(last) > 12_000
        };
        if silent {
            return Ok((sent, false));
        }
        // Stop reading from ffmpeg once the client is far enough ahead; the
        // pipe backpressure pauses the encoder without killing it.
        if ahead_ms > window_ms {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(80)) => continue,
                _ = &mut *closed => return Ok((sent, false)),
            }
        }
        // Wait for the first buffer report before racing ahead.
        if last == 0 && now_ms().saturating_sub(opened_at) > 1_500 {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                _ = &mut *closed => return Ok((sent, false)),
            }
        }
        let read = tokio::select! {
            read = stdout.read(&mut buf) => read,
            _ = &mut *closed => return Ok((sent, false)),
        };
        let Ok(n) = read else {
            break;
        };
        if n == 0 {
            break;
        }
        sent += n;
        let chunk = Message::Binary(buf[..n].to_vec().into());
        let failed = tokio::select! {
            result = sink.send(chunk) => result.is_err(),
            _ = &mut *closed => return Ok((sent, false)),
        };
        if failed {
            return Ok((sent, false));
        }
    }
    if sent == 0 {
        let mut text = String::new();
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(400),
            stderr.read_to_string(&mut text),
        )
        .await;
        let tail: String = text
            .trim()
            .lines()
            .rev()
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        return Err((format!("{encoder} failed: {tail}"), 0));
    }
    Ok((sent, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_follows_resolution() {
        assert_eq!(profile_for(Some(1920), Some(1080)).0, "4.1");
        assert_eq!(profile_for(Some(1280), Some(720)).0, "4.0");
        assert_eq!(profile_for(Some(3840), Some(2160)).0, "5.1");
        assert_eq!(profile_for(None, None).0, "4.0");
    }

    #[test]
    fn software_encoder_is_the_fallback() {
        let args = encoder_args("libx264", "4.1", 30, true);
        assert!(args.contains(&"libx264".to_string()));
        assert!(args.contains(&"ultrafast".to_string()));
        let hw = encoder_args("h264_nvenc", "4.1", 30, false);
        assert!(hw.contains(&"h264_nvenc".to_string()));
        assert!(hw.contains(&"4.1".to_string()));
    }
}
