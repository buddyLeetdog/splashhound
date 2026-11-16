use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Result};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension, Row};
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
  missing     INTEGER NOT NULL DEFAULT 0
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
    Ok(conn)
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| {
        r.get(0)
    })
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
    let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
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
    Ok(())
}

pub fn get_video(conn: &Connection, id: i64) -> Result<Option<VideoDto>> {
    let sql = format!("SELECT {} FROM videos v WHERE v.id = ?1", VIDEO_COLS);
    let mut v = conn.query_row(&sql, params![id], video_from_row).optional()?;
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
    pub nsfw: String,
    pub reviewed: String,
    pub min_rating: i64,
    pub creator: Option<String>,
    pub sort: String,
    pub limit: i64,
    pub offset: i64,
}

fn build_where(f: &VideoFilter) -> (String, Vec<SqlValue>) {
    let mut clauses: Vec<String> = vec!["v.missing = 0".into()];
    let mut vals: Vec<SqlValue> = Vec::new();

    if let Some(s) = f
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let like = format!("%{}%", s);
        clauses.push(
            "(v.file_name LIKE ? COLLATE NOCASE OR v.path LIKE ? COLLATE NOCASE \
              OR COALESCE(v.creator,'') LIKE ? COLLATE NOCASE \
              OR COALESCE(v.notes,'') LIKE ? COLLATE NOCASE)"
                .into(),
        );
        for _ in 0..4 {
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
            let placeholders = std::iter::repeat("?").take(f.tags.len()).collect::<Vec<_>>().join(",");
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

pub fn rating_queue(conn: &Connection, limit: i64) -> Result<Vec<VideoDto>> {
    let limit = limit.clamp(1, 2000);
    let sql = format!(
        "SELECT {} FROM videos v WHERE v.missing = 0 AND v.reviewed = 0
         ORDER BY v.added_at ASC, v.id ASC LIMIT ?1",
        VIDEO_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut items: Vec<VideoDto> = stmt
        .query_map(params![limit], video_from_row)?
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
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    let s = conn.query_row(
        "SELECT
           COUNT(*),
           COALESCE(SUM(reviewed),0),
           COALESCE(SUM(CASE WHEN reviewed = 0 THEN 1 ELSE 0 END),0),
           COALESCE(SUM(nsfw),0),
           COALESCE(SUM(CASE WHEN id IN (SELECT video_id FROM video_tags) THEN 1 ELSE 0 END),0),
           COALESCE(SUM(CASE WHEN md5 IS NULL OR md5 = '' THEN 1 ELSE 0 END),0),
           COALESCE(SUM(size),0),
           COALESCE(SUM(CASE WHEN duration_ms > 0 THEN duration_ms ELSE 0 END),0)
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
    s.missing = conn.query_row("SELECT COUNT(*) FROM videos WHERE missing = 1", [], |r| r.get(0))?;
    Ok(s)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub name: String,
    pub count: i64,
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
            conn.execute("INSERT OR IGNORE INTO tags(name) VALUES (?1)", params![name])?;
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
                    "UPDATE videos SET size = ?2, mtime = ?3, duration_ms = NULL, width = NULL,
                        height = NULL, vcodec = NULL, acodec = NULL, has_audio = 0, md5 = NULL,
                        missing = 0
                     WHERE id = ?1",
                    params![id, size, mtime],
                )?;
            } else {
                conn.execute(
                    "UPDATE videos SET missing = 0 WHERE id = ?1 AND missing <> 0",
                    params![id],
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
                "INSERT INTO videos(path, file_name, ext, size, mtime, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![path, file_name, ext, size, mtime, now],
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
    conn.execute("UPDATE videos SET duration_ms = -1 WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn set_md5(conn: &Connection, id: i64, md5: &str) -> Result<()> {
    conn.execute("UPDATE videos SET md5 = ?2 WHERE id = ?1", params![id, md5])?;
    Ok(())
}

pub fn mark_viewed(conn: &Connection, id: i64, now: i64) -> Result<i64> {
    conn.execute(
        "UPDATE videos SET view_count = view_count + 1, last_viewed = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    let count = conn.query_row("SELECT view_count FROM videos WHERE id = ?1", params![id], |r| r.get(0))?;
    Ok(count)
}

pub fn delete_video(conn: &mut Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM video_tags WHERE video_id = ?1", params![id])?;
    conn.execute("DELETE FROM videos WHERE id = ?1", params![id])?;
    conn.execute("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM video_tags)", [])?;
    Ok(())
}
