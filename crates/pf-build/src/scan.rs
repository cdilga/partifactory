//! Small, byte-preserving scanners for CSS and JavaScript: they find the URL-bearing spans
//! (`url()`, `@import`, `import`/`export … from`, literal `import("…")`) and the
//! `sourceMappingURL` comments, so the rewrites can change exactly those bytes and nothing else.
//! They are deliberately lexers, not parsers: everything they don't recognise is copied as is.

use std::ops::Range;

/// A replacement of `range` (byte offsets into the scanned text) by `with`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub with: String,
}

/// Apply non-overlapping edits (any order).
pub fn apply(text: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|e| e.range.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for e in edits {
        if e.range.start < at {
            continue; // overlapping: keep the first
        }
        out.push_str(&text[at..e.range.start]);
        out.push_str(&e.with);
        at = e.range.end;
    }
    out.push_str(&text[at..]);
    out
}

fn starts_ci(b: &[u8], i: usize, pat: &str) -> bool {
    b.len() >= i + pat.len() && b[i..i + pat.len()].eq_ignore_ascii_case(pat.as_bytes())
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// End (exclusive, after the closing quote) of a string starting at `i` (the quote).
fn string_end(b: &[u8], i: usize) -> usize {
    let q = b[i];
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            c if c == q => return j + 1,
            b'\n' if q != b'`' => return j, // unterminated
            _ => j += 1,
        }
    }
    b.len()
}

/// The text inside a string literal that starts at `i` (the quote) and ends at `end`.
fn inner_range(b: &[u8], i: usize, end: usize) -> Range<usize> {
    if end >= i + 2 && b[end - 1] == b[i] {
        (i + 1)..(end - 1)
    } else {
        (i + 1)..end
    }
}

/// The body of a comment starting at `i` (`/*`) and ending at `end`.
fn comment_body(text: &str, i: usize, end: usize) -> &str {
    if end >= i + 4 && text[..end].ends_with("*/") {
        &text[i + 2..end - 2]
    } else {
        &text[i + 2..end]
    }
}

// ------------------------------------------------------------------ CSS

/// What a CSS reference is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssRefKind {
    Url,
    Import,
}

/// A URL in CSS: `range` covers the URL text only (inside any quotes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssRef {
    pub range: Range<usize>,
    pub url: String,
    pub kind: CssRefKind,
}

#[derive(Debug, Default, Clone)]
pub struct CssScan {
    pub refs: Vec<CssRef>,
    /// `/*# sourceMappingURL=… */` comments (whole comment).
    pub source_maps: Vec<Range<usize>>,
}

pub fn scan_css(text: &str) -> CssScan {
    let b = text.as_bytes();
    let mut out = CssScan::default();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let end = text[i + 2..]
                .find("*/")
                .map(|p| i + 2 + p + 2)
                .unwrap_or(b.len());
            let body = comment_body(text, i, end).trim_start();
            if body.starts_with("# sourceMappingURL=") || body.starts_with("@ sourceMappingURL=") {
                out.source_maps.push(i..end);
            }
            i = end;
        } else if c == b'"' || c == b'\'' {
            i = string_end(b, i);
        } else if c == b'@' && starts_ci(b, i, "@import") {
            let j = skip_ws(b, i + 7);
            if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                let end = string_end(b, j);
                let inner = inner_range(b, j, end);
                out.refs.push(CssRef {
                    url: text[inner.clone()].to_string(),
                    range: inner,
                    kind: CssRefKind::Import,
                });
                i = end;
            } else {
                i = j;
            }
        } else if (c == b'u' || c == b'U')
            && starts_ci(b, i, "url(")
            && (i == 0
                || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'-' || b[i - 1] == b'_'))
        {
            let is_import = text[..i]
                .trim_end()
                .to_ascii_lowercase()
                .ends_with("@import");
            let kind = if is_import {
                CssRefKind::Import
            } else {
                CssRefKind::Url
            };
            let j = skip_ws(b, i + 4);
            if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                let end = string_end(b, j);
                let inner = inner_range(b, j, end);
                out.refs.push(CssRef {
                    url: text[inner.clone()].to_string(),
                    range: inner,
                    kind,
                });
                i = end;
            } else {
                let close = text[j..].find(')').map(|p| j + p).unwrap_or(b.len());
                let raw = &text[j..close];
                let trimmed = raw.trim_end();
                let inner = j..j + trimmed.len();
                if !trimmed.is_empty() {
                    out.refs.push(CssRef {
                        url: trimmed.to_string(),
                        range: inner,
                        kind,
                    });
                }
                i = close;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Rewrite every CSS URL `f` maps and strip `sourceMappingURL` comments.
pub fn rewrite_css(text: &str, f: &mut dyn FnMut(&str, CssRefKind) -> Option<String>) -> String {
    let s = scan_css(text);
    let mut edits: Vec<Edit> = s
        .source_maps
        .into_iter()
        .map(|r| Edit {
            range: r,
            with: String::new(),
        })
        .collect();
    for r in s.refs {
        if let Some(w) = f(&r.url, r.kind) {
            edits.push(Edit {
                range: r.range,
                with: w,
            });
        }
    }
    apply(text, edits)
}

// ------------------------------------------------------------------ JavaScript

/// A module specifier in JS: `range` covers the text inside the quotes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsSpecifier {
    pub range: Range<usize>,
    pub spec: String,
}

