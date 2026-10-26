//! Block ids for Markdown files (PLAN §3c "Long documents", decision 54). A block is a top-level
//! comrak node. Ids live in the version's manifest (`meta.blocks`), so they're versioned
//! atomically with the text, and carry over from the parent version on every commit: exact hash
//! matches in order first, then same-kind blocks with similar words, then fresh ids.
use crate::PfError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// One block of one file: `[start, end)` byte range, kind and content hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRec {
    pub id: String,
    pub kind: String,
    pub start: usize,
    pub end: usize,
    pub hash: String,
}

pub type FileBlocks = BTreeMap<String, Vec<BlockRec>>;

pub fn is_markdown(path: &str) -> bool {
    path.ends_with(".md") || path.ends_with(".markdown")
}

/// Top-level blocks as `(kind, start, end)`, end exclusive and without the trailing newline.
pub fn parse(text: &str) -> Vec<(String, usize, usize)> {
    use comrak::nodes::NodeValue;
    let mut options = comrak::Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.tasklist = true;
    options.extension.autolink = true;
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, text, &options);
    let mut line_starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let offset = |line: usize, column: usize| -> usize {
        let base = line_starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(text.len());
        (base + column).min(text.len())
    };
    let mut out = vec![];
    for node in root.children() {
        let data = node.data.borrow();
        let kind = match data.value {
            NodeValue::Heading(_) => "heading",
            NodeValue::Paragraph => "paragraph",
            NodeValue::List(_) => "list",
            NodeValue::BlockQuote => "quote",
            NodeValue::CodeBlock(_) => "code",
            NodeValue::Table(_) => "table",
            NodeValue::HtmlBlock(_) => "html",
            NodeValue::ThematicBreak => "divider",
            _ => "other",
        };
        let sp = data.sourcepos;
        let start = offset(sp.start.line, sp.start.column.saturating_sub(1));
        let mut end = offset(sp.end.line, sp.end.column).max(start);
        // Never cut a UTF-8 character, and drop a trailing newline comrak may include.
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        while end > start && text.as_bytes()[end - 1] == b'\n' {
            end -= 1;
        }
        out.push((kind.to_owned(), start, end));
    }
    out
}

fn hash(s: &str) -> String {
    pf_blobs::sha256_hex(s.as_bytes())[..16].to_owned()
}

/// A new 8-character Crockford base32 id not in `used`, derived from `seed` (the block's hash
/// and position), so the same text always gets the same first ids and part_get agrees with edits.
fn fresh(used: &BTreeSet<String>, seed: &str) -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    for attempt in 0.. {
        let digest = pf_blobs::sha256_hex(format!("{seed}:{attempt}").as_bytes());
        let id: String = digest
            .bytes()
            .take(8)
            .map(|b| ALPHABET[(b as usize * 7 + 3) % 32] as char)
            .collect();
        if !used.contains(&id) {
            return id;
        }
    }
    unreachable!("an unused id always exists")
}

