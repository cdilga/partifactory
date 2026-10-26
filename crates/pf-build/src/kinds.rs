//! Kind detection (§2 Kinds). Built bundles always explicitly request `files`.

use serde::{Deserialize, Serialize};

/// The kinds the build child supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Html,
    Markdown,
    Text,
    Svg,
    Code,
    Files,
}

/// The kinds that exist in this release, for error messages.
pub const AVAILABLE_KINDS: &[&str] = &["html", "markdown", "text", "svg", "code", "files"];

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Html => "html",
            Kind::Markdown => "markdown",
            Kind::Text => "text",
            Kind::Svg => "svg",
            Kind::Code => "code",
            Kind::Files => "files",
        }
    }

    pub fn parse(s: &str) -> Result<Kind, KindError> {
        match s {
            "html" => Ok(Kind::Html),
            "markdown" => Ok(Kind::Markdown),
            "text" => Ok(Kind::Text),
            "svg" => Ok(Kind::Svg),
            "code" => Ok(Kind::Code),
            "files" => Ok(Kind::Files),
            other => Err(KindError::NotYet(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KindError {
    #[error(
        "kind {0:?} isn't available yet; the kinds that exist are html, markdown, text, svg, code and files"
    )]
    NotYet(String),
    #[error(
        "can't tell the kind of entry {0:?}; use html, markdown, text, svg or code, or send kind: \"files\" for a built bundle"
    )]
    Undetected(String),
}

impl KindError {
    /// The contract error code.
    pub fn code(&self) -> &'static str {
        "invalid_input"
    }
}

/// Detect the kind from the entry path. Never returns [`Kind::Files`].
pub fn detect(entry: &str) -> Result<Kind, KindError> {
    let base = entry.rsplit('/').next().unwrap_or(entry);
    let lower = base.to_ascii_lowercase();
    if lower == "index.html" || lower == "index.htm" {
        return Ok(Kind::Html);
    }
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "html" | "htm" => Ok(Kind::Html),
        "md" | "markdown" => Ok(Kind::Markdown),
        "txt" => Ok(Kind::Text),
        "svg" => Ok(Kind::Svg),
        "rs" | "py" | "go" | "js" | "mjs" | "cjs" | "css" | "json" | "toml" | "yaml" | "yml"
        | "sh" | "bash" | "zsh" | "c" | "h" | "cpp" | "hpp" | "java" | "sql" | "rb" | "php"
        | "xml" | "swift" | "kt" => Ok(Kind::Code),
        _ => Err(KindError::Undetected(entry.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_table() {
        let table: &[(&str, Result<Kind, ()>)] = &[
            ("index.html", Ok(Kind::Html)),
            ("INDEX.HTML", Ok(Kind::Html)),
            ("page.html", Ok(Kind::Html)),
            ("index.md", Ok(Kind::Markdown)),
            ("notes/plan.md", Ok(Kind::Markdown)),
            ("index.txt", Ok(Kind::Text)),
            ("list.txt", Ok(Kind::Text)),
            ("logo.svg", Ok(Kind::Svg)),
            ("App.tsx", Err(())),
            ("main.rs", Ok(Kind::Code)),
            ("noext", Err(())),
        ];
        for (entry, want) in table {
            let got = detect(entry).map_err(|_| ());
            assert_eq!(&got, want, "{entry}");
            assert_ne!(
                detect(entry).ok(),
                Some(Kind::Files),
                "files is never detected"
            );
        }
    }

    #[test]
    fn refusal_names_the_kinds_that_exist() {
        let e = detect("App.tsx").unwrap_err();
        assert_eq!(e.code(), "invalid_input");
        let msg = e.to_string();
        for k in ["html", "markdown", "text", "files"] {
            assert!(msg.contains(k), "{msg}");
        }
        let e = Kind::parse("tsx").unwrap_err().to_string();
        for k in AVAILABLE_KINDS {
            assert!(e.contains(k), "{e}");
        }
    }
}