#[derive(Debug, Default, Clone)]
pub struct JsScan {
    /// `import … from "x"`, `export … from "x"`, `import "x"`, `import("x")`.
    pub specifiers: Vec<JsSpecifier>,
    /// Other string literals that look like root-relative paths (`"/data.json"`).
    pub root_relative: Vec<String>,
    /// `//# sourceMappingURL=…` comments.
    pub source_maps: Vec<Range<usize>>,
}

fn ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// The identifier ending right before `end` (after skipping whitespace back), with its start.
fn word_before(b: &[u8], end: usize) -> (usize, &[u8]) {
    let mut j = end;
    while j > 0 && b[j - 1].is_ascii_whitespace() {
        j -= 1;
    }
    let e = j;
    while j > 0 && ident_byte(b[j - 1]) {
        j -= 1;
    }
    (j, &b[j..e])
}

/// Whether a `/` at `i` starts a regex literal (by the previous significant byte).
fn regex_allowed(b: &[u8], i: usize) -> bool {
    let mut j = i;
    while j > 0 && b[j - 1].is_ascii_whitespace() {
        j -= 1;
    }
    if j == 0 {
        return true;
    }
    let p = b[j - 1];
    if b"(,=:[!&|?{};+-*%<>~^".contains(&p) {
        return true;
    }
    let (_, w) = word_before(b, j);
    matches!(
        w,
        b"return"
            | b"typeof"
            | b"case"
            | b"do"
            | b"else"
            | b"in"
            | b"of"
            | b"new"
            | b"delete"
            | b"void"
            | b"throw"
    )
}

fn regex_end(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    let mut class = false;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'[' => {
                class = true;
                j += 1
            }
            b']' => {
                class = false;
                j += 1
            }
            b'/' if !class => return j + 1,
            b'\n' => return j,
            _ => j += 1,
        }
    }
    b.len()
}

fn looks_root_relative(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() > 2
        && b[0] == b'/'
        && (b[1].is_ascii_alphanumeric() || b[1] == b'_')
        && !s.contains(char::is_whitespace)
        && (s[1..].contains('/') || s[1..].contains('.'))
}

pub fn scan_js(text: &str) -> JsScan {
    let b = text.as_bytes();
    let mut out = JsScan::default();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = text[i..].find('\n').map(|p| i + p).unwrap_or(b.len());
            let body = &text[i + 2..end];
            if body.starts_with("# sourceMappingURL=") || body.starts_with("@ sourceMappingURL=") {
                out.source_maps.push(i..end);
            }
            i = end;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let end = text[i + 2..]
                .find("*/")
                .map(|p| i + 2 + p + 2)
                .unwrap_or(b.len());
            let body = comment_body(text, i, end).trim_start();
            if body.starts_with("# sourceMappingURL=") {
                out.source_maps.push(i..end);
            }
            i = end;
        } else if c == b'"' || c == b'\'' || c == b'`' {
            let end = string_end(b, i);
            let inner = inner_range(b, i, end);
            let lit = &text[inner.clone()];
            // What precedes the literal decides whether it is a module specifier.
            let mut j = i;
            while j > 0 && b[j - 1].is_ascii_whitespace() {
                j -= 1;
            }
            let is_spec = if c == b'`' {
                false
            } else if j > 0 && b[j - 1] == b'(' {
                let (ws, w) = word_before(b, j - 1);
                w == b"import" && (ws == 0 || b[ws - 1] != b'.')
            } else {
                let (ws, w) = word_before(b, j);
                (w == b"from" || w == b"import") && (ws == 0 || b[ws - 1] != b'.')
            };
            if is_spec {
                out.specifiers.push(JsSpecifier {
                    range: inner,
                    spec: lit.to_string(),
                });
            } else if c != b'`' && looks_root_relative(lit) {
                out.root_relative.push(lit.to_string());
            }
            i = end;
        } else if c == b'/' && regex_allowed(b, i) {
            i = regex_end(b, i);
        } else {
            i += 1;
        }
    }
    out
}

