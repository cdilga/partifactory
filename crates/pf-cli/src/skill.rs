use crate::{Result, error, login};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn install(path: &Path, body: &str, codex: bool) -> Result<()> {
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(error(
            "invalid_input",
            "Skill installation refuses symlink destinations",
        ));
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| error("invalid_input", e.to_string()))?;
    }
    let content = if codex {
        let old = match std::fs::read_to_string(path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(error("invalid_input", e.to_string())),
        };
        if let Some(start) = old.find("<!-- partifactory -->") {
            let end = old[start..]
                .find("<!-- /partifactory -->")
                .map(|n| start + n + "<!-- /partifactory -->".len())
                .ok_or_else(|| {
                    error(
                        "invalid_input",
                        "Existing partifactory snippet has no closing marker",
                    )
                })?;
            format!("{}{}{}", &old[..start], body.trim_end(), &old[end..])
        } else {
            format!(
                "{}{}{}",
                old,
                if old.is_empty() || old.ends_with('\n') {
                    "\n"
                } else {
                    "\n\n"
                },
                body
            )
        }
    } else {
        body.to_string()
    };
    std::fs::write(path, content).map_err(|e| error("invalid_input", e.to_string()))
}
pub(super) fn run(args: &[String]) -> Result<Value> {
    if args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "--help" | "-h"))
    {
        return Ok(
            json!({"usage":"part skill install [--claude | --codex | --path file] [--url origin]","description":"Fetch the public contract and install a Claude skill or the Codex AGENTS.md snippet. No token is required."}),
        );
    }
    if args.first().map(String::as_str) != Some("install") {
        return Err(error(
            "invalid_input",
            "Use part skill install [--claude | --codex | --path file]",
        ));
    }
    let mut mode = None;
    let mut path = None;
    let mut url = std::env::var("PARTIFACTORY_URL").ok();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--claude" | "--codex" => {
                if mode.is_some() {
                    return Err(error("invalid_input", "Choose one installation target"));
                }
                mode = Some(args[i].clone());
            }
            "--path" => {
                if mode.is_some() {
                    return Err(error("invalid_input", "Choose one installation target"));
                }
                i += 1;
                path =
                    Some(PathBuf::from(args.get(i).ok_or_else(|| {
                        error("invalid_input", "--path needs a file")
                    })?));
                mode = Some("--path".into());
            }
            "--url" => {
                i += 1;
                url = Some(
                    args.get(i)
                        .ok_or_else(|| error("invalid_input", "--url needs an origin"))?
                        .clone(),
                );
            }
            _ => {
                return Err(error(
                    "unknown_field",
                    format!("Unknown skill install argument {}", args[i]),
                ));
            }
        }
        i += 1;
    }
    let url = login::validated_url(
        &url.ok_or_else(|| error("invalid_input", "Set PARTIFACTORY_URL or pass --url"))?,
    )?;
    let codex = mode.as_deref() == Some("--codex");
    let path = match path {
        Some(path) => {
            if path.is_dir() {
                path.join("SKILL.md")
            } else {
                path
            }
        }
        None if codex => PathBuf::from("AGENTS.md"),
        None => PathBuf::from(
            std::env::var("HOME")
                .map_err(|_| error("invalid_input", "HOME is missing; use --path"))?,
        )
        .join(".claude/skills/partifactory/SKILL.md"),
    };
    let response = login::http_client()?
        .get(format!(
            "{url}/{}",
            if codex { "agents.md" } else { "skill.md" }
        ))
        .send()
        .map_err(|e| error("internal", format!("Could not fetch skill: {e}")))?;
    if !response.status().is_success() {
        return Err(error(
            "not_found",
            format!("Skill discovery returned {}", response.status()),
        ));
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    response
        .take(1_000_001)
        .read_to_end(&mut bytes)
        .map_err(|e| error("internal", e.to_string()))?;
    if bytes.len() > 1_000_000 {
        return Err(error("too_large", "Skill exceeds 1 MB"));
    }
    let body =
        String::from_utf8(bytes).map_err(|_| error("invalid_input", "Skill must be UTF-8"))?;
    if !body.contains(if codex {
        "<!-- partifactory -->"
    } else {
        "name: partifactory"
    }) {
        return Err(error(
            "invalid_input",
            "The response is not a partifactory skill",
        ));
    }
    install(&path, &body, codex)?;
    Ok(json!({"installed":path,"source":url,"kind":if codex{"agents"}else{"skill"}}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_install_preserves_other_instructions_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "Existing project instructions.\n").unwrap();
        let body = "<!-- partifactory -->\nRead https://parts.example.com/skill.md\n<!-- /partifactory -->\n";
        install(&path, body, true).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        install(&path, body, true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        assert!(first.starts_with("Existing project instructions."));
    }
}
