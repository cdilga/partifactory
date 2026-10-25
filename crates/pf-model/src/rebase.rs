//! Bounded, useful diagnostics for byte-exact anchors.
use crate::PfError;

pub fn anchor_error(text: &str, anchor: &str, file: &str, count: usize) -> PfError {
    let lines: Vec<&str> = text.lines().collect();
    let wanted: Vec<&str> = anchor.lines().collect();
    let width = wanted.len().max(1).min(lines.len().max(1));
    let mut best = (0usize, 0.0f64);
    let anchor_preview: String = anchor.chars().take(1024).collect();
    let mut cells_left = 8_000_000usize;
    // Bound diagnostic work even on hostile files; anchors still match the full file above.
    for (i, window) in lines.windows(width).take(2048).enumerate() {
        let candidate = window.join("\n");
        let candidate: String = candidate.chars().take(1024).collect();
        let cells = candidate.chars().count().max(1) * anchor_preview.chars().count().max(1);
        if cells > cells_left { break; }
        cells_left -= cells;
        let ratio = similarity(&candidate, &anchor_preview);
        if ratio > best.1 {
            best = (i, ratio);
        }
    }
    let (start, end, context) = if best.1 >= 0.6 || count > 1 {
        let i = if count > 1 {
            text[..text.find(anchor).unwrap_or(0)].lines().count()
        } else {
            best.0
        };
        let start = i.saturating_sub(3);
        let end = (i + width + 3).min(lines.len());
        (start + 1, end, lines[start..end].join("\n"))
    } else {
        (
            1,
            text.chars().take(600).filter(|c| *c == '\n').count() + 1,
            text.chars().take(600).collect(),
        )
    };
    let context: String = context
        .chars()
        .scan(0usize, |n, c| {
            *n += c.len_utf8();
            (*n <= 2048).then_some(c)
        })
        .collect();
    PfError::new(
        if count == 0 {
            "anchor_not_found"
        } else {
            "anchor_ambiguous"
        },
        if count == 0 {
            "old_str does not occur byte-for-byte; copy an exact anchor from part_get"
        } else {
            "old_str occurs more than once; include more surrounding text"
        },
    )
    .with("file", file)
    .with("count", count)
    .with("context", context)
    .with("lines", serde_json::json!([start, end]))
    .with("similarity", best.1)
}

fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(old + 1)
                .min(previous + usize::from(x != y));
            previous = old;
        }
    }
    1.0 - row[b.len()] as f64 / a.len().max(b.len()) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn useful_context_and_bounds() {
        let e = anchor_error(
            "alpha\npacking passport\nomega",
            "packing passports",
            "index.txt",
            0,
        );
        assert!(e.details["similarity"].as_f64().unwrap() >= 0.6);
        assert!(e.details["context"].as_str().unwrap().contains("passport"));
        let e = anchor_error(&"é".repeat(3000), "missing", "x", 0);
        assert!(e.details["context"].as_str().unwrap().len() <= 2048);
    }
}