/// Rewrite every specifier `f` maps and strip `sourceMappingURL` comments. Returns the text and
/// the root-relative literals no rewrite can reach (for the `root_relative_url` lint).
pub fn rewrite_js(text: &str, f: &mut dyn FnMut(&str) -> Option<String>) -> (String, Vec<String>) {
    let s = scan_js(text);
    let mut edits: Vec<Edit> = s
        .source_maps
        .into_iter()
        .map(|r| Edit {
            range: r,
            with: String::new(),
        })
        .collect();
    for sp in s.specifiers {
        if let Some(w) = f(&sp.spec) {
            edits.push(Edit {
                range: sp.range,
                with: w,
            });
        }
    }
    (apply(text, edits), s.root_relative)
}

// ------------------------------------------------------------------ srcset

/// Split a `srcset` into `(url range, url)` candidates.
pub fn srcset_urls(v: &str) -> Vec<(Range<usize>, String)> {
    let b = v.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
            i += 1;
        }
        let s = i;
        while i < b.len() && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut e = i;
        // A URL can't end in a comma: a trailing comma separates candidates.
        while e > s && b[e - 1] == b',' {
            e -= 1;
        }
        if e > s {
            out.push((s..e, v[s..e].to_string()));
        }
        if e < i {
            continue;
        }
        // Skip descriptors up to the next comma.
        while i < b.len() && b[i] != b',' {
            i += 1;
        }
    }
    out
}

pub fn rewrite_srcset(v: &str, f: &mut dyn FnMut(&str) -> Option<String>) -> Option<String> {
    let edits: Vec<Edit> = srcset_urls(v)
        .into_iter()
        .filter_map(|(r, u)| f(&u).map(|w| Edit { range: r, with: w }))
        .collect();
    if edits.is_empty() {
        None
    } else {
        Some(apply(v, edits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_urls_and_imports() {
        let css = r#"@import "a.css"; @import url('b.css'); /* url(no.png) */
.x{background:url( /img/x.png ) no-repeat} .y{background:URL("data:image/png;base64,AA")}
.z{content:"url(not)"}
/*# sourceMappingURL=app.css.map */"#;
        let s = scan_css(css);
        let urls: Vec<_> = s.refs.iter().map(|r| (r.url.as_str(), r.kind)).collect();
        assert_eq!(
            urls,
            [
                ("a.css", CssRefKind::Import),
                ("b.css", CssRefKind::Import),
                ("/img/x.png", CssRefKind::Url),
                ("data:image/png;base64,AA", CssRefKind::Url),
            ]
        );
        assert_eq!(s.source_maps.len(), 1);
        let out = rewrite_css(css, &mut |u, _| {
            u.strip_prefix('/').map(|r| format!("./{r}"))
        });
        assert!(out.contains("url( ./img/x.png )"));
        assert!(!out.contains("sourceMappingURL"));
        assert!(out.contains("url(no.png)"), "comments untouched");
    }

    #[test]
    fn js_specifiers() {
        let js = r#"import React from "react";
import{a as b}from"/npm/x@1/+esm";
export * from '/stable/y.mjs';
import './side.js';
const m = await import("https://esm.sh/z@2");
obj.import("nope"); const s = "from"; const re = /"from"/g;
fetch("/data.json"); const t = `import "q"`;
//# sourceMappingURL=main.js.map
"#;
        let s = scan_js(js);
        let specs: Vec<_> = s.specifiers.iter().map(|x| x.spec.as_str()).collect();
        assert_eq!(
            specs,
            [
                "react",
                "/npm/x@1/+esm",
                "/stable/y.mjs",
                "./side.js",
                "https://esm.sh/z@2"
            ]
        );
        assert_eq!(s.root_relative, ["/data.json"]);
        assert_eq!(s.source_maps.len(), 1);
        let (out, _) = rewrite_js(js, &mut |sp| (sp == "react").then(|| "X".to_string()));
        assert!(out.contains(r#"import React from "X";"#));
        assert!(!out.contains("sourceMappingURL"));
    }

    #[test]
    fn srcset_candidates() {
        let v = "/a.png 1x, https://cdn.example/b.png 2x,/c.png";
        let urls: Vec<_> = srcset_urls(v).into_iter().map(|x| x.1).collect();
        assert_eq!(urls, ["/a.png", "https://cdn.example/b.png", "/c.png"]);
        let out =
            rewrite_srcset(v, &mut |u| u.strip_prefix('/').map(|r| format!("./{r}"))).unwrap();
        assert_eq!(out, "./a.png 1x, https://cdn.example/b.png 2x,./c.png");
    }
}
