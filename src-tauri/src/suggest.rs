//! Rule-based label suggestions.
//!
//! There is deliberately no machine learning here: with a few dozen labelled
//! videos a learned model scored far worse than plain keyword rules. Instead the
//! file name and folder path are matched against the toys/tags/creators the
//! library already uses, using a small synonym table for the names people
//! actually type ("bball" for Beach Ball, "killer whale" for Orca, ...).
//!
//! Suggestions are only proposals; nothing is saved until the user accepts one.

use std::collections::HashSet;

use serde::Serialize;

use crate::db;

/// One proposed label.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// "creator", "tag" or "toy".
    pub kind: String,
    pub name: String,
    /// 0..1; used for ordering and shown as a subtle hint.
    pub confidence: f64,
    /// Why it was suggested, e.g. "found \u{201c}orca\u{201d} in the file name".
    pub reason: String,
}

/// Alternatives that mean the same toy. Keys are lowercase canonical forms of
/// names the library uses; anything not listed is matched by its own name.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("orca", &["orca", "killer whale", "killerwhale"]),
    (
        "beach ball",
        &[
            "beach ball",
            "beachball",
            "bball",
            "beach-ball",
            "beachballs",
        ],
    ),
    (
        "swim ring",
        &["swim ring", "swimring", "swim ring", "floatie", "pool ring"],
    ),
    (
        "swim wing",
        &["swim wing", "swimwing", "armband", "water wing"],
    ),
    ("island", &["island"]),
    ("dragon", &["dragon"]),
    ("boat", &["boat", "dinghy", "raft"]),
    ("crocodile", &["crocodile", "croc", "alligator", "gator"]),
    ("dolphin", &["dolphin"]),
    ("horse", &["horse", "pony"]),
    ("seal", &["seal", "sea lion", "sealion"]),
    ("gryphon", &["gryphon", "griffin", "griffon"]),
    ("nemo", &["nemo", "clownfish", "clown fish"]),
    ("deer", &["deer", "stag", "doe", "reindeer"]),
    ("cat", &["cat", "kitty", "kitten"]),
    ("balloons", &["balloon", "balloons"]),
    ("pool", &["pool"]),
    ("latex", &["latex", "rubber"]),
    ("popfest", &["popfest"]),
    ("outdoor", &["outdoor", "outside", "garden"]),
];

/// Text used for matching: the file name plus the folder path, with separators
/// and punctuation flattened so "Beach-Ball_2" matches "beach ball".
fn haystack(file_name: &str, path: &str) -> String {
    let mut text = String::with_capacity(file_name.len() + path.len() + 2);
    text.push_str(file_name);
    text.push(' ');
    text.push_str(path);
    let mut out = String::with_capacity(text.len() + 8);
    let mut last_was_space = false;
    for ch in text.chars() {
        let mapped = if ch.is_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            ' '
        };
        if mapped == ' ' {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            out.push(mapped);
            last_was_space = false;
        }
    }
    out
}

