use std::{path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    extract::{Path as AxPath, Request, State as AxState},
    http::{header, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

use crate::{db, media, stream, AppState};

/// Local HTTP server used only for media streaming (range requests) and
/// generated thumbnails. Data flows through Tauri IPC instead.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/media/{id}", get(media_handler))
        .route("/thumb/{id}/{index}", get(thumb_handler))
        .route("/poster/{id}", get(poster_handler))
        .route("/lowres/{id}", get(lowres_handler))
        .route("/stream/{id}", get(stream::upgrade))
        .with_state(state)
}

pub fn spawn(state: Arc<AppState>, listener: std::net::TcpListener) {
    let app = router(state);

    tauri::async_runtime::spawn(async move {
        match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => {
                if let Err(e) = axum::serve(listener, app).await {
                    eprintln!("media server stopped: {e}");
                }
            }
            Err(e) => eprintln!("media server could not start: {e}"),
        }
    });
}

async fn media_handler(
    AxState(state): AxState<Arc<AppState>>,
    AxPath(id): AxPath<i64>,
    req: Request,
) -> Response {
    let Some(video) = state.video(id) else {
        return (StatusCode::NOT_FOUND, "video not found").into_response();
    };
    let direct = video.playback == "direct";
    let path = if direct {
        PathBuf::from(&video.path)
    } else {
        media::play_cache_path(&state, &video)
    };
    if !path.is_file() {
        if !direct {
            media::spawn_conversion(state.clone(), video);
        }
        return (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response();
    }
    let mime = if direct {
        mime_for_ext(&video.ext)
    } else {
        "video/mp4"
    };
    let (is_head, range) = request_parts(&req);
    serve_file(is_head, range, &path, mime).await
}

async fn poster_handler(
    AxState(state): AxState<Arc<AppState>>,
    AxPath(id): AxPath<i64>,
    req: Request,
) -> Response {
    let Some(video) = state.video(id) else {
        return (StatusCode::NOT_FOUND, "video not found").into_response();
    };
    let index = (media::THUMB_COUNT / 2).min(media::THUMB_COUNT - 1);
    let (is_head, range) = request_parts(&req);
    match media::ensure_thumb(&state, &video, index).await {
        Some(path) => {
            // Keep the database placeholder in sync with the cached poster. Do it
            // in the background so serving the poster is not delayed.
            let bg_state = state.clone();
            let bg_video = video.clone();
            tauri::async_runtime::spawn(async move {
                media::ensure_lowres(&bg_state, &bg_video).await;
            });
            image_response(is_head, range, &path).await
        }
        None => (StatusCode::NOT_FOUND, "no poster").into_response(),
    }
}

/// Tiny placeholder stored in the database so grid cards paint immediately.
async fn lowres_handler(
    AxState(state): AxState<Arc<AppState>>,
    AxPath(id): AxPath<i64>,
) -> Response {
    let bytes = {
        let Ok(conn) = state.db.lock() else {
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        };
        db::thumb_small(&conn, id)
    };
    match bytes {
        Some(bytes) => {
            let mut res = Response::new(Body::from(bytes));
            let headers = res.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=604800"),
            );
            res
        }
        None => (StatusCode::NOT_FOUND, "no placeholder").into_response(),
    }
}

async fn thumb_handler(
    AxState(state): AxState<Arc<AppState>>,
    AxPath((id, index)): AxPath<(i64, usize)>,
    req: Request,
) -> Response {
    let Some(video) = state.video(id) else {
        return (StatusCode::NOT_FOUND, "video not found").into_response();
    };
    if index >= media::THUMB_COUNT {
        return (StatusCode::NOT_FOUND, "bad index").into_response();
    }
    let (is_head, range) = request_parts(&req);
    match media::ensure_thumb(&state, &video, index).await {
        Some(path) => image_response(is_head, range, &path).await,
        None => (StatusCode::NOT_FOUND, "no thumbnail").into_response(),
    }
}

fn request_parts(req: &Request) -> (bool, Option<String>) {
    let is_head = req.method() == Method::HEAD;
    let range = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    (is_head, range)
}

async fn image_response(is_head: bool, range: Option<String>, path: &std::path::Path) -> Response {
    let mut res = serve_file(is_head, range, path, "image/jpeg").await;
    if res.status().is_success() {
        res.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=604800"),
        );
    }
    res
}

fn mime_for_ext(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "ogv" | "ogg" => "video/ogg",
        _ => "application/octet-stream",
    }
}

fn parse_range(value: &str, len: u64) -> Result<(u64, u64), ()> {
    if len == 0 {
        return Err(());
    }
    let value = value.trim();
    let spec = value.strip_prefix("bytes=").ok_or(())?;
    let (start_raw, end_raw) = spec.split_once('-').ok_or(())?;
    if start_raw.trim().is_empty() {
        let suffix: u64 = end_raw.trim().parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = len.saturating_sub(suffix);
        Ok((start, len - 1))
    } else {
        let start: u64 = start_raw.trim().parse().map_err(|_| ())?;
        let end: u64 = if end_raw.trim().is_empty() {
            len - 1
        } else {
            end_raw.trim().parse().map_err(|_| ())?
        };
        let end = end.min(len - 1);
        if start >= len || start > end {
            return Err(());
        }
        Ok((start, end))
    }
}

async fn serve_file(
    is_head: bool,
    range_header: Option<String>,
    path: &std::path::Path,
    mime: &'static str,
) -> Response {
    let meta = match tokio::fs::metadata(path).await {
        Ok(meta) => meta,
        Err(_) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
    };
    let len = meta.len();
    match range_header.map(|h| parse_range(&h, len)) {
        Some(Err(())) => {
            let mut res = (StatusCode::RANGE_NOT_SATISFIABLE, "").into_response();
            if let Ok(value) = HeaderValue::from_str(&format!("bytes */{len}")) {
                res.headers_mut().insert(header::CONTENT_RANGE, value);
            }
            res
        }
        Some(Ok((start, end))) => {
            let mut file = match tokio::fs::File::open(path).await {
                Ok(f) => f,
                Err(_) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
            };
            if file.seek(SeekFrom::Start(start)).await.is_err() {
                return (StatusCode::INTERNAL_SERVER_ERROR, "seek failed").into_response();
            }
            let length = end - start + 1;
            let body = if is_head {
                Body::empty()
            } else {
                Body::from_stream(ReaderStream::new(file.take(length)))
            };
            let mut res = Response::new(body);
            *res.status_mut() = StatusCode::PARTIAL_CONTENT;
            let headers = res.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
            headers.insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&length.to_string()).unwrap(),
            );
            headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{len}")).unwrap(),
            );
            headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            res
        }
        None => {
            let body = if is_head {
                Body::empty()
            } else {
                match tokio::fs::File::open(path).await {
                    Ok(file) => Body::from_stream(ReaderStream::new(file)),
                    Err(_) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
                }
            };
            let mut res = Response::new(body);
            let headers = res.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
            headers.insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).unwrap(),
            );
            headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            res
        }
    }
}
