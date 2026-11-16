//! Built-in stream player.
//!
//! Files the webview cannot decode (and files with variable frame rate) are
//! decoded by ffmpeg and re-muxed as a fragmented MP4 that is fed to a
//! `MediaSource` over a WebSocket. That gives full codec coverage, constant
//! frame rate output and normal A/V sync, using only the ffmpeg build the app
//! already depends on — no extra libraries and the same path on Windows/Linux.

use std::process::Stdio;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;

use crate::AppState;

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

    let mut cmd = quiet_command(&ffmpeg.ffmpeg);
    cmd.args(["-hide_banner", "-v", "error", "-nostdin"]);
    // Realtime pacing keeps the buffer small; seeking opens a new connection.
    cmd.args(["-re"]);
    if start > 0.5 {
        cmd.arg("-ss").arg(format!("{start:.3}"));
    }
    cmd.arg("-i").arg(&video.path);
    cmd.args(["-map", "0:v:0", "-map", "0:a:0?"]);
    cmd.args([
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-tune",
        "zerolatency",
        "-profile:v",
        "high",
        "-level:v",
        level,
        "-crf",
        "23",
        "-pix_fmt",
        "yuv420p",
        // Constant frame rate output fixes variable-rate sources.
        "-fps_mode",
        "cfr",
        "-r",
        &format!("{fps:.3}"),
        // One keyframe per second, so the muxer emits a fragment every second
        // instead of buffering several seconds before the first one is complete.
        "-g",
        &gop.to_string(),
        "-keyint_min",
        &gop.to_string(),
        "-sc_threshold",
        "0",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-ac",
        "2",
        "-movflags",
        "empty_moov+default_base_moof+frag_keyframe",
        "-f",
        "mp4",
        "pipe:1",
    ]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let Ok(mut spawned) = cmd.spawn() else {
        let _ = sink
            .send(Message::Text("could not start the decoder".into()))
            .await;
        return;
    };
    let mut stderr = spawned.stderr.take().unwrap();
    let mut stdout = spawned.stdout.take().unwrap();
    let mut child = Decoder(spawned);
    let err_task = tokio::spawn(async move {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text).await;
        text
    });

    let mut buf = vec![0u8; 64 * 1024];
    let mut sent = 0usize;
    loop {
        let read = tokio::select! {
            read = stdout.read(&mut buf) => read,
            _ = &mut closed => break,
        };
        let Ok(n) = read else { break };
        if n == 0 {
            break;
        }
        sent += n;
        let chunk = Message::Binary(buf[..n].to_vec().into());
        let failed = tokio::select! {
            result = sink.send(chunk) => result.is_err(),
            _ = &mut closed => true,
        };
        if failed {
            break;
        }
    }

    // Kill before waiting: a decoder blocked on a full stdout pipe would
    // otherwise never exit and the process would be left behind.
    let _ = child.0.kill().await;
    let status = child.0.wait().await;
    closed.abort();
    let stderr_text = err_task.await.unwrap_or_default();
    if sent == 0 {
        // Nothing was decoded: tell the player why instead of showing a blank frame.
        let tail: String = stderr_text
            .trim()
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        let message = match status {
            Ok(st) if !st.success() => format!("decoder failed: {tail}"),
            _ => "the decoder produced no video".to_string(),
        };
        let _ = sink.send(Message::Text(message.into())).await;
    }
    let _ = sink.send(Message::Close(None)).await;
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
}
