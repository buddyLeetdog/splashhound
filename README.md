# Splashhound

A local-first desktop tool for indexing, categorizing and browsing a video
collection. Built with Rust + Tauri 2, SQLite for the library, and a plain
HTML/JS frontend rendered in WebView2.

Everything runs on your machine: the only network listener is a
`127.0.0.1` HTTP server used to stream video with range requests and serve
generated preview frames.

## Requirements

- Windows 10/11 with the WebView2 runtime (preinstalled on current Windows)
- [ffmpeg](https://ffmpeg.org/) — required for previews and for playing
  formats WebView2 cannot decode natively (MKV, FLV, WMV, AVI, …)

```powershell
winget install Gyan.FFmpeg
```

The app also auto-detects ffmpeg installed to a custom folder via
**Settings → ffmpeg**.

## Running

```powershell
cargo run --manifest-path src-tauri/Cargo.toml
```

or build a binary:

```powershell
cargo build --release --manifest-path src-tauri/Cargo.toml
# -> src-tauri/target/release/splashhound.exe
```

To create an installer, install the Tauri CLI once and bundle:

```powershell
cargo install tauri-cli --version "^2"
cd src-tauri
cargo tauri build
```

## First use

1. **Settings → Choose…** and pick the root folder of your library.
   A scan starts automatically.
2. The scan walks the folder recursively, reads duration/resolution/codecs with
   `ffprobe`, computes an MD5 hash per file for duplicate detection, and flags
   files that disappeared as *missing*.
3. New videos are queued in the **Rate** view.

Folders named `_trash` are excluded from scans — that is where removed
duplicates go.

## Views

### Library
Grid of every indexed video with poster frame, duration, size, resolution,
rating and flag badges.

Filters: full-text search (name, path, creator, notes), tag chips
(any/all matching), NSFW, reviewed state, minimum rating, creator.
Sorting: newest added, name, rating, most viewed, longest, largest,
random. **Shuffle** re-rolls a random order. Click a card for the detail view
(player, 12 preview frames, editable metadata).

### Rate
One view for all uncategorized videos: large player, 12 frames sampled across
the video (click a frame to seek), and a panel for creator, NSFW, popping,
destruction, deflation, inflation, free-form tags, notes and a 1–5 star rating.

- `Enter` — save & next (marks the video reviewed when the box is ticked)
- `S` — skip (saves edits but keeps it unreviewed)
- `1`–`5` — rating
- `N` `P` `D` `F` `I` — toggle the flags

Tags autocomplete from previously used tags; new ones are created on save.

### Duplicates
Groups byte-identical files by MD5. Pick the keeper with the radio button or
**Auto-pick keeper** (most viewed, then oldest), then move the rest to
`<library>/_trash`. Nothing is deleted permanently.

### Settings
Library folder, scan progress/stop, ffmpeg location, database/cache/trash
paths, cache size and cleanup, library statistics.

## Index

SQLite database in `%APPDATA%\com.splashhound.desktop\library.db`.

`videos` columns: file name, path, size, modification time, duration, width,
height, video/audio codec, has-audio, MD5, creator, NSFW, popping, destruction,
deflation, inflation, notes, rating (0–5), reviewed, view count, last viewed,
date added, missing.

Tags live in a separate `tags` table with a `video_tags` join table, so the
"other tags" field is a proper many-to-many tag set.

Generated preview frames and converted playback files are cached in
`%LOCALAPPDATA%\com.splashhound.desktop` and can be cleared from Settings.

## Playback

- MP4/MOV/WebM with H.264/VP8/VP9/AV1 play directly.
- MKV/FLV/TS with H.264 are remuxed to a cached MP4 (fast, no quality loss).
- WMV/AVI/MPEG-2/etc. are transcoded to H.264/AAC once and cached.

The first playback of a non-native file shows conversion progress; afterwards
it starts immediately.

## Development

```
ui/                 frontend (index.html, styles.css, app.js — no build step)
src-tauri/src/
  lib.rs            app setup, shared state, headless dev mode
  db.rs             schema and queries
  media.rs          ffprobe, MD5 hashing, thumbnails, conversion
  scan.rs           folder walk + indexing pipeline
  server.rs         localhost media server (range requests, thumbs)
  commands.rs       Tauri IPC commands
tools/
  make_icons.py     regenerate app icons from the chosen design
  icon_proposals.py icon concept sketches (dog + swim ring, 5 palettes)
  ui_probe.py       drive the running app over the WebView2 devtools protocol
  seed_dev_db.py    point the dev database at a folder without clicking
```

Regenerating icons: `python tools/make_icons.py` writes every size plus
`icon.ico`. Because the icons are embedded by the Tauri codegen, a plain
`cargo build` may not pick them up — run `cargo clean -p splashhound` (or
`cargo clean --release -p splashhound`) and rebuild.

Headless indexing (also used for manual verification):

```powershell
splashhound.exe --scan "D:\path\to\videos"
splashhound.exe --scan "D:\path\to\videos" --serve   # keep the media server up
```

Debug builds accept remote debugging through WebView2:

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9222"
cargo run
python tools/ui_probe.py eval "document.querySelectorAll('.card').length"
```
