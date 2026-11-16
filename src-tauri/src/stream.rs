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
        cmd.args(["-vaapi_device", "/dev/dri/renderD128"]);
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

/// Encoder arguments for the chosen codec, matching the codec string the
/// frontend asked for (high profile, level from the resolution).
fn encoder_args(name: &str, level: &str, gop: i64) -> Vec<String> {
    let mut args: Vec<String> = match name {
        "h264_nvenc" => vec![
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p4",
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
        "h264_vaapi" => vec![
            "-vaapi_device",
            "/dev/dri/renderD128",
            "-c:v",
            "h264_vaapi",
            "-qp",
            "23",
            "-vf",
            "format=nv12,hwupload",
        ],
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

    // VAAPI uploads frames to the GPU and must not be given a raw pixel format.
    if name != "h264_vaapi" {
        args.extend(
            [
                "-profile:v",
                "high",
                "-level:v",
                level,
                "-pix_fmt",
                "yuv420p",
            ]
            .into_iter()
            .map(str::to_string),
        );
    }
    args.extend(
        [
            "-g".to_string(),
            gop.to_string(),
            "-keyint_min".to_string(),
            gop.to_string(),
        ]
        .into_iter(),
    );
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
    encoder: &str,
) -> tokio::process::Command {
    let mut cmd = quiet_command(ffmpeg);
    cmd.args(["-hide_banner", "-v", "error", "-nostdin"]);
    // Realtime pacing keeps the buffer small; seeking opens a new connection.
    cmd.args(["-re"]);
    if start > 0.5 {
        cmd.arg("-ss").arg(format!("{start:.3}"));
    }
    cmd.arg("-i").arg(path);
    cmd.args(["-map", "0:v:0", "-map", "0:a:0?"]);
    cmd.args(encoder_args(encoder, level, gop));
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
    // Watches for the player going away. Closing the modal or switching video
    // must stop the encoder even while it is blocked writing.
    let mut closed = tokio::spawn(async move {
        while let Some(message) = stream.next().await {
            match message {
                Ok(Message::Close(_)) | Err(_) => return,
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
    let mut encoders: Vec<&str> = Vec::new();
    if let Some(hw) = hardware {
        encoders.push(hw);
    }
    encoders.push("libx264");

    let mut sent = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut child = None;

    for encoder in encoders {
        let mut spawned =
            match build_command(&ffmpeg.ffmpeg, &video.path, start, fps, level, gop, encoder)
                .spawn()
            {
                Ok(child) => child,
                Err(err) => {
                    errors.push(format!("{encoder}: {err}"));
                    continue;
                }
            };
        let mut stderr = spawned.stderr.take().unwrap();
        let mut stdout = spawned.stdout.take().unwrap();
        let guard = Decoder(spawned);

        // Hand the socket to the first encoder that produces data; if it dies
        // early, fall back to the next one (hardware -> software).
        match stream_with(&mut sink, &mut closed, &mut stdout, &mut stderr, encoder).await {
            Ok(bytes) => {
                sent = bytes;
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
                errors.push(format!("{encoder}: {reason}"));
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
    }
    let _ = sink.send(Message::Close(None)).await;
    closed.abort();
    drop(child);
}

/// Streams one encoder's output. Returns the byte count, or an error plus the
/// byte count when it stopped part way through.
async fn stream_with(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    closed: &mut tokio::task::JoinHandle<()>,
    stdout: &mut tokio::process::ChildStdout,
    stderr: &mut tokio::process::ChildStderr,
    encoder: &str,
) -> Result<usize, (String, usize)> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut sent = 0usize;
    loop {
        let read = tokio::select! {
            read = stdout.read(&mut buf) => read,
            _ = &mut *closed => return Ok(sent),
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
            _ = &mut *closed => return Ok(sent),
        };
        if failed {
            return Ok(sent);
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
    Ok(sent)
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
        let args = encoder_args("libx264", "4.1", 30);
        assert!(args.contains(&"libx264".to_string()));
        assert!(args.contains(&"ultrafast".to_string()));
        let hw = encoder_args("h264_nvenc", "4.1", 30);
        assert!(hw.contains(&"h264_nvenc".to_string()));
        assert!(hw.contains(&"4.1".to_string()));
    }
}
