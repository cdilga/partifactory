//! The `part` CLI. Aliases construct registry inputs and call the same HTTPS operations.
use serde_json::{Value, json};
use std::{collections::BTreeMap, ffi::OsString, io::Read, path::Path};
mod login;
mod skill;
pub const EXIT_NOT_IMPLEMENTED: i32 = 64;
type Result<T> = std::result::Result<T, CliError>;
#[derive(Debug)]
struct CliError {
    body: Value,
    code: i32,
}
fn error(code: &str, message: impl Into<String>) -> CliError {
    CliError {
        body: json!({"error":{"code":code,"message":message.into(),"hint":pf_ops::catalogue::hint(code),"details":{}}}),
        code: pf_ops::catalogue::exit_code(code),
    }
}
struct Client {
    http: reqwest::blocking::Client,
    url: String,
    token: String,
}
impl Client {
    fn new() -> Result<Self> {
        let (url, token) = login::credentials()?;
        let http = login::http_client()?;
        Ok(Self { http, url, token })
    }

    fn post(&self, path: &str, body: Vec<u8>, json_body: bool) -> Result<Value> {
        for attempt in 0..2 {
            let response = self
                .http
                .post(format!("{}{path}", self.url))
                .bearer_auth(&self.token)
                .header("x-part-surface", "cli")
                .header(
                    "content-type",
                    if json_body {
                        "application/json"
                    } else {
                        "application/octet-stream"
                    },
                )
                .body(body.clone())
                .send();
            let response = match response {
                Ok(r) => r,
                Err(e) if attempt == 0 && (e.is_timeout() || e.is_connect()) => continue,
                Err(e) => return Err(error("internal", format!("request failed: {e}"))),
            };
            let status = response.status();
            let value = response.json::<Value>().map_err(|_| {
                error(
                    "internal",
                    format!("server returned {status} without a JSON response"),
                )
            })?;
            if !status.is_success() {
                let code = value["error"]["code"].as_str().unwrap_or("internal");
                return Err(CliError {
                    code: pf_ops::catalogue::exit_code(code),
                    body: value,
                });
            }
            return Ok(value);
        }
        Err(error("internal", "request retry exhausted"))
    }
    fn op(&self, op: &str, input: &Value) -> Result<Value> {
        self.post(
            &format!("/api/v1/{op}"),
            serde_json::to_vec(input).map_err(|e| error("invalid_input", e.to_string()))?,
            true,
        )
    }
}
fn keychain(url: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let r = std::process::Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                "partifactory",
                "-a",
                url,
                "-w",
            ])
            .output()
            .ok()?;
        if r.status.success() {
            return String::from_utf8(r.stdout).ok().map(|s| s.trim().into());
        }
    }
    let _ = url;
    None
}
fn json_input(arg: &str) -> Result<Value> {
    let mut bytes = Vec::new();
    if arg == "-" {
        std::io::stdin()
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| error("invalid_input", e.to_string()))?;
    } else if let Some(p) = arg.strip_prefix('@') {
        bytes = std::fs::read(p).map_err(|e| error("invalid_input", e.to_string()))?;
    } else {
        return Err(error("invalid_input", "--json input is @file or -"));
    }
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(error("too_large", "JSON exceeds 8 MiB"));
    }
    serde_json::from_slice(&bytes).map_err(|e| error("invalid_input", e.to_string()))
}
fn help(op: Option<&str>) -> Result<Value> {
    if op == Some("login") {
        return Ok(
            json!({"usage":"part login [--url origin] [--agent-name name] [--scope scopes] [--no-browser]","description":"Approve this CLI in your browser; credentials are stored in a mode-600 config and refreshed automatically."}),
        );
    }
    if let Some(op) = op {
        let op = canonical(op);
        let d = pf_ops::definitions()
            .into_iter()
            .find(|d| d.name == op)
            .ok_or_else(|| error("not_found", "unknown operation"))?;
        return Ok(serde_json::to_value(d).expect("definition serializes"));
    }
    Ok(
        json!({"usage":"part login [--url origin] | part <op> --json @file|- | part put <file-or-dir> --name <name> | part get <name> | part edit <name> --replace <old> <new>","ops":pf_ops::definitions().iter().map(|d|d.name).collect::<Vec<_>>()}),
    )
}
fn canonical(op: &str) -> &str {
    match op {
        "create" => "part_create",
        "edit" => "part_edit",
        "get" => "part_get",
        "check" => "part_check",
        "data" => "part_data",
        "link-for" => "part_link_for",
        _ => op,
    }
}
fn collect_files(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
        let meta =
            std::fs::symlink_metadata(path).map_err(|e| error("invalid_input", e.to_string()))?;
        if meta.file_type().is_symlink() {
            return Err(error("invalid_input", "put refuses symlinks"));
        }
        if meta.is_dir() {
            for entry in
                std::fs::read_dir(path).map_err(|e| error("invalid_input", e.to_string()))?
            {
                walk(
                    root,
                    &entry
                        .map_err(|e| error("invalid_input", e.to_string()))?
                        .path(),
                    out,
                )?;
            }
        } else if meta.is_file() {
            let name = path
                .strip_prefix(root)
                .map_err(|e| error("invalid_input", e.to_string()))?
                .to_str()
                .ok_or_else(|| error("invalid_input", "file names must be UTF-8"))?
                .replace('\\', "/");
            pf_model::manifest::check_path(&name).map_err(|e| error(e.code, e.message))?;
            if meta.len() > 2 * 1024 * 1024 {
                return Err(error("too_large", "source file exceeds 2 MiB"));
            }
            out.insert(
                name,
                std::fs::read(path).map_err(|e| error("invalid_input", e.to_string()))?,
            );
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    let root = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or_else(|| Path::new("."))
    };
    walk(root, path, &mut out)?;
    if out.is_empty() {
        return Err(error("invalid_input", "put needs at least one file"));
    }
    pf_model::manifest::check_paths(out.keys().map(String::as_str))
        .map_err(|e| error(e.code, e.message))?;
    Ok(out)
}
fn put(client: &Client, path: &str, mut input: Value, rid: &str) -> Result<Value> {
    let files = collect_files(Path::new(path))?;
    let hashes: BTreeMap<String, String> = files
        .iter()
        .map(|(p, b)| (p.clone(), pf_blobs::sha256_hex(b)))
        .collect();
    let response = client.post(
        "/api/v1/blobs/missing",
        serde_json::to_vec(&json!({"hashes":hashes.values().collect::<Vec<_>>()})).unwrap(),
        true,
    )?;
    let missing = response["missing"]
        .as_array()
        .ok_or_else(|| error("internal", "invalid missing-blob response"))?;
    for (path, hash) in &hashes {
        if missing.contains(&json!(hash)) {
            client.post("/api/v1/blobs", files[path].clone(), false)?;
        }
    }
    let name = input["name"].as_str().map(str::to_string);
    if let Some(name) = &name {
        match client.op("part_get", &json!({"name":name})) {
            Ok(old) => {
                let old_files = old["files"]
                    .as_object()
                    .ok_or_else(|| error("internal", "invalid file list"))?;
                let mut edits = Vec::new();
                for (path, hash) in &hashes {
                    if old_files
                        .get(path)
                        .is_none_or(|f| f["hash"].as_str() != Some(hash))
                    {
                        edits.push(json!({"op":"write","file":path,"blob":hash,"if_match":old_files.get(path).map(|f|f["hash"].clone()).unwrap_or(Value::Null)}));
                    }
                }
                for (path, file) in old_files {
                    if !hashes.contains_key(path) {
                        edits.push(json!({"op":"delete","file":path,"if_match":file["hash"]}));
                    }
                }
                if edits.is_empty() {
                    return Ok(
                        json!({"name":name,"url":old["url"],"head":old["head"],"live":old["live"],"build":old["build"],"unchanged":true}),
                    );
                }
                return client.op(
                    "part_edit",
                    &json!({"name":name,"request_id":rid,"edits":edits}),
                );
            }
            Err(e) if e.body["error"]["code"] == "not_found" => {}
            Err(e) => return Err(e),
        }
    }
    input["files"] = json!(
        hashes
            .iter()
            .map(|(p, h)| (p.clone(), json!({"blob":h})))
            .collect::<BTreeMap<_, _>>()
    );
    input["request_id"] = json!(rid);
    if input.get("title").is_none() {
        input["title"] = json!(name.unwrap_or_else(|| {
            Path::new(path)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into()
        }));
    }
    client.op("part_create", &input)
}
fn execute(args: &[String]) -> Result<Value> {
    let Some(op) = args.first() else {
        return help(None);
    };
    if ["--help", "-h", "help"].contains(&op.as_str()) {
        return help(args.get(1).map(String::as_str));
    }
    if ["--version", "-V"].contains(&op.as_str()) {
        return Ok(json!({"version":env!("CARGO_PKG_VERSION")}));
    }
    if op == "login" {
        if args[1..]
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
        {
            return help(Some("login"));
        }
        return login::run(&args[1..]);
    }
    if op == "skill" {
        return skill::run(&args[1..]);
    }
    let mut input = json!({});
    let mut positionals = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--json" {
            if let Some(next) = args
                .get(i + 1)
                .filter(|s| s.starts_with('@') || s.as_str() == "-")
            {
                input = json_input(next)?;
                i += 1;
            }
        } else if arg == "--replace" {
            let old = args
                .get(i + 1)
                .ok_or_else(|| error("invalid_input", "--replace needs old and new strings"))?;
            let new = args
                .get(i + 2)
                .ok_or_else(|| error("invalid_input", "--replace needs old and new strings"))?;
            input["edits"] = json!([{"op":"replace","old_str":old,"new_str":new}]);
            i += 2;
        } else if let Some(flag) = arg.strip_prefix("--") {
            let (key, value) = if let Some((k, v)) = flag.split_once('=') {
                (k, v.to_string())
            } else if let Some(next) = args.get(i + 1).filter(|s| !s.starts_with("--")) {
                i += 1;
                (flag, next.clone())
            } else {
                (flag, "true".into())
            };
            let key = key.replace('-', "_");
            let value = serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value));
            input[&key] = value;
        } else {
            positionals.push(arg.clone());
        }
        i += 1;
    }
    if !input.is_object() {
        return Err(error("invalid_input", "op input must be an object"));
    }
    let rid = input["request_id"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if op == "put" {
        let path = positionals
            .first()
            .ok_or_else(|| error("invalid_input", "put needs a file or directory"))?;
        return put(&Client::new()?, path, input, &rid);
    }
    let op = canonical(op);
    if !pf_ops::definitions().iter().any(|d| d.name == op) {
        return Err(error("invalid_input", "unknown operation; run part help"));
    }
    if let Some(name) = positionals.first() {
        input["name"] = json!(name);
    }
    if matches!(op, "part_create" | "part_edit")
        || op == "part_data"
            && matches!(
                input["action"].as_str(),
                Some("set" | "delete" | "reset" | "snapshot_restore")
            )
    {
        input["request_id"] = json!(rid);
    }
    Client::new()?.op(op, &input)
}
pub fn run(args: Vec<OsString>) -> i32 {
    let args: Vec<String> = args
        .into_iter()
        .skip(1)
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    match execute(&args) {
        Ok(value) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).expect("JSON value")
            );
            0
        }
        Err(e) => {
            eprintln!("{}", serde_json::to_string(&e.body).expect("JSON value"));
            e.code
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn put_collects_files_and_rejects_links() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("index.html"), "hi").unwrap();
        std::fs::write(dir.path().join("assets/x.txt"), "x").unwrap();
        assert_eq!(collect_files(dir.path()).ok().unwrap().len(), 2);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("index.html", dir.path().join("link")).unwrap();
            assert!(collect_files(dir.path()).is_err());
        }
    }
    #[test]
    fn error_exit_codes_follow_contract() {
        assert_eq!(pf_ops::catalogue::exit_code("forbidden"), 3);
        assert_eq!(pf_ops::catalogue::exit_code("not_found"), 4);
        assert_eq!(pf_ops::catalogue::exit_code("anchor_not_found"), 5);
        assert_eq!(pf_ops::catalogue::exit_code("internal"), 7);
    }
}