/// Whole-word search, so "cat" does not match "category" and "seal" does not
/// match "sealed".
fn contains_word(text: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(pos) = text[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before_ok = start == 0 || text.as_bytes()[start - 1] == b' ';
        let after_ok = end == text.len() || text.as_bytes()[end] == b' ';
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

fn keys_for(name: &str) -> Vec<String> {
    let lower = name.trim().to_ascii_lowercase();
    let mut keys: Vec<String> = SYNONYMS
        .iter()
        .find(|(canonical, _)| *canonical == lower)
        .map(|(_, list)| list.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    if keys.is_empty() {
        keys.push(lower.clone());
        // "Beach Ball" should also match a bare "beachball" in one token.
        let squeezed: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
        if squeezed != lower {
            keys.push(squeezed);
        }
    } else {
        let squeezed: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
        if squeezed != lower && !keys.iter().any(|k| k == &squeezed) {
            keys.push(squeezed);
        }
    }
    keys
}

/// Remove the parts of `text` covered by `needle` so a shorter name inside a
/// longer one does not match twice: "killer whale" must not also propose
/// "Whale", and "beach ball" must not also propose "Ball".
fn without_matches(text: &str, needle: &str) -> String {
    let mut out = text.to_string();
    let mut from = 0;
    while let Some(pos) = out[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before_ok = start == 0 || out.as_bytes()[start - 1] == b' ';
        let after_ok = end == out.len() || out.as_bytes()[end] == b' ';
        if before_ok && after_ok {
            let blank = " ".repeat(needle.len());
            out.replace_range(start..end, &blank);
            from = start + needle.len();
        } else {
            from = start + 1;
        }
        if from >= out.len() {
            break;
        }
    }
    out
}

/// Look for a creator name in the file name, preferring a prefix match because
/// files are usually named "<Creator> - <title>".
fn match_creator<'a>(
    creators: &'a [String],
    text: &str,
) -> Option<(&'a String, f64, &'static str)> {
    let mut best: Option<(&String, f64, &'static str)> = None;
    for creator in creators {
        let key = creator.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        if text.starts_with(&key) {
            return Some((creator, 0.9, "starts with this creator"));
        }
        if contains_word(text, &key) {
            let score = 0.75;
            if best.map(|(_, s, _)| score > s).unwrap_or(true) {
                best = Some((creator, score, "mentions this creator"));
            }
        }
    }
    best
}

/// Build suggestions for one video. `known_*` are the library's existing names;
/// `already_toys`/`already_tags` hold what the video has now, so accepted
/// proposals disappear from the list.
#[allow(clippy::too_many_arguments)]
pub fn suggest(
    file_name: &str,
    path: &str,
    creator: Option<&str>,
    already_toys: &[String],
    already_tags: &[String],
    known_toys: &[String],
    known_tags: &[String],
    known_creators: &[String],
) -> Vec<Suggestion> {
    let text = haystack(file_name, path);
    let mut out = Vec::new();
    let mut claimed: HashSet<String> = HashSet::new();

    if creator.map(str::trim).unwrap_or("").is_empty() {
        if let Some((name, confidence, reason)) = match_creator(known_creators, &text) {
            out.push(Suggestion {
                kind: "creator".into(),
                name: name.clone(),
                confidence,
                reason: reason.into(),
            });
            claimed.insert(name.to_ascii_lowercase());
        }
    }

    let taken_toys: HashSet<String> = already_toys
        .iter()
        .map(|t| t.trim().to_ascii_lowercase())
        .collect();

    // Every toy the text mentions, not just the first: a clip can have several.
    // Matched words are blanked out as we go, so a longer name that already
    // matched does not also produce a shorter name inside it ("Killer Whale"
    // must not propose "Whale"). Longest names are tried first for the same
    // reason.
    let longest_first = {
        let mut names: Vec<&String> = known_toys.iter().collect();
        names.sort_by_key(|name| {
            std::cmp::Reverse(keys_for(name).iter().map(|k| k.len()).max().unwrap_or(0))
        });
        names
    };
    let mut pool = text.clone();
    for name in longest_first {
        let key = name.trim().to_ascii_lowercase();
        if claimed.contains(&key) || taken_toys.contains(&key) {
            continue;
        }
        let mut hit = None;
        for needle in keys_for(name) {
            if contains_word(&pool, &needle) {
                hit = Some(needle);
                break;
            }
        }
        let Some(needle) = hit else { continue };
        out.push(Suggestion {
            kind: "toy".into(),
            name: name.clone(),
            confidence: 0.8,
            reason: "name or folder mentions this toy".into(),
        });
        claimed.insert(key);
        pool = without_matches(&pool, &needle);
    }

    let taken_tags: HashSet<String> = already_tags
        .iter()
        .map(|t| t.trim().to_ascii_lowercase())
        .collect();
    for name in known_tags {
        let key = name.trim().to_ascii_lowercase();
        if claimed.contains(&key) || taken_tags.contains(&key) {
            continue;
        }
        let mut hit = None;
        for needle in keys_for(name) {
            if contains_word(&pool, &needle) {
                hit = Some(needle);
                break;
            }
        }
        let Some(needle) = hit else { continue };
        out.push(Suggestion {
            kind: "tag".into(),
            name: name.clone(),
            confidence: 0.7,
            reason: "name or folder mentions this tag".into(),
        });
        claimed.insert(key);
        pool = without_matches(&pool, &needle);
    }

    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    // Keep the list short: too many chips turn into noise.
    out.truncate(6);
    out
}

/// Suggestions for a video id, using the library's current vocabulary.
pub fn suggest_for(conn: &rusqlite::Connection, id: i64) -> Result<Vec<Suggestion>, String> {
    let video = db::get_video(conn, id)
        .map_err(|e| e.to_string())?
        .ok_or("Video not found.")?;
    let toys: Vec<String> = db::all_toys(conn)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|t| t.name)
        .collect();
    let tags: Vec<String> = db::all_tags(conn)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|t| t.name)
        .collect();
    let creators = db::creators(conn).map_err(|e| e.to_string())?;
    Ok(suggest(
        &video.file_name,
        &video.path,
        video.creator.as_deref(),
        &video.toys,
        &video.tags,
        &toys,
        &tags,
        &creators,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn finds_toys_by_synonyms_in_the_name() {
        let out = suggest(
            "Quartirra - beachball and orca pool fun.mp4",
            "/lib/Quartirra/beachball and orca pool fun.mp4",
            None,
            &[],
            &[],
            &s(&["Beach Ball", "Orca", "Dragon"]),
            &s(&["Pool"]),
            &s(&["Quartirra"]),
        );
        let names: Vec<&str> = out.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"Beach Ball"), "{names:?}");
        assert!(names.contains(&"Orca"), "{names:?}");
        assert!(names.contains(&"Pool"), "{names:?}");
        assert_eq!(out[0].kind, "creator");
        assert_eq!(out[0].name, "Quartirra");
    }

    #[test]
    fn suggests_the_creator_from_a_prefix() {
        let out = suggest(
            "Fuusen - horse ride.mp4",
            "/lib/whatever/Fuusen - horse ride.mp4",
            None,
            &[],
            &[],
            &s(&["Horse"]),
            &[],
            &s(&["Fuusen", "Galas"]),
        );
        assert_eq!(out[0].kind, "creator");
        assert_eq!(out[0].name, "Fuusen");
        assert!(out[0].confidence > 0.8);
    }

    #[test]
    fn skips_labels_the_video_already_has() {
        let out = suggest(
            "orca.mp4",
            "/lib/orca.mp4",
            Some("Quartirra"),
            &s(&["Orca"]),
            &[],
            &s(&["Orca"]),
            &[],
            &s(&["Quartirra"]),
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_longer_name_does_not_also_suggest_its_substring() {
        let out = suggest(
            "2008-03-22_MX_Killer_Whale_Destruction.mp4",
            "/lib/MX/Killer Whale.mp4",
            None,
            &[],
            &[],
            &s(&["Orca", "Whale"]),
            &[],
            &[],
        );
        let names: Vec<&str> = out.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["Orca"], "{out:?}");
    }

    #[test]
    fn word_matching_does_not_match_substrings() {
        assert!(contains_word("the orca jumps", "orca"));
        assert!(!contains_word("orcas jumps", "orca"));
        assert!(!contains_word("category", "cat"));
        assert!(!contains_word("sealed box", "seal"));
    }

    #[test]
    fn separators_are_flattened() {
        let text = haystack("Beach-Ball_2.mp4", "/lib/Beach_Ball/");
        assert!(contains_word(&text, "beach ball"), "{text}");
    }

    #[test]
    fn unknown_names_still_match_their_own_spelling() {
        let out = suggest(
            "pokemon floatie.mp4",
            "/lib/pokemon floatie.mp4",
            None,
            &[],
            &[],
            &s(&["Pokemon"]),
            &[],
            &[],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Pokemon");
    }
}
