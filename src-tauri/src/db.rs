use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Result};
use rusqlite::{
    params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension, Row,
};
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS settings (
  key   TEXT PRIMARY KEY,
  value TEXT
);

CREATE TABLE IF NOT EXISTS videos (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  path        TEXT NOT NULL UNIQUE,
  rel_path    TEXT,
  file_name   TEXT NOT NULL,
  ext         TEXT NOT NULL DEFAULT '',
  size        INTEGER NOT NULL DEFAULT 0,
  mtime       INTEGER NOT NULL DEFAULT 0,
  duration_ms INTEGER,
  width       INTEGER,
  height      INTEGER,
  vcodec      TEXT,
  acodec      TEXT,
  has_audio   INTEGER NOT NULL DEFAULT 0,
  md5         TEXT,
  creator     TEXT,
  nsfw        INTEGER NOT NULL DEFAULT 0,
  popping     INTEGER NOT NULL DEFAULT 0,
  destruction INTEGER NOT NULL DEFAULT 0,
  deflation   INTEGER NOT NULL DEFAULT 0,
  inflation   INTEGER NOT NULL DEFAULT 0,
  notes       TEXT,
  rating      INTEGER NOT NULL DEFAULT 0,
  reviewed    INTEGER NOT NULL DEFAULT 0,
  view_count  INTEGER NOT NULL DEFAULT 0,
  last_viewed INTEGER,
  added_at    INTEGER NOT NULL DEFAULT 0,
  missing     INTEGER NOT NULL DEFAULT 0,
  hash_state  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS tags (
  id   INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_tags_name ON tags(name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS video_tags (
  video_id INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
  tag_id   INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (video_id, tag_id)
);

CREATE TABLE IF NOT EXISTS toys (
  id   INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_toys_name ON toys(name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS video_toys (
  video_id INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
  toy_id   INTEGER NOT NULL REFERENCES toys(id) ON DELETE CASCADE,
  PRIMARY KEY (video_id, toy_id)
);
CREATE INDEX IF NOT EXISTS idx_vtoy_toy ON video_toys(toy_id, video_id);

CREATE TABLE IF NOT EXISTS trash (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  original_path TEXT NOT NULL,
  trashed_path  TEXT NOT NULL,
  file_name     TEXT NOT NULL,
  size          INTEGER NOT NULL DEFAULT 0,
  md5           TEXT,
  meta_json     TEXT,
  trashed_at    INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS video_dna (
  video_id    INTEGER PRIMARY KEY REFERENCES videos(id) ON DELETE CASCADE,
  frames      TEXT NOT NULL,
  duration_ms INTEGER,
  width       INTEGER,
  height      INTEGER,
  computed_at INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_videos_md5 ON videos(md5);
CREATE INDEX IF NOT EXISTS idx_videos_size ON videos(size);
CREATE INDEX IF NOT EXISTS idx_videos_reviewed ON videos(reviewed);
CREATE INDEX IF NOT EXISTS idx_vt_tag ON video_tags(tag_id, video_id);
"#;

pub const VIDEO_COLS: &str = "v.id, v.path, v.file_name, v.ext, v.size, v.mtime, \
     v.duration_ms, v.width, v.height, v.vcodec, v.acodec, v.has_audio, v.md5, \
     v.creator, v.nsfw, v.popping, v.destruction, v.deflation, v.inflation, \
     v.notes, v.rating, v.reviewed, v.view_count, v.added_at, v.missing";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoDto {
    pub id: i64,
    pub path: String,
    pub file_name: String,
    pub ext: String,
    pub size: i64,
    pub mtime: i64,
    pub duration_ms: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub has_audio: bool,
    pub md5: Option<String>,
    pub creator: Option<String>,
    pub nsfw: bool,
    pub popping: bool,
    pub destruction: bool,
    pub deflation: bool,
    pub inflation: bool,
    pub notes: Option<String>,
    pub rating: i64,
    pub reviewed: bool,
    pub view_count: i64,
    pub added_at: i64,
    pub missing: bool,
    pub tags: Vec<String>,
    pub toys: Vec<String>,
    pub playback: String,
}

impl VideoDto {
    pub fn duration_seconds(&self) -> f64 {
        self.duration_ms
            .filter(|d| *d > 0)
            .map(|d| d as f64 / 1000.0)
            .unwrap_or(0.0)
    }
}

fn b(v: i64) -> bool {
    v != 0
}

pub fn video_from_row(r: &Row<'_>) -> rusqlite::Result<VideoDto> {
    let ext: String = r.get(3)?;
    let vcodec: Option<String> = r.get(9)?;
    let acodec: Option<String> = r.get(10)?;
    let duration: Option<i64> = r.get(6)?;
    Ok(VideoDto {
        id: r.get(0)?,
        path: r.get(1)?,
        file_name: r.get(2)?,
        ext,
        size: r.get(4)?,
        mtime: r.get(5)?,
        duration_ms: duration.filter(|d| *d > 0),
        width: r.get(7)?,
        height: r.get(8)?,
        vcodec: vcodec.clone(),
        acodec: acodec.clone(),
        has_audio: b(r.get(11)?),
        md5: r.get(12)?,
        creator: r.get(13)?,
        nsfw: b(r.get(14)?),
        popping: b(r.get(15)?),
        destruction: b(r.get(16)?),
        deflation: b(r.get(17)?),
        inflation: b(r.get(18)?),
        notes: r.get(19)?,
        rating: r.get(20)?,
        reviewed: b(r.get(21)?),
        view_count: r.get(22)?,
        added_at: r.get(23)?,
        missing: b(r.get(24)?),
        tags: Vec::new(),
        toys: Vec::new(),
        playback: crate::media::playback_str(
            &r.get::<_, String>(3)?,
            vcodec.as_deref(),
            acodec.as_deref(),
        )
        .to_string(),
    })
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(15))?;
    conn.execute_batch(SCHEMA)?;
    ensure_column(&conn, "videos", "hash_state", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(&conn, "videos", "rel_path", "TEXT")?;
    Ok(conn)
}

/// Add a column to an existing database created by an older build.
fn ensure_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    let mut exists = false;
    {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name.eq_ignore_ascii_case(column) {
                exists = true;
                break;
            }
        }
    }
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
    }
    Ok(())
}

/// Strip the library root from an absolute path, returning the portable part.
pub fn relative_to(path: &str, root: &str) -> Option<String> {
    let root = root.trim_end_matches(['\\', '/']);
    if root.is_empty() {
        return None;
    }
    let prefix = format!("{root}\\");
    if path.len() <= prefix.len() {
        return None;
    }
    if path.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes()) {
        Some(path[prefix.len()..].to_string())
    } else {
        None
    }
}

/// Fill in `rel_path` for rows stored under `root` (migration + first scan).
pub fn backfill_rel_paths(conn: &Connection, root: &str) -> Result<usize> {
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, path FROM videos WHERE rel_path IS NULL")?;
        let mapped = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        mapped.collect::<rusqlite::Result<_>>()?
    };
    let mut filled = 0;
    for (id, path) in rows {
        if let Some(rel) = relative_to(&path, root) {
            conn.execute(
                "UPDATE videos SET rel_path = ?2 WHERE id = ?1",
                params![id, rel],
            )?;
            filled += 1;
        }
    }
    Ok(filled)
}

/// Rewrite every absolute path to the new root, keeping ids/tags/ratings.
pub fn repath_library(conn: &mut Connection, new_root: &str) -> Result<usize> {
    let root = new_root.trim_end_matches(['\\', '/']).to_string();
    let rows: Vec<(i64, String)> = {
        let mut stmt =
            conn.prepare("SELECT id, rel_path FROM videos WHERE rel_path IS NOT NULL")?;
        let mapped = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        mapped.collect::<rusqlite::Result<_>>()?
    };
    let tx = conn.transaction()?;
    let mut moved = 0;
    for (id, rel) in rows {
        let new_path = format!("{root}\\{rel}");
        if tx.execute(
            "UPDATE OR IGNORE videos SET path = ?1 WHERE id = ?2",
            params![new_path, id],
        )? > 0
        {
            moved += 1;
        }
    }
    tx.commit()?;
    Ok(moved)
}

/// Move trash entries' original locations to the new root too.
pub fn repath_trash(conn: &mut Connection, old_root: &str, new_root: &str) -> Result<usize> {
    let root = new_root.trim_end_matches(['\\', '/']).to_string();
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, original_path FROM trash")?;
        let mapped = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        mapped.collect::<rusqlite::Result<_>>()?
    };
    let tx = conn.transaction()?;
    let mut moved = 0;
    for (id, path) in rows {
        if let Some(rel) = relative_to(&path, old_root) {
            tx.execute(
                "UPDATE trash SET original_path = ?1 WHERE id = ?2",
                params![format!("{root}\\{rel}"), id],
            )?;
            moved += 1;
        }
    }
    tx.commit()?;
    Ok(moved)
}

/// Remove rows flagged as missing (files deleted or from an old root).
pub fn purge_missing(conn: &Connection) -> Result<usize> {
    let removed = conn.execute("DELETE FROM videos WHERE missing = 1", [])?;
    conn.execute(
        "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM video_tags)",
        [],
    )?;
    conn.execute(
        "DELETE FROM toys WHERE id NOT IN (SELECT toy_id FROM video_toys)",
        [],
    )?;
    Ok(removed)
}

/// Strip the Windows verbatim prefix (`\\?\`) so paths read normally in the UI.
pub fn display_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// Older builds stored canonicalized paths such as `\\?\D:\Videos\...`. Rewrite
/// them in place (root setting and every video row) so they display cleanly and
/// rescans match the existing rows instead of inserting duplicates.
pub fn normalize_stored_paths(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        UPDATE OR IGNORE videos SET path = '\\' || substr(path, 9)
          WHERE path LIKE '\\?\UNC\%';
        UPDATE OR IGNORE videos SET path = substr(path, 5)
          WHERE path LIKE '\\?\%';
        DELETE FROM videos
          WHERE path LIKE '\\?\%'
            AND substr(path, 5) IN (SELECT path FROM videos WHERE path NOT LIKE '\\?\%');
        UPDATE settings SET value = '\\' || substr(value, 9)
          WHERE key = 'root' AND value LIKE '\\?\UNC\%';
        UPDATE settings SET value = substr(value, 5)
          WHERE key = 'root' AND value LIKE '\\?\%';
        "#,
    )?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn attach_tags(conn: &Connection, items: &mut [VideoDto]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = items.iter().map(|v| v.id).collect();
    let placeholders = std::iter::repeat("?")
        .take(ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT vt.video_id, t.name FROM video_tags vt
         JOIN tags t ON t.id = vt.tag_id
         WHERE vt.video_id IN ({})
         ORDER BY t.name COLLATE NOCASE",
        placeholders
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut map: HashMap<i64, Vec<String>> = HashMap::new();
    let rows = stmt.query_map(params_from_iter(ids.iter()), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (vid, name) = row?;
        map.entry(vid).or_default().push(name);
    }
    for v in items.iter_mut() {
        v.tags = map.remove(&v.id).unwrap_or_default();
    }

    let toy_sql = format!(
        "SELECT vt.video_id, t.name FROM video_toys vt
         JOIN toys t ON t.id = vt.toy_id
         WHERE vt.video_id IN ({})
         ORDER BY t.name COLLATE NOCASE",
        placeholders
    );
    let mut stmt = conn.prepare(&toy_sql)?;
    let mut toy_map: HashMap<i64, Vec<String>> = HashMap::new();
    let rows = stmt.query_map(params_from_iter(ids.iter()), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (vid, name) = row?;
        toy_map.entry(vid).or_default().push(name);
    }
    for v in items.iter_mut() {
        v.toys = toy_map.remove(&v.id).unwrap_or_default();
    }
    Ok(())
}

pub fn get_video(conn: &Connection, id: i64) -> Result<Option<VideoDto>> {
    let sql = format!("SELECT {} FROM videos v WHERE v.id = ?1", VIDEO_COLS);
    let mut v = conn
        .query_row(&sql, params![id], video_from_row)
        .optional()?;
    if let Some(v) = v.as_mut() {
        attach_tags(conn, std::slice::from_mut(v))?;
    }
    Ok(v)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoFilter {
    pub search: Option<String>,
    pub tags: Vec<String>,
    pub tag_mode: String,
    pub toys: Vec<String>,
    pub nsfw: String,
    pub reviewed: String,
    pub min_rating: i64,
    pub creator: Option<String>,
    pub folder: Option<String>,
    pub sort: String,
    pub limit: i64,
    pub offset: i64,
}

fn build_where(f: &VideoFilter) -> (String, Vec<SqlValue>) {
    let mut clauses: Vec<String> = vec!["v.missing = 0".into()];
    let mut vals: Vec<SqlValue> = Vec::new();

    if let Some(s) = f.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let like = format!("%{}%", s);
        clauses.push(
            "(v.file_name LIKE ? COLLATE NOCASE OR v.path LIKE ? COLLATE NOCASE \
              OR COALESCE(v.creator,'') LIKE ? COLLATE NOCASE \
              OR COALESCE(v.notes,'') LIKE ? COLLATE NOCASE \
              OR EXISTS (SELECT 1 FROM video_toys vt JOIN toys t ON t.id = vt.toy_id
                         WHERE vt.video_id = v.id AND t.name LIKE ? COLLATE NOCASE))"
                .into(),
        );
        for _ in 0..5 {
            vals.push(SqlValue::Text(like.clone()));
        }
    }

    if !f.tags.is_empty() {
        if f.tag_mode == "all" {
            for t in &f.tags {
                clauses.push(
                    "EXISTS (SELECT 1 FROM video_tags vt JOIN tags t ON t.id = vt.tag_id
                             WHERE vt.video_id = v.id AND t.name = ? COLLATE NOCASE)"
                        .into(),
                );
                vals.push(SqlValue::Text(t.clone()));
            }
        } else {
            let placeholders = std::iter::repeat("?")
                .take(f.tags.len())
                .collect::<Vec<_>>()
                .join(",");
            clauses.push(format!(
                "EXISTS (SELECT 1 FROM video_tags vt JOIN tags t ON t.id = vt.tag_id
                         WHERE vt.video_id = v.id AND t.name COLLATE NOCASE IN ({}))",
                placeholders
            ));
            for t in &f.tags {
                vals.push(SqlValue::Text(t.clone()));
            }
        }
    }

    if !f.toys.is_empty() {
        let placeholders = std::iter::repeat("?")
            .take(f.toys.len())
            .collect::<Vec<_>>()
            .join(",");
        clauses.push(format!(
            "EXISTS (SELECT 1 FROM video_toys vt JOIN toys t ON t.id = vt.toy_id
                     WHERE vt.video_id = v.id AND t.name COLLATE NOCASE IN ({}))",
            placeholders
        ));
        for t in &f.toys {
            vals.push(SqlValue::Text(t.clone()));
        }
    }

    if f.nsfw == "yes" {
        clauses.push("v.nsfw = 1".into());
    } else if f.nsfw == "no" {
        clauses.push("v.nsfw = 0".into());
    }
    if f.reviewed == "yes" {
        clauses.push("v.reviewed = 1".into());
    } else if f.reviewed == "no" {
        clauses.push("v.reviewed = 0".into());
    }
    if f.min_rating > 0 {
        clauses.push("v.rating >= ?".into());
        vals.push(SqlValue::Integer(f.min_rating));
    }
    if let Some(c) = f.creator.as_deref().filter(|c| !c.is_empty()) {
        clauses.push("v.creator = ?".into());
        vals.push(SqlValue::Text(c.to_string()));
    }
    if let Some(dir) = f.folder.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        let prefix = format!("{}\\", dir.trim_end_matches('\\'));
        clauses.push("substr(v.path, 1, length(?)) = ?".into());
        vals.push(SqlValue::Text(prefix.clone()));
        vals.push(SqlValue::Text(prefix));
    }

    (clauses.join(" AND "), vals)
}

