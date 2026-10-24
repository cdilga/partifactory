//! Names (§4): a part's name is its host label. `[a-z0-9-]`, 1–58 characters, no leading,
//! trailing or double hyphens. `blobs` is reserved in code; site config reserves more; and every
//! name ever used stays reserved forever (the `names` table), because renames 301 forever.

use crate::PfError;
use pf_db::rusqlite::{Connection, OptionalExtension};

pub const MAX_LEN: usize = 58;
/// Reserved in code, whatever the config says: the shared blob host.
pub const RESERVED_IN_CODE: &[&str] = &["blobs"];

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "the", "of", "for", "to", "in", "on", "at", "by", "with", "my", "our",
    "your", "is", "it", "this", "that", "from", "or", "as", "be", "are", "new",
];

/// Why a name isn't valid, or `Ok`.
pub fn validate(name: &str) -> Result<(), PfError> {
    let bad = |why: &str| PfError::invalid("name", format!("name {name:?} {why}"));
    if name.is_empty() || name.len() > MAX_LEN {
        return Err(bad("must be 1-58 characters"));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(bad("may use only a-z, 0-9 and -"));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(bad("may not start or end with -"));
    }
    if name.contains("--") {
        return Err(bad("may not contain --"));
    }
    Ok(())
}

/// Reserved by code or by site config (`[names] reserved`).
pub fn is_reserved(name: &str, reserved: &[String]) -> bool {
    RESERVED_IN_CODE.contains(&name) || reserved.iter().any(|r| r == name)
}

/// Lower-case `[a-z0-9]` tokens of free text.
fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn join_trim(tokens: &[&str]) -> String {
    let mut s = String::new();
    for t in tokens {
        let next = if s.is_empty() {
            t.to_string()
        } else {
            format!("{s}-{t}")
        };
        if next.len() > MAX_LEN {
            break;
        }
        s = next;
    }
    if s.is_empty()
        && let Some(t) = tokens.first()
    {
        s = t[..t.len().min(MAX_LEN)].to_string();
    }
    s
}

/// The name a title implies: "Sleep chart" → `sleep-chart`. Never empty (falls back to `part`).
pub fn derive(title: &str) -> String {
    let t = tokens(title);
    let refs: Vec<&str> = t.iter().map(String::as_str).collect();
    let s = join_trim(&refs);
    if s.is_empty() { "part".to_string() } else { s }
}

/// Candidate names, best first, deterministic (no LLM): the derived name, 2- and 3-token
/// subsets of the title and description without stop words, the space as a prefix, then numeric
/// suffixes last. Not filtered for availability.
pub fn candidates(title: &str, description: &str, space_slug: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |s: String, out: &mut Vec<String>| {
        if validate(&s).is_ok() && !out.contains(&s) {
            out.push(s);
        }
    };
    let base = derive(title);
    push(base.clone(), &mut out);
    let mut words: Vec<String> = tokens(title);
    for w in tokens(description) {
        if !words.contains(&w) {
            words.push(w);
        }
    }
    let words: Vec<&str> = words
        .iter()
        .map(String::as_str)
        .filter(|w| !STOP_WORDS.contains(w))
        .collect();
    let words = &words[..words.len().min(8)];
    for i in 0..words.len() {
        for j in i + 1..words.len() {
            push(join_trim(&[words[i], words[j]]), &mut out);
        }
    }
    for i in 0..words.len() {
        for j in i + 1..words.len() {
            for k in j + 1..words.len() {
                push(join_trim(&[words[i], words[j], words[k]]), &mut out);
            }
        }
    }
    if let Some(space) = space_slug {
        let sp = derive(space.trim_start_matches('~'));
        push(join_trim(&[&sp, &base]), &mut out);
    }
    for n in 2..=99 {
        let suffix = format!("-{n}");
        let stem = &base[..base.len().min(MAX_LEN - suffix.len())];
        push(format!("{}{suffix}", stem.trim_end_matches('-')), &mut out);
    }
    out
}

/// `true` when `name` was ever used (any state in `names`) or is a live part's name.
pub fn is_used(conn: &Connection, name: &str) -> pf_db::rusqlite::Result<bool> {
    let n: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM names WHERE name = ?1 UNION ALL SELECT 1 FROM parts WHERE name = ?1 LIMIT 1",
            [name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(n.is_some())
}

/// Whether `name` can be taken now.
pub fn is_free(
    conn: &Connection,
    name: &str,
    reserved: &[String],
) -> pf_db::rusqlite::Result<bool> {
    Ok(validate(name).is_ok() && !is_reserved(name, reserved) && !is_used(conn, name)?)
}

/// Up to `limit` free alternatives, best first.
pub fn suggestions(
    conn: &Connection,
    title: &str,
    description: &str,
    space_slug: Option<&str>,
    reserved: &[String],
    limit: usize,
) -> pf_db::rusqlite::Result<Vec<String>> {
    let mut out = Vec::new();
    for c in candidates(title, description, space_slug) {
        if out.len() >= limit {
            break;
        }
        if is_free(conn, &c, reserved)? {
            out.push(c);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_table() {
        let long_ok = "a".repeat(58);
        let long_bad = "a".repeat(59);
        for (name, ok) in [
            ("a", true),
            ("sleep-chart", true),
            ("a1-b2-c3", true),
            (long_ok.as_str(), true),
            ("", false),
            (long_bad.as_str(), false),
            ("-a", false),
            ("a-", false),
            ("a--b", false),
            ("A", false),
            ("a_b", false),
            ("a.b", false),
            ("ä", false),
        ] {
            assert_eq!(validate(name).is_ok(), ok, "{name:?}");
        }
    }

    #[test]
    fn reserved_in_code_and_config() {
        assert!(is_reserved("blobs", &[]));
        let cfg = vec!["admin".to_string(), "www".to_string()];
        assert!(is_reserved("admin", &cfg));
        assert!(!is_reserved("admin", &[]));
        assert!(!is_reserved("garden", &cfg));
    }

    #[test]
    fn derive_titles() {
        assert_eq!(derive("Sleep chart"), "sleep-chart");
        assert_eq!(derive("  Mum's 70th -- party!! "), "mum-s-70th-party");
        assert_eq!(derive("!!!"), "part");
        let long = derive(&"word ".repeat(30));
        assert!(long.len() <= 58 && validate(&long).is_ok());
    }

    #[test]
    fn candidates_are_deterministic_valid_and_ranked() {
        let a = candidates(
            "The packing list for the trip",
            "Summer holiday",
            Some("~admin"),
        );
        let b = candidates(
            "The packing list for the trip",
            "Summer holiday",
            Some("~admin"),
        );
        assert_eq!(a, b);
        assert_eq!(a[0], "the-packing-list-for-the-trip");
        assert!(a.contains(&"packing-list".to_string()));
        assert!(a.contains(&"admin-the-packing-list-for-the-trip".to_string()));
        assert!(a.iter().all(|c| validate(c).is_ok()));
        // Numeric suffixes come last.
        let first_numeric = a.iter().position(|c| c.ends_with("-2")).unwrap();
        assert!(
            a[first_numeric..]
                .iter()
                .all(|c| c.rsplit('-').next().unwrap().parse::<u32>().is_ok())
        );
    }
}
