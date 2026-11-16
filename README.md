# Splashhound

Created by **Leetdog**.

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
**Settings → ffmpeg**. On the first launch without ffmpeg it shows a setup
prompt with the install command and a "Check again" button.

## Install (prebuilt)

Prebuilt binaries are attached to
[Releases](https://github.com/buddyLeetdog/splashhound/releases):

- **Windows**: unzip `splashhound-windows-x64.zip` and run `splashhound.exe`.
  WebView2 is required (preinstalled on Windows 10/11); ffmpeg is recommended.
- **Ubuntu / Debian**: install the `.deb` with
  `sudo apt install ./splashhound_1.0.0_amd64.deb`. Requires Ubuntu 24.04+ or
  Debian 13+ (WebKitGTK 4.1).
- **Other Linux**: unpack `splashhound-linux-x64.tar.gz` and run
  `./splashhound`. It is built against Ubuntu 24.04, so it needs glibc 2.39+
  and WebKitGTK 4.1, GTK 3 + libayatana-appindicator (Arch: `webkit2gtk-4.1`,
  `gtk3`, `libsoup3`, `libayatana-appindicator`).
- **Arch Linux**: build with `packaging/arch/PKGBUILD`
  (`cd packaging/arch && makepkg -si`), or install the `splashhound` AUR
  package once published.

The Windows, Linux and `.deb` builds are produced by GitHub Actions for every
`v*` tag; see `packaging/README.md` for local packaging and AUR instructions.

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

### Stats
Collection overview: videos, reviewed share, total size and duration, never
viewed, recently added, videos without toys or tags, trash and DNA progress,
plus rating distribution, toy counts, creators and folder counts.

### Old codecs
Lists every video whose streams the built-in player cannot decode — WMV/VC-1,
MPEG-1/2, MPEG-4 ASP (Xvid/DivX), Sorenson FLV, Indeo, MJPEG and high-bit-depth
H.264 — with codec, resolution, size and length. Select files and either:

- **Build playback cache** — converts them into the playback cache. Nothing on
  disk changes, playback starts instantly afterwards, and the entries count
  toward the cache limit.
- **Convert to MP4** — writes a real `.mp4` next to the original, adds it to the
  library with the same tags, rating and flags, and optionally moves the source
  to `_trash`.

Both run in the background with a progress bar, a cancel button and a list of
failures.

### Settings
Library folder, scan progress/stop, ffmpeg location, database/cache/trash
paths, hashing mode, cache limit, tag management, metadata export/backup and
library statistics.

## Index

By default the SQLite database lives in the app's data folder:

- Windows — `%APPDATA%\com.splashhound.desktop\library.db`
- Linux — `~/.local/share/com.splashhound.desktop/library.db`
  (or `$XDG_DATA_HOME/com.splashhound.desktop/library.db`)

**Settings → Storage → Change folder…** moves it elsewhere: the database is
copied to the new folder, the
current file is kept as a backup, and an existing `library.db` at the target is
renamed aside first. The chosen location is remembered in `config.json` next to
the default folder, so the app finds it on startup.

`videos` columns: file name, path, size, modification time, duration, width,
height, video/audio codec, has-audio, MD5, creator, NSFW, popping, destruction,
deflation, inflation, notes, rating (0–5), reviewed, view count, last viewed,
date added, missing.

Each row also stores its path relative to the library root. If you move the
collection to another disk, choose the new folder in Settings: when the
relative layout matches, all entries are re-linked in place and keep their
tags, ratings and toys. Files that no longer exist are flagged as missing;
**Settings → Data → Remove missing entries** clears those rows.

Tags live in a separate `tags` table with a `video_tags` join table, so the
"other tags" field is a proper many-to-many tag set. Toys use their own
`toys` / `video_toys` tables, so the inflatable shown in a video can be tagged
and filtered independently of general tags.

Generated preview frames and converted playback files are cached in
`%LOCALAPPDATA%\com.splashhound.desktop` on Windows and
`~/.cache/com.splashhound.desktop` (`$XDG_CACHE_HOME`) on Linux, and can be
cleared from Settings.

## Indexing and cache

By default only files whose size collides with another file are hashed — a
unique size can never be byte-identical to anything else, so this avoids
reading the whole library. Switch to "hash every file" in Settings for full MD5
coverage. Probing and hashing show progress with a time estimate.

The cache holding previews and converted playback files has a size limit
(default 4 GB, configurable) and evicts the oldest entries automatically.

## Near-duplicates (Video DNA)

The Duplicates view has **Exact (MD5)** and **Similar (Video DNA)** tabs. DNA
analysis samples frames across each video, hashes them perceptually and compares
the sequences, which finds re-encodes or different resolutions that MD5 cannot.
It runs on demand from that tab, is incremental and cancellable, and persists.

## Extra workflow

- **Folders**: filter the library by subfolder; "Rate this folder" scopes the
  rating queue to the current folder selection.
- **Batch editing**: click Select, pick many cards, then set tags, toys, creator,
  flags, rating or reviewed state for all of them.
- **Trash**: files moved out of the library are recorded there and can be
  restored with their tags/ratings, deleted individually, or selected in bulk
  (restore selected / delete selected / delete all).
- **Deleting videos**: the detail view and the Rate view have a "Move to trash"
  button, so removing something is always reversible.
- **Detail navigation**: Prev/Next buttons and the left/right arrow keys move
  through the current view, including while the player is fullscreen (fullscreen
  is kept when the video changes). Shift+Left/Right seeks 10 seconds,
  Ctrl+Left/Right seeks 1 minute.
- **Duplicates**: click a filename in either duplicate tab to open the detail
  player for that exact copy.
- **Tags**: rename (merging into an existing name) or delete tags in Settings.
- **Visible fields**: Settings can hide the NSFW, Popping or Destruction flags
  from the editor, filters, badges, bulk edit and shortcuts. Data is kept, so
  unhiding them brings everything back.
- **Bulk duplicate cleanup**: "Keep best in all" in either Duplicates tab keeps
  the highest-quality copy of every group and moves the rest to trash.
- **Quick chips**: the most-used toys and tags appear as dashed one-click chips
  under the toy/tag inputs.
- **Discreet mode**: blurs posters, thumbnails and names until you hover; toggle
  it in Settings or with Ctrl+H.
- **Data**: export/import tags, ratings and flags as JSON, and back up the
  database file.
- **Scrolling**: the library loads the next page automatically when you reach
  the bottom.

## Playback

- MP4/MOV/WebM with H.264, VP8, VP9 or AV1 play directly.
- Any other container that holds a decodable video stream — MKV, TS/M2TS, FLV,
  AVI, MPG — is remuxed to a cached MP4. The video is copied bit for bit; the
  audio is copied as well when MP4 can hold it (AAC, MP3) and re-encoded to AAC
  only when it cannot (AC3, DTS, PCM, …).
- WMV/VC-1, MPEG-1, MPEG-2, MPEG-4 ASP (Xvid/DivX), Sorenson FLV, Indeo and
  MJPEG are transcoded to H.264/AAC once and cached.
- H.264 must be 8-bit 4:2:0. Hi10P and 4:2:2 files are transcoded because
  WebView2 cannot decode them.

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
  make_icons.py     regenerate the placeholder app icons (.png + .ico)
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