pub fn list_videos(conn: &Connection, f: &VideoFilter) -> Result<(i64, Vec<VideoDto>)> {
    let (where_sql, vals) = build_where(f);
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM videos v WHERE {}", where_sql),
        params_from_iter(vals.iter()),
        |r| r.get(0),
    )?;

    let order = match f.sort.as_str() {
        "name_asc" => "v.file_name COLLATE NOCASE ASC",
        "name_desc" => "v.file_name COLLATE NOCASE DESC",
        "rating_desc" => "v.rating DESC, v.added_at DESC",
        "rating_asc" => "v.rating ASC, v.added_at DESC",
        "size_desc" => "v.size DESC",
        "duration_desc" => "v.duration_ms DESC",
        "views_desc" => "v.view_count DESC, v.last_viewed DESC",
        "random" => "RANDOM()",
        _ => "v.added_at DESC, v.id DESC",
    };
    let limit = if f.limit > 0 { f.limit.min(500) } else { 100 };
    let sql = format!(
        "SELECT {} FROM videos v WHERE {} ORDER BY {} LIMIT ? OFFSET ?",
        VIDEO_COLS, where_sql, order
    );
    let mut vals2 = vals;
    vals2.push(SqlValue::Integer(limit));
    vals2.push(SqlValue::Integer(f.offset.max(0)));

    let mut stmt = conn.prepare(&sql)?;
    let mut items: Vec<VideoDto> = stmt
        .query_map(params_from_iter(vals2.iter()), video_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    attach_tags(conn, &mut items)?;
    Ok((total, items))
}