fn words(s: &str) -> BTreeSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Blocks of `new_text`, with ids carried from `old` (blocks of `old_text`).
pub fn carry(old: &[BlockRec], old_text: &str, new_text: &str) -> Vec<BlockRec> {
    let parsed = parse(new_text);
    let new: Vec<(String, usize, usize, String)> = parsed
        .into_iter()
        .map(|(k, s, e)| {
            let h = hash(&new_text[s..e]);
            (k, s, e, h)
        })
        .collect();
    let mut ids: Vec<Option<String>> = vec![None; new.len()];
    let mut taken = vec![false; old.len()];
    // Pass 1: exact hashes, longest common subsequence in order.
    let (n, m) = (new.len(), old.len());
    if n * m <= 4_000_000 {
        let mut dp = vec![vec![0u32; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i][j] = if new[i].3 == old[j].hash {
                    dp[i + 1][j + 1] + 1
                } else {
                    dp[i + 1][j].max(dp[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if new[i].3 == old[j].hash {
                ids[i] = Some(old[j].id.clone());
                taken[j] = true;
                i += 1;
                j += 1;
            } else if dp[i + 1][j] >= dp[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    // Pass 2: same kind, similar words (Jaccard >= 0.5), between the anchors of pass 1.
    for i in 0..n {
        if ids[i].is_some() {
            continue;
        }
        let lo = (0..i)
            .rev()
            .find_map(|k| {
                ids[k]
                    .as_ref()
                    .and_then(|id| old.iter().position(|o| &o.id == id))
            })
            .map_or(0, |p| p + 1);
        let hi = (i + 1..n)
            .find_map(|k| {
                ids[k]
                    .as_ref()
                    .and_then(|id| old.iter().position(|o| &o.id == id))
            })
            .unwrap_or(m);
        let mine = words(&new_text[new[i].1..new[i].2]);
        let best = (lo..hi.max(lo))
            .filter(|&j| !taken[j] && old[j].kind == new[i].0)
            .filter_map(|j| {
                let theirs = words(old_text.get(old[j].start..old[j].end).unwrap_or(""));
                let union = mine.union(&theirs).count();
                let score = if union == 0 {
                    1.0
                } else {
                    mine.intersection(&theirs).count() as f64 / union as f64
                };
                (score >= 0.5).then_some((j, score))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((j, _)) = best {
            ids[i] = Some(old[j].id.clone());
            taken[j] = true;
        }
    }
    // Pass 3: fresh ids, never one this file already uses.
    let mut used: BTreeSet<String> = old.iter().map(|o| o.id.clone()).collect();
    used.extend(ids.iter().flatten().cloned());
    new.into_iter()
        .zip(ids)
        .enumerate()
        .map(|(i, ((kind, start, end, hash), id))| {
            let id = id.unwrap_or_else(|| {
                let f = fresh(&used, &format!("{hash}:{i}"));
                used.insert(f.clone());
                f
            });
            BlockRec {
                id,
                kind,
                start,
                end,
                hash,
            }
        })
        .collect()
}

/// The `meta.blocks` of a manifest, if any.
pub fn from_meta(meta: &Value) -> FileBlocks {
    serde_json::from_value(meta.get("blocks").cloned().unwrap_or(Value::Null)).unwrap_or_default()
}
pub fn to_meta(blocks: &FileBlocks) -> Value {
    if blocks.is_empty() {
        json!({})
    } else {
        json!({"blocks": blocks})
    }
}

fn find<'a>(blocks: &'a [BlockRec], id: &str) -> Result<&'a BlockRec, PfError> {
    blocks.iter().find(|b| b.id == id).ok_or_else(|| {
        PfError::new(
            "block_not_found",
            "no block with that id; read the current ids with part_get",
        )
        .with("block", id)
        .with(
            "blocks",
            blocks
                .iter()
                .map(|b| json!({"id": b.id, "kind": b.kind}))
                .collect::<Vec<_>>(),
        )
    })
}

fn check_hash(text: &str, b: &BlockRec, if_hash: Option<&str>) -> Result<(), PfError> {
    match if_hash {
        Some(h) if h != b.hash => Err(PfError::new(
            "block_changed",
            "the block changed since you read it",
        )
        .with("block", b.id.as_str())
        .with("current", &text[b.start..b.end])
        .with("hash", b.hash.as_str())),
        _ => Ok(()),
    }
}

fn normalise(markdown: &str) -> &str {
    markdown.trim_matches('\n')
}

/// `{op: "replace_block"}`: the block's bytes become `markdown`; the block keeps its id.
pub fn replace(
    text: &str,
    blocks: &[BlockRec],
    id: &str,
    markdown: &str,
    if_hash: Option<&str>,
) -> Result<(String, Option<(usize, String)>), PfError> {
    let b = find(blocks, id)?;
    check_hash(text, b, if_hash)?;
    let out = format!(
        "{}{}{}",
        &text[..b.start],
        normalise(markdown),
        &text[b.end..]
    );
    Ok((out, Some((b.start, b.id.clone()))))
}

/// `{op: "insert_after"}`: `markdown` as new block(s) after `block`, or at the start.
pub fn insert_after(
    text: &str,
    blocks: &[BlockRec],
    after: &str,
    markdown: &str,
) -> Result<String, PfError> {
    let md = normalise(markdown);
    if after == "start" {
        let rest = text.trim_start_matches('\n');
        return Ok(if rest.is_empty() {
            format!("{md}\n")
        } else {
            format!("{md}\n\n{rest}")
        });
    }
    let b = find(blocks, after)?;
    let tail = &text[b.end..];
    Ok(format!(
        "{}\n\n{md}{}",
        &text[..b.end],
        if tail.is_empty() {
            "\n".to_owned()
        } else {
            tail.to_owned()
        }
    ))
}

/// `{op: "delete_block"}`: the block and the blank lines after it go.
pub fn delete(
    text: &str,
    blocks: &[BlockRec],
    id: &str,
    if_hash: Option<&str>,
) -> Result<String, PfError> {
    let b = find(blocks, id)?;
    check_hash(text, b, if_hash)?;
    let mut end = b.end;
    while end < text.len() && text.as_bytes()[end] == b'\n' {
        end += 1;
    }
    let start = if end == text.len() {
        text[..b.start].trim_end_matches('\n').len()
    } else {
        b.start
    };
    let mut out = format!("{}{}", &text[..start], &text[end..]);
    if end == text.len() && !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

/// Re-derive blocks after an edit, keeping `pinned` (a replaced block's id at its start offset).
pub fn reblock(
    old: &[BlockRec],
    old_text: &str,
    new_text: &str,
    pinned: Option<(usize, String)>,
) -> Vec<BlockRec> {
    let mut out = carry(old, old_text, new_text);
    if let Some((start, id)) = pinned
        && let Some(slot) = out.iter().position(|b| b.start == start)
    {
        // The replaced block keeps its id, wherever carry put it.
        if let Some(other) = out.iter().position(|b| b.id == id)
            && other != slot
        {
            let fresh_id = out[slot].id.clone();
            out[other].id = fresh_id;
        }
        out[slot].id = id;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Plan\n\nThe first paragraph has several words in it.\n\n- a\n- b\n\n| x | y |\n|---|:-:|\n| 1 | 2 |\n\n```\ncode\n```\n";

    #[test]
    fn blocks_cover_top_level_nodes_exactly() {
        let b = parse(DOC);
        let kinds: Vec<_> = b.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(kinds, ["heading", "paragraph", "list", "table", "code"]);
        assert_eq!(&DOC[b[0].1..b[0].2], "# Plan");
        assert_eq!(&DOC[b[2].1..b[2].2], "- a\n- b");
        assert_eq!(&DOC[b[3].1..b[3].2], "| x | y |\n|---|:-:|\n| 1 | 2 |");
        assert_eq!(&DOC[b[4].1..b[4].2], "```\ncode\n```");
    }

    #[test]
    fn ids_survive_edits_moves_and_small_rewrites() {
        let v1 = carry(&[], "", DOC);
        let ids: BTreeSet<_> = v1.iter().map(|b| b.id.clone()).collect();
        assert_eq!(ids.len(), v1.len());
        assert!(v1.iter().all(|b| b.id.len() == 8));
        // Insert a block at the top and lightly edit the paragraph.
        let v2text = DOC
            .replace("# Plan\n", "Intro.\n\n# Plan\n")
            .replace("several words", "many words");
        let v2 = carry(&v1, DOC, &v2text);
        assert_eq!(v2[1].id, v1[0].id, "heading kept by hash");
        assert_eq!(v2[2].id, v1[1].id, "paragraph kept by similarity");
        assert!(!ids.contains(&v2[0].id), "the new block gets a fresh id");
    }

    #[test]
    fn block_ops_resolve_by_id_and_check_hashes() {
        let b = carry(&[], "", DOC);
        let (t, pin) = replace(
            DOC,
            &b,
            &b[1].id,
            "Totally different words here.",
            Some(&b[1].hash),
        )
        .unwrap();
        let after = reblock(&b, DOC, &t, pin);
        assert_eq!(after[1].id, b[1].id, "replace keeps the id");
        assert!(t.contains("\n\nTotally different words here.\n\n- a"));
        assert_eq!(
            replace(DOC, &b, &b[1].id, "x", Some("stale"))
                .unwrap_err()
                .code,
            "block_changed"
        );
        assert_eq!(
            replace(DOC, &b, "NOPE0000", "x", None).unwrap_err().code,
            "block_not_found"
        );
        let ins = insert_after(DOC, &b, &b[0].id, "## Added").unwrap();
        assert!(ins.starts_with("# Plan\n\n## Added\n\nThe first paragraph"));
        assert!(
            insert_after(DOC, &b, "start", "Top")
                .unwrap()
                .starts_with("Top\n\n# Plan")
        );
        let del = delete(DOC, &b, &b[2].id, None).unwrap();
        assert!(del.contains("words in it.\n\n| x | y |"));
        let last = delete(DOC, &b, &b[4].id, None).unwrap();
        assert!(last.ends_with("| 1 | 2 |\n"));
    }
}