pub fn rating_queue(conn: &Connection, limit: i64, folder: Option<&str>) -> Result<Vec<VideoDto>> {
    let limit = limit.clamp(1, 2000);
    let mut where_sql = String::from("v.missing = 0 AND v.reviewed = 0");
    let mut vals: Vec<SqlValue> = Vec::new();
    if let Some(dir) = folder.map(str::trim).filter(|d| !d.is_empty()) {
        let prefix = format!("{}\\", dir.trim_end_matches('\\'));
        where_sql.push_str(" AND substr(v.path, 1, length(?1)) = ?1");
        vals.push(SqlValue::Text(prefix));
    }
    let sql = format!(
        "SELECT {} FROM videos v WHERE {} ORDER BY v.added_at ASC, v.id ASC LIMIT ?",
        VIDEO_COLS, where_sql
    );
    vals.push(SqlValue::Integer(limit));
    let mut stmt = conn.prepare(&sql)?;
    let mut items: Vec<VideoDto> = stmt
        .query_map(params_from_iter(vals.iter()), video_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    attach_tags(conn, &mut items)?;
    Ok(items)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub md5: String,
    pub wasted: i64,
    pub videos: Vec<VideoDto>,
}

pub fn duplicates(conn: &Connection) -> Result<Vec<DuplicateGroup>> {
    let hashes: Vec<(String, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT md5, COUNT(*) FROM videos
             WHERE missing = 0 AND md5 IS NOT NULL AND md5 <> ''
             GROUP BY md5 HAVING COUNT(*) > 1
             ORDER BY COUNT(*) DESC, SUM(size) DESC",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    let mut groups = Vec::new();
    for (md5, _count) in hashes {
        let sql = format!(
            "SELECT {} FROM videos v WHERE v.md5 = ?1 AND v.missing = 0
             ORDER BY v.added_at ASC, v.id ASC",
            VIDEO_COLS
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut videos: Vec<VideoDto> = stmt
            .query_map(params![md5], video_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        attach_tags(conn, &mut videos)?;
        let wasted = videos.iter().skip(1).map(|v| v.size).sum();
        groups.push(DuplicateGroup {
            md5,
            wasted,
            videos,
        });
    }
    Ok(groups)
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total: i64,
    pub reviewed: i64,
    pub unreviewed: i64,
    pub nsfw: i64,
    pub tagged: i64,
    pub duplicates: i64,
    pub missing: i64,
    pub total_size: i64,
    pub total_duration_ms: i64,
    pub hash_pending: i64,
    pub hash_skipped: i64,
    pub dna_done: i64,
    pub dna_pending: i64,
    pub trash: i64,
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    let s = conn.query_row(
        "SELECT
           COUNT(*),
           COALESCE(SUM(reviewed),0),
           COALESCE(SUM(CASE WHEN reviewed = 0 THEN 1 ELSE 0 END),0),
           COALESCE(SUM(nsfw),0),
           COALESCE(SUM(CASE WHEN id IN (SELECT video_id FROM video_tags) THEN 1 ELSE 0 END),0),
           COALESCE(SUM(CASE WHEN (md5 IS NULL OR md5 = '') AND hash_state = 0 THEN 1 ELSE 0 END),0),
           COALESCE(SUM(size),0),
           COALESCE(SUM(CASE WHEN duration_ms > 0 THEN duration_ms ELSE 0 END),0),
           COALESCE(SUM(CASE WHEN (md5 IS NULL OR md5 = '') AND hash_state = 2 THEN 1 ELSE 0 END),0)
         FROM videos WHERE missing = 0",
        [],
        |r| {
            Ok(Stats {
                total: r.get(0)?,
                reviewed: r.get(1)?,
                unreviewed: r.get(2)?,
                nsfw: r.get(3)?,
                tagged: r.get(4)?,
                hash_pending: r.get(5)?,
                total_size: r.get(6)?,
                total_duration_ms: r.get(7)?,
                hash_skipped: r.get(8)?,
                ..Default::default()
            })
        },
    )?;
    let mut s = s;
    s.duplicates = conn.query_row(
        "SELECT COUNT(*) FROM (
           SELECT md5 FROM videos WHERE missing = 0 AND md5 IS NOT NULL AND md5 <> ''
           GROUP BY md5 HAVING COUNT(*) > 1)",
        [],
        |r| r.get(0),
    )?;
    s.missing = conn.query_row("SELECT COUNT(*) FROM videos WHERE missing = 1", [], |r| {
        r.get(0)
    })?;
    s.trash = conn.query_row("SELECT COUNT(*) FROM trash", [], |r| r.get(0))?;
    s.dna_done = conn.query_row("SELECT COUNT(*) FROM video_dna", [], |r| r.get(0))?;
    s.dna_pending = conn.query_row(
        "SELECT COUNT(*) FROM videos
         WHERE missing = 0 AND id NOT IN (SELECT video_id FROM video_dna)",
        [],
        |r| r.get(0),
    )?;
    Ok(s)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub name: String,
    pub count: i64,
}

pub fn all_videos(conn: &Connection) -> Result<Vec<VideoDto>> {
    let sql = format!(
        "SELECT {} FROM videos v WHERE v.missing = 0 ORDER BY v.id",
        VIDEO_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut items: Vec<VideoDto> = stmt
        .query_map([], video_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    attach_tags(conn, &mut items)?;
    Ok(items)
}

pub fn all_tags(conn: &Connection) -> Result<Vec<TagCount>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, COUNT(v.id)
         FROM tags t
         LEFT JOIN video_tags vt ON vt.tag_id = t.id
         LEFT JOIN videos v ON v.id = vt.video_id AND v.missing = 0
         GROUP BY t.id
         ORDER BY t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TagCount {
            name: r.get(0)?,
            count: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub toys: Vec<TagCount>,
    pub creators: Vec<TagCount>,
    pub ratings: Vec<i64>,
    pub no_toys: i64,
    pub no_tags: i64,
    pub never_viewed: i64,
    pub added_week: i64,
    pub folders: Vec<FolderCount>,
}

pub fn overview(conn: &Connection, now: i64) -> Result<Overview> {
    let toys = all_toys(conn)?;

    let creators: Vec<TagCount> = {
        let mut stmt = conn.prepare(
            "SELECT creator, COUNT(*) FROM videos
             WHERE missing = 0 AND creator IS NOT NULL AND TRIM(creator) <> ''
             GROUP BY creator ORDER BY COUNT(*) DESC, creator COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TagCount {
                name: r.get(0)?,
                count: r.get(1)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let mut ratings = vec![0i64; 6];
    {
        let mut stmt =
            conn.prepare("SELECT rating, COUNT(*) FROM videos WHERE missing = 0 GROUP BY rating")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (rating, count) = row?;
            if (0..=5).contains(&rating) {
                ratings[rating as usize] = count;
            }
        }
    }

    let no_toys: i64 = conn.query_row(
        "SELECT COUNT(*) FROM videos
         WHERE missing = 0 AND id NOT IN (SELECT video_id FROM video_toys)",
        [],
        |r| r.get(0),
    )?;
    let no_tags: i64 = conn.query_row(
        "SELECT COUNT(*) FROM videos
         WHERE missing = 0 AND id NOT IN (SELECT video_id FROM video_tags)",
        [],
        |r| r.get(0),
    )?;
    let never_viewed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM videos WHERE missing = 0 AND view_count = 0",
        [],
        |r| r.get(0),
    )?;
    let added_week: i64 = conn.query_row(
        "SELECT COUNT(*) FROM videos WHERE missing = 0 AND added_at >= ?1",
        params![now - 7 * 24 * 3600],
        |r| r.get(0),
    )?;

    Ok(Overview {
        toys,
        creators,
        ratings,
        no_toys,
        no_tags,
        never_viewed,
        added_week,
        folders: folders(conn)?,
    })
}

pub fn all_toys(conn: &Connection) -> Result<Vec<TagCount>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, COUNT(v.id)
         FROM toys t
         LEFT JOIN video_toys vt ON vt.toy_id = t.id
         LEFT JOIN videos v ON v.id = vt.video_id AND v.missing = 0
         GROUP BY t.id
         ORDER BY t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TagCount {
            name: r.get(0)?,
            count: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Rename a toy; if the new name exists this merges the two.
pub fn rename_toy(conn: &mut Connection, old: &str, new: &str) -> Result<()> {
    let new = new.trim();
    if new.is_empty() {
        return Err(anyhow!("Toy name cannot be empty."));
    }
    let old_id: i64 = conn.query_row(
        "SELECT id FROM toys WHERE name = ?1 COLLATE NOCASE",
        params![old],
        |r| r.get(0),
    )?;
    let tx = conn.transaction()?;
    tx.execute("INSERT OR IGNORE INTO toys(name) VALUES (?1)", params![new])?;
    let new_id: i64 = tx.query_row(
        "SELECT id FROM toys WHERE name = ?1 COLLATE NOCASE",
        params![new],
        |r| r.get(0),
    )?;
    if new_id != old_id {
        tx.execute(
            "UPDATE OR IGNORE video_toys SET toy_id = ?1 WHERE toy_id = ?2",
            params![new_id, old_id],
        )?;
        tx.execute("DELETE FROM video_toys WHERE toy_id = ?1", params![old_id])?;
        tx.execute("DELETE FROM toys WHERE id = ?1", params![old_id])?;
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_toy(conn: &mut Connection, name: &str) -> Result<()> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM toys WHERE name = ?1 COLLATE NOCASE",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM video_toys WHERE toy_id = ?1", params![id])?;
        tx.execute("DELETE FROM toys WHERE id = ?1", params![id])?;
        tx.commit()?;
    }
    Ok(())
}

pub fn creators(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT creator FROM videos
         WHERE missing = 0 AND creator IS NOT NULL AND TRIM(creator) <> ''
         ORDER BY creator COLLATE NOCASE",
    )?;
    let out = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(out)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoPatch {
    pub creator: Option<String>,
    pub nsfw: Option<bool>,
    pub popping: Option<bool>,
    pub destruction: Option<bool>,
    pub deflation: Option<bool>,
    pub inflation: Option<bool>,
    pub notes: Option<String>,
    pub rating: Option<i64>,
    pub reviewed: Option<bool>,
    pub tags: Option<Vec<String>>,
    pub toys: Option<Vec<String>>,
}

impl Default for VideoPatch {
    fn default() -> Self {
        Self {
            creator: None,
            nsfw: None,
            popping: None,
            destruction: None,
            deflation: None,
            inflation: None,
            notes: None,
            rating: None,
            reviewed: None,
            tags: None,
            toys: None,
        }
    }
}

pub fn save_video(conn: &mut Connection, id: i64, p: &VideoPatch) -> Result<VideoDto> {
    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<SqlValue> = Vec::new();

    macro_rules! text_field {
        ($v:expr, $col:expr) => {
            if let Some(value) = &$v {
                let trimmed = value.trim();
                sets.push($col);
                vals.push(if trimmed.is_empty() {
                    SqlValue::Null
                } else {
                    SqlValue::Text(trimmed.to_string())
                });
            }
        };
    }
    macro_rules! bool_field {
        ($v:expr, $col:expr) => {
            if let Some(value) = $v {
                sets.push($col);
                vals.push(SqlValue::Integer(if value { 1 } else { 0 }));
            }
        };
    }

    text_field!(p.creator, "creator = ?");
    text_field!(p.notes, "notes = ?");
    bool_field!(p.nsfw, "nsfw = ?");
    bool_field!(p.popping, "popping = ?");
    bool_field!(p.destruction, "destruction = ?");
    bool_field!(p.deflation, "deflation = ?");
    bool_field!(p.inflation, "inflation = ?");
    bool_field!(p.reviewed, "reviewed = ?");
    if let Some(r) = p.rating {
        sets.push("rating = ?");
        vals.push(SqlValue::Integer(r.clamp(0, 5)));
    }

    if !sets.is_empty() {
        vals.push(SqlValue::Integer(id));
        let sql = format!("UPDATE videos SET {} WHERE id = ?", sets.join(", "));
        conn.execute(&sql, params_from_iter(vals.iter()))?;
    }

    if let Some(tags) = &p.tags {
        conn.execute("DELETE FROM video_tags WHERE video_id = ?1", params![id])?;
        let mut seen: HashSet<String> = HashSet::new();
        for raw in tags {
            let name = raw.trim();
            if name.is_empty() || !seen.insert(name.to_lowercase()) {
                continue;
            }
            conn.execute(
                "INSERT OR IGNORE INTO tags(name) VALUES (?1)",
                params![name],
            )?;
            let tag_id: i64 = conn.query_row(
                "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO video_tags(video_id, tag_id) VALUES (?1, ?2)",
                params![id, tag_id],
            )?;
        }
        conn.execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM video_tags)",
            [],
        )?;
    }

    if let Some(toys) = &p.toys {
        conn.execute("DELETE FROM video_toys WHERE video_id = ?1", params![id])?;
        let mut seen: HashSet<String> = HashSet::new();
        for raw in toys {
            let name = raw.trim();
            if name.is_empty() || !seen.insert(name.to_lowercase()) {
                continue;
            }
            conn.execute(
                "INSERT OR IGNORE INTO toys(name) VALUES (?1)",
                params![name],
            )?;
            let toy_id: i64 = conn.query_row(
                "SELECT id FROM toys WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO video_toys(video_id, toy_id) VALUES (?1, ?2)",
                params![id, toy_id],
            )?;
        }
        conn.execute(
            "DELETE FROM toys WHERE id NOT IN (SELECT toy_id FROM video_toys)",
            [],
        )?;
    }

    get_video(conn, id)?.ok_or_else(|| anyhow!("video {id} not found"))
}

#[derive(Debug)]
pub struct Upsert {
    pub id: i64,
    pub is_new: bool,
    pub changed: bool,
}

pub fn upsert_file(
    conn: &Connection,
    path: &str,
    rel_path: Option<&str>,
    file_name: &str,
    ext: &str,
    size: i64,
    mtime: i64,
    now: i64,
) -> Result<Upsert> {
    let existing: Option<(i64, i64, i64)> = conn
        .query_row(
            "SELECT id, size, mtime FROM videos WHERE path = ?1",
            params![path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;

    match existing {
        Some((id, old_size, old_mtime)) => {
            let changed = old_size != size || old_mtime != mtime;
            if changed {
                conn.execute(
                    "UPDATE videos SET size = ?2, mtime = ?3, rel_path = ?4, duration_ms = NULL, width = NULL,
                        height = NULL, vcodec = NULL, acodec = NULL, has_audio = 0, md5 = NULL,
                        hash_state = 0, missing = 0
                     WHERE id = ?1",
                    params![id, size, mtime, rel_path],
                )?;
            } else {
                conn.execute(
                    "UPDATE videos SET missing = 0, rel_path = COALESCE(?2, rel_path)
                     WHERE id = ?1",
                    params![id, rel_path],
                )?;
            }
            Ok(Upsert {
                id,
                is_new: false,
                changed,
            })
        }
        None => {
            conn.execute(
                "INSERT INTO videos(path, rel_path, file_name, ext, size, mtime, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![path, rel_path, file_name, ext, size, mtime, now],
            )?;
            Ok(Upsert {
                id: conn.last_insert_rowid(),
                is_new: true,
                changed: false,
            })
        }
    }
}

pub fn mark_missing(conn: &Connection, seen: &HashSet<String>) -> Result<usize> {
    let rows: Vec<(i64, String, i64)> = {
        let mut stmt = conn.prepare("SELECT id, path, missing FROM videos")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut changed = 0;
    for (id, path, missing) in rows {
        let should_miss = !seen.contains(&path);
        if should_miss && missing == 0 {
            conn.execute("UPDATE videos SET missing = 1 WHERE id = ?1", params![id])?;
            changed += 1;
        }
    }
    Ok(changed)
}

pub fn pending_metadata(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, path FROM videos
         WHERE missing = 0 AND duration_ms IS NULL
         ORDER BY size ASC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn pending_hashes(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, path FROM videos
         WHERE missing = 0 AND (md5 IS NULL OR md5 = '')
         ORDER BY size ASC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn set_metadata(conn: &Connection, id: i64, p: &crate::media::ProbeInfo) -> Result<()> {
    conn.execute(
        "UPDATE videos SET duration_ms = ?2, width = ?3, height = ?4, vcodec = ?5,
            acodec = ?6, has_audio = ?7
         WHERE id = ?1",
        params![
            id,
            p.duration_ms,
            p.width,
            p.height,
            p.vcodec,
            p.acodec,
            if p.has_audio { 1 } else { 0 }
        ],
    )?;
    Ok(())
}

pub fn mark_probe_failed(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE videos SET duration_ms = -1 WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

pub fn set_md5(conn: &Connection, id: i64, md5: &str) -> Result<()> {
    conn.execute(
        "UPDATE videos SET md5 = ?2, hash_state = 1 WHERE id = ?1",
        params![id, md5],
    )?;
    Ok(())
}

pub fn mark_viewed(conn: &Connection, id: i64, now: i64) -> Result<i64> {
    conn.execute(
        "UPDATE videos SET view_count = view_count + 1, last_viewed = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    let count = conn.query_row(
        "SELECT view_count FROM videos WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    Ok(count)
}

pub fn delete_video(conn: &mut Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM video_tags WHERE video_id = ?1", params![id])?;
    conn.execute("DELETE FROM videos WHERE id = ?1", params![id])?;
    conn.execute(
        "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM video_tags)",
        [],
    )?;
    Ok(())
}

/* ------------------------------ hashing --------------------------------- */

pub fn hash_mode(conn: &Connection) -> String {
    get_setting(conn, "hash_mode").unwrap_or_else(|| "duplicates".to_string())
}

pub fn set_hash_mode(conn: &Connection, mode: &str) -> Result<()> {
    set_setting(conn, "hash_mode", mode)?;
    if mode == "all" {
        conn.execute("UPDATE videos SET hash_state = 0 WHERE hash_state = 2", [])?;
    }
    Ok(())
}

/// Files worth hashing. In "duplicates" mode only files whose size is shared
/// with another file can possibly be byte-identical, so unique sizes are
/// marked as skipped instead of read in full.
pub fn hash_candidates(conn: &Connection, mode: &str) -> Result<Vec<(i64, String)>> {
    if mode == "all" {
        return pending_hashes(conn);
    }
    conn.execute(
        "UPDATE videos
            SET hash_state = CASE WHEN size IN (
                    SELECT size FROM videos WHERE missing = 0
                    GROUP BY size HAVING COUNT(*) > 1
                ) THEN 0 ELSE 2 END
          WHERE missing = 0 AND hash_state <> 1 AND (md5 IS NULL OR md5 = '')",
        [],
    )?;
    let mut stmt = conn.prepare(
        "SELECT id, path FROM videos
         WHERE missing = 0 AND hash_state = 0 AND (md5 IS NULL OR md5 = '')
         ORDER BY size ASC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/* ------------------------------ folders --------------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderCount {
    pub path: String,
    pub count: i64,
}

pub fn folders(conn: &Connection) -> Result<Vec<FolderCount>> {
    let mut stmt = conn.prepare("SELECT path FROM videos WHERE missing = 0")?;
    let paths = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut counts: HashMap<String, i64> = HashMap::new();
    for path in paths {
        let path = path?;
        let parent = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if !parent.is_empty() {
            *counts.entry(parent).or_insert(0) += 1;
        }
    }
    let mut out: Vec<FolderCount> = counts
        .into_iter()
        .map(|(path, count)| FolderCount { path, count })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/* -------------------------------- tags ---------------------------------- */

/// Rename a tag; if the new name already exists this merges the two.
pub fn rename_tag(conn: &mut Connection, old: &str, new: &str) -> Result<()> {
    let new = new.trim();
    if new.is_empty() {
        return Err(anyhow!("Tag name cannot be empty."));
    }
    let old_id: i64 = conn.query_row(
        "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
        params![old],
        |r| r.get(0),
    )?;
    let tx = conn.transaction()?;
    tx.execute("INSERT OR IGNORE INTO tags(name) VALUES (?1)", params![new])?;
    let new_id: i64 = tx.query_row(
        "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
        params![new],
        |r| r.get(0),
    )?;
    if new_id != old_id {
        tx.execute(
            "UPDATE OR IGNORE video_tags SET tag_id = ?1 WHERE tag_id = ?2",
            params![new_id, old_id],
        )?;
        tx.execute("DELETE FROM video_tags WHERE tag_id = ?1", params![old_id])?;
        tx.execute("DELETE FROM tags WHERE id = ?1", params![old_id])?;
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_tag(conn: &mut Connection, name: &str) -> Result<()> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM video_tags WHERE tag_id = ?1", params![id])?;
        tx.execute("DELETE FROM tags WHERE id = ?1", params![id])?;
        tx.commit()?;
    }
    Ok(())
}

/* ----------------------------- bulk editing ------------------------------ */

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BulkEdit {
    pub ids: Vec<i64>,
    pub creator: Option<String>,
    pub rating: Option<i64>,
    pub reviewed: Option<bool>,
    pub nsfw: Option<bool>,
    pub popping: Option<bool>,
    pub destruction: Option<bool>,
    pub deflation: Option<bool>,
    pub inflation: Option<bool>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub add_toys: Vec<String>,
    pub remove_toys: Vec<String>,
}

pub fn bulk_update(conn: &mut Connection, edit: &BulkEdit) -> Result<usize> {
    if edit.ids.is_empty() {
        return Ok(0);
    }
    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<SqlValue> = Vec::new();

    if let Some(creator) = &edit.creator {
        let trimmed = creator.trim();
        sets.push("creator = ?");
        vals.push(if trimmed.is_empty() {
            SqlValue::Null
        } else {
            SqlValue::Text(trimmed.to_string())
        });
    }
    macro_rules! bool_field {
        ($value:expr, $col:expr) => {
            if let Some(value) = $value {
                sets.push($col);
                vals.push(SqlValue::Integer(if value { 1 } else { 0 }));
            }
        };
    }
    bool_field!(edit.nsfw, "nsfw = ?");
    bool_field!(edit.popping, "popping = ?");
    bool_field!(edit.destruction, "destruction = ?");
    bool_field!(edit.deflation, "deflation = ?");
    bool_field!(edit.inflation, "inflation = ?");
    bool_field!(edit.reviewed, "reviewed = ?");
    if let Some(rating) = edit.rating {
        sets.push("rating = ?");
        vals.push(SqlValue::Integer(rating.clamp(0, 5)));
    }

    let tx = conn.transaction()?;
    if !sets.is_empty() {
        let sql = format!("UPDATE videos SET {} WHERE id = ?", sets.join(", "));
        let mut stmt = tx.prepare(&sql)?;
        for id in &edit.ids {
            let mut row = vals.clone();
            row.push(SqlValue::Integer(*id));
            stmt.execute(params_from_iter(row.iter()))?;
        }
    }

    for raw in edit.add_tags.iter().chain(edit.remove_tags.iter()) {
        let name = raw.trim();
        if !name.is_empty() {
            tx.execute(
                "INSERT OR IGNORE INTO tags(name) VALUES (?1)",
                params![name],
            )?;
        }
    }
    for raw in &edit.add_tags {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        let tag_id: i64 = tx.query_row(
            "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
            params![name],
            |r| r.get(0),
        )?;
        for id in &edit.ids {
            tx.execute(
                "INSERT OR IGNORE INTO video_tags(video_id, tag_id) VALUES (?1, ?2)",
                params![id, tag_id],
            )?;
        }
    }
    for raw in &edit.remove_tags {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        let tag_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(tag_id) = tag_id {
            for id in &edit.ids {
                tx.execute(
                    "DELETE FROM video_tags WHERE video_id = ?1 AND tag_id = ?2",
                    params![id, tag_id],
                )?;
            }
        }
    }
    tx.execute(
        "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM video_tags)",
        [],
    )?;

    for raw in edit.add_toys.iter().chain(edit.remove_toys.iter()) {
        let name = raw.trim();
        if !name.is_empty() {
            tx.execute(
                "INSERT OR IGNORE INTO toys(name) VALUES (?1)",
                params![name],
            )?;
        }
    }
    for raw in &edit.add_toys {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        let toy_id: i64 = tx.query_row(
            "SELECT id FROM toys WHERE name = ?1 COLLATE NOCASE",
            params![name],
            |r| r.get(0),
        )?;
        for id in &edit.ids {
            tx.execute(
                "INSERT OR IGNORE INTO video_toys(video_id, toy_id) VALUES (?1, ?2)",
                params![id, toy_id],
            )?;
        }
    }
    for raw in &edit.remove_toys {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        let toy_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM toys WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(toy_id) = toy_id {
            for id in &edit.ids {
                tx.execute(
                    "DELETE FROM video_toys WHERE video_id = ?1 AND toy_id = ?2",
                    params![id, toy_id],
                )?;
            }
        }
    }
    tx.execute(
        "DELETE FROM toys WHERE id NOT IN (SELECT toy_id FROM video_toys)",
        [],
    )?;
    tx.commit()?;
    Ok(edit.ids.len())
}

/* -------------------------------- trash --------------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    pub id: i64,
    pub original_path: String,
    pub trashed_path: String,
    pub file_name: String,
    pub size: i64,
    pub md5: Option<String>,
    pub meta_json: Option<String>,
    pub trashed_at: i64,
}

pub fn trash_add(
    conn: &Connection,
    original_path: &str,
    trashed_path: &str,
    file_name: &str,
    size: i64,
    md5: Option<&str>,
    meta_json: Option<&str>,
    now: i64,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO trash(original_path, trashed_path, file_name, size, md5, meta_json, trashed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            original_path,
            trashed_path,
            file_name,
            size,
            md5,
            meta_json,
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn trash_from_row(r: &Row<'_>) -> rusqlite::Result<TrashEntry> {
    Ok(TrashEntry {
        id: r.get(0)?,
        original_path: r.get(1)?,
        trashed_path: r.get(2)?,
        file_name: r.get(3)?,
        size: r.get(4)?,
        md5: r.get(5)?,
        meta_json: r.get(6)?,
        trashed_at: r.get(7)?,
    })
}

pub fn trash_list(conn: &Connection) -> Result<Vec<TrashEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, original_path, trashed_path, file_name, size, md5, meta_json, trashed_at
         FROM trash ORDER BY trashed_at DESC, id DESC",
    )?;
    let rows = stmt.query_map([], trash_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn trash_get(conn: &Connection, id: i64) -> Result<Option<TrashEntry>> {
    Ok(conn
        .query_row(
            "SELECT id, original_path, trashed_path, file_name, size, md5, meta_json, trashed_at
             FROM trash WHERE id = ?1",
            params![id],
            trash_from_row,
        )
        .optional()?)
}

pub fn trash_delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM trash WHERE id = ?1", params![id])?;
    Ok(())
}

/* ------------------------------- video DNA ------------------------------ */

pub fn dna_pending(
    conn: &Connection,
) -> Result<Vec<(i64, String, Option<i64>, Option<i64>, Option<i64>)>> {
    let mut stmt = conn.prepare(
        "SELECT v.id, v.path, v.duration_ms, v.width, v.height FROM videos v
         WHERE v.missing = 0 AND v.id NOT IN (SELECT video_id FROM video_dna)
         ORDER BY v.size ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn set_dna(
    conn: &Connection,
    id: i64,
    frames: &str,
    duration_ms: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
    now: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO video_dna(video_id, frames, duration_ms, width, height, computed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(video_id) DO UPDATE SET
             frames = excluded.frames,
             duration_ms = excluded.duration_ms,
             width = excluded.width,
             height = excluded.height,
             computed_at = excluded.computed_at",
        params![id, frames, duration_ms, width, height, now],
    )?;
    Ok(())
}

pub fn clear_dna(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM video_dna WHERE video_id = ?1", params![id])?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct SimilarGroup {
    pub score: f64,
    pub videos: Vec<VideoDto>,
}

fn parse_dna(frames: &str) -> Vec<u64> {
    frames
        .split(',')
        .filter_map(|part| u64::from_str_radix(part.trim(), 16).ok())
        .collect()
}

fn frame_similarity(a: &[u64], b: &[u64]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let matches = (0..n)
        .filter(|i| (a[*i] ^ b[*i]).count_ones() <= 12)
        .count();
    matches as f64 / n as f64
}

fn uf_find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn uf_union(parent: &mut [usize], a: usize, b: usize) {
    let ra = uf_find(parent, a);
    let rb = uf_find(parent, b);
    if ra != rb {
        parent[rb] = ra;
    }
}

struct DnaRow {
    id: i64,
    frames: Vec<u64>,
    duration_ms: i64,
    aspect: f64,
}

/// Groups of videos whose sampled frame hashes match, even when the files are
/// re-encodes, different resolutions or slightly different bitrates.
pub fn similar_groups(conn: &Connection) -> Result<Vec<SimilarGroup>> {
    let rows: Vec<(i64, String, Option<i64>, Option<i64>, Option<i64>)> = {
        let mut stmt = conn.prepare(
            "SELECT d.video_id, d.frames, d.duration_ms, d.width, d.height
             FROM video_dna d JOIN videos v ON v.id = d.video_id
             WHERE v.missing = 0",
        )?;
        let mapped = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        mapped.collect::<rusqlite::Result<_>>()?
    };

    let dna: Vec<DnaRow> = rows
        .into_iter()
        .filter_map(|(id, frames, duration_ms, width, height)| {
            let frames = parse_dna(&frames);
            if frames.len() < 4 {
                return None;
            }
            let aspect = match (width, height) {
                (Some(w), Some(h)) if h > 0 => w as f64 / h as f64,
                _ => 0.0,
            };
            Some(DnaRow {
                id,
                frames,
                duration_ms: duration_ms.unwrap_or(0),
                aspect,
            })
        })
        .collect();

    let n = dna.len();
    let mut parent: Vec<usize> = (0..n).collect();
    let mut edge_scores: HashMap<(usize, usize), f64> = HashMap::new();

    for i in 0..n {
        for j in (i + 1)..n {
            let (a, b) = (&dna[i], &dna[j]);
            if a.duration_ms > 0 && b.duration_ms > 0 {
                let tolerance = (a.duration_ms as f64 * 0.08).max(3000.0);
                if (a.duration_ms - b.duration_ms).abs() as f64 > tolerance {
                    continue;
                }
            }
            if a.aspect > 0.0 && b.aspect > 0.0 && (a.aspect / b.aspect - 1.0).abs() > 0.08 {
                continue;
            }
            let score = frame_similarity(&a.frames, &b.frames);
            if score >= 0.78 {
                uf_union(&mut parent, i, j);
                edge_scores.insert((i, j), score);
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = uf_find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }

    let mut out = Vec::new();
    for (_, members) in groups {
        if members.len() < 2 {
            continue;
        }
        let mut score: f64 = 0.0;
        for (pos, a) in members.iter().enumerate() {
            for b in members.iter().skip(pos + 1) {
                let key = if a < b { (*a, *b) } else { (*b, *a) };
                if let Some(value) = edge_scores.get(&key) {
                    score = score.max(*value);
                }
            }
        }
        let mut videos = Vec::new();
        for index in &members {
            if let Some(video) = get_video(conn, dna[*index].id)? {
                videos.push(video);
            }
        }
        if videos.len() >= 2 {
            // Best copy first: highest resolution, then largest file (bitrate),
            // then longest duration. This is also the default keeper in the UI.
            videos.sort_by(|a, b| {
                let area = |v: &VideoDto| v.width.unwrap_or(0) * v.height.unwrap_or(0);
                area(b)
                    .cmp(&area(a))
                    .then_with(|| b.size.cmp(&a.size))
                    .then_with(|| b.duration_ms.unwrap_or(0).cmp(&a.duration_ms.unwrap_or(0)))
                    .then_with(|| a.added_at.cmp(&b.added_at))
            });
            out.push(SimilarGroup { score, videos });
        }
    }
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    #[test]
    fn relative_to_strips_root() {
        assert_eq!(
            relative_to(r"D:\Lib\a\b.mp4", r"D:\Lib"),
            Some(r"a\b.mp4".to_string())
        );
        assert_eq!(relative_to(r"D:\Library\a.mp4", r"D:\Lib"), None);
        assert_eq!(
            relative_to(r"D:\Lib\a.mp4", r"D:\Lib\"),
            Some("a.mp4".to_string())
        );
        assert_eq!(relative_to(r"D:\Lib\a.mp4", ""), None);
    }

    #[test]
    fn repath_keeps_tags() {
        let mut conn = test_conn();
        conn.execute(
            "INSERT INTO videos(path, file_name, added_at) VALUES (?1, ?2, ?3)",
            params![r"D:\Old\a.mp4", "a.mp4", 1],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        conn.execute("INSERT INTO tags(name) VALUES ('x')", [])
            .unwrap();
        let tag_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO video_tags(video_id, tag_id) VALUES (?1, ?2)",
            params![id, tag_id],
        )
        .unwrap();

        assert_eq!(backfill_rel_paths(&conn, r"D:\Old").unwrap(), 1);
        assert_eq!(repath_library(&mut conn, r"E:\New").unwrap(), 1);

        let path: String = conn
            .query_row("SELECT path FROM videos WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(path, r"E:\New\a.mp4");
        let tags: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM video_tags WHERE video_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tags, 1);
    }

    #[test]
    fn purge_removes_only_missing() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO videos(path, file_name, missing) VALUES ('a', 'a', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO videos(path, file_name, missing) VALUES ('b', 'b', 1)",
            [],
        )
        .unwrap();
        assert_eq!(purge_missing(&conn).unwrap(), 1);
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1);
    }
}
