//! Device login and a mode-600, atomically replaced refresh bundle. The file lock covers the
//! reread, one-use refresh exchange and durable write, so concurrent CLI invocations cannot reuse.
use super::{CliError, Result, error};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub(super) fn config_path() -> Result<PathBuf> {
    std::env::var_os("PARTIFACTORY_CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(|p| PathBuf::from(p).join("partifactory/config.json"))
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|p| PathBuf::from(p).join(".config/partifactory/config.json"))
        })
        .ok_or_else(|| {
            error(
                "invalid_input",
                "Set PARTIFACTORY_CONFIG or HOME for credential storage.",
            )
        })
}
fn private_file(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| error("invalid_input", "Cannot read credential file metadata."))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(error(
            "invalid_input",
            "Credential files must be regular files, not symlinks.",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o777 != 0o600 {
            return Err(error(
                "invalid_input",
                "credential config must have mode 600",
            ));
        }
    }
    Ok(())
}
pub(super) fn read_config(path: &Path) -> Result<Value> {
    if std::fs::symlink_metadata(path).is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(json!({}));
    }
    private_file(path)?;
    let mut data = Vec::new();
    File::open(path)
        .map_err(|_| error("invalid_input", "Cannot open credential config."))?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut data)
        .map_err(|_| error("invalid_input", "Cannot read credential config."))?;
    if data.len() > 1024 * 1024 {
        return Err(error("invalid_input", "Credential config is too large."));
    }
    let value: Value = serde_json::from_slice(&data)
        .map_err(|_| error("invalid_input", "Credential config is not valid JSON."))?;
    if !value.is_object() {
        return Err(error(
            "invalid_input",
            "Credential config must be a JSON object.",
        ));
    }
    Ok(value)
}
fn prepare_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(parent)
            .map_err(|_| error("invalid_input", "Cannot create credential directory."))?;
    }
    Ok(())
}
fn lock(path: &Path) -> Result<File> {
    prepare_parent(path)?;
    let lock_path = path.with_extension("lock");
    if std::fs::symlink_metadata(&lock_path).is_ok() {
        private_file(&lock_path)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&lock_path)
        .map_err(|_| error("internal", "Cannot open credential lock."))?;
    private_file(&lock_path)?;
    fs4::fs_std::FileExt::lock_exclusive(&file)
        .map_err(|_| error("internal", "Cannot lock credential refresh."))?;
    Ok(file)
}
fn write_config(path: &Path, value: &Value) -> Result<()> {
    prepare_parent(path)?;
    if std::fs::symlink_metadata(path).is_ok() {
        private_file(path)?;
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temp = parent.join(format!(".partifactory-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options
            .open(&temp)
            .map_err(|_| error("internal", "Cannot create credential file."))?;
        let encoded = serde_json::to_vec_pretty(value)
            .map_err(|_| error("internal", "Cannot encode credentials."))?;
        file.write_all(&encoded)
            .and_then(|_| file.sync_all())
            .map_err(|_| error("internal", "Cannot persist credentials."))?;
        std::fs::rename(&temp, path)
            .map_err(|_| error("internal", "Cannot replace credential file."))?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|d| d.sync_all())
            .map_err(|_| error("internal", "Cannot sync credential directory."))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}
fn validated_url(value: &str) -> Result<String> {
    let url =
        reqwest::Url::parse(value).map_err(|_| error("invalid_input", "Invalid server URL."))?;
    if (url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(error(
            "invalid_input",
            "Use the HTTPS workspace origin, without credentials, a path, query or fragment.",
        ));
    }
    Ok(url.origin().ascii_serialization())
}
pub(super) fn http_client() -> Result<reqwest::blocking::Client> {
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::none());
    if let Ok(path) = std::env::var("PARTIFACTORY_CA_CERT") {
        let pem = std::fs::read(path)
            .map_err(|_| error("invalid_input", "Cannot read PARTIFACTORY_CA_CERT."))?;
        for cert in reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|_| error("invalid_input", "Invalid PARTIFACTORY_CA_CERT PEM bundle."))?
        {
            builder = builder.add_root_certificate(cert);
        }
    }
    builder
        .build()
        .map_err(|_| error("internal", "Cannot initialize the HTTPS client."))
}
fn oauth_request(
    http: &reqwest::blocking::Client,
    url: &str,
    fields: &[(&str, &str)],
) -> Result<(bool, Value)> {
    let response = http
        .post(url)
        .form(fields)
        .send()
        .map_err(|_| error("internal", "The authorization server could not be reached."))?;
    let success = response.status().is_success();
    let value = response.json::<Value>().map_err(|_| {
        error(
            "internal",
            "The authorization server returned an invalid response.",
        )
    })?;
    Ok((success, value))
}
fn oauth_failure(value: &Value) -> CliError {
    let message = match value["error"].as_str() {
        Some("access_denied") => "The agent connection was denied.",
        Some("expired_token") => "The device code expired; run part login again.",
        Some("invalid_grant") => {
            "The saved connection expired or was revoked; run part login again."
        }
        Some("invalid_scope") => "The requested scopes are not supported.",
        _ => "Authorization failed; run part login again.",
    };
    error("unauthenticated", message)
}
fn bundle(url: &str, response: &Value) -> Result<Value> {
    let token = response["access_token"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| error("internal", "The token response had no access token."))?;
    let refresh = response["refresh_token"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            error(
                "internal",
                "The token response had no rotated refresh token.",
            )
        })?;
    let lifetime = response["expires_in"]
        .as_i64()
        .filter(|v| *v > 0 && *v <= 3600)
        .ok_or_else(|| error("internal", "Invalid access-token lifetime."))?;
    Ok(json!({"url":url,"token":token,"refresh_token":refresh,"expires_at":now()+lifetime*1000}))
}

/// Selection priority remains environment, macOS keychain, then protected config. Only a config
/// bundle with a refresh token is rotated; static personal/environment credentials are untouched.
pub(super) fn credentials() -> Result<(String, String)> {
    let env_url = std::env::var("PARTIFACTORY_URL").ok();
    let env_token = std::env::var("PARTIFACTORY_TOKEN").ok();
    let cfg = if env_url.is_some() && env_token.is_some() {
        json!({})
    } else {
        read_config(&config_path()?)?
    };
    let url = validated_url(
        env_url
            .or_else(|| cfg["url"].as_str().map(str::to_owned))
            .as_deref()
            .ok_or_else(|| {
                error(
                    "invalid_input",
                    "Set PARTIFACTORY_URL or run part login --url <workspace-origin>.",
                )
            })?,
    )?;
    if let Some(token) = env_token {
        return Ok((url, token));
    }
    if let Some(token) = super::keychain(&url) {
        return Ok((url, token));
    }
    // Config credentials are always tied to the exact server that issued them.
    if cfg["url"]
        .as_str()
        .is_some_and(|saved| saved.trim_end_matches('/') != url)
    {
        return Err(error(
            "unauthenticated",
            "The saved credentials belong to a different workspace; run part login.",
        ));
    }
    let token = cfg["token"].as_str().ok_or_else(|| {
        error(
            "unauthenticated",
            "Run part login or set PARTIFACTORY_TOKEN.",
        )
    })?;
    if cfg["refresh_token"].as_str().is_none()
        || cfg["expires_at"]
            .as_i64()
            .is_some_and(|expires| expires > now() + 30_000)
    {
        return Ok((url, token.to_owned()));
    }
    let http = http_client()?;
    let token = refresh_locked(&config_path()?, &url, &http)?;
    Ok((url, token))
}
fn refresh_locked(path: &Path, url: &str, http: &reqwest::blocking::Client) -> Result<String> {
    let _lock = lock(path)?;
    let current = read_config(path)?;
    if current["url"]
        .as_str()
        .is_none_or(|saved| saved.trim_end_matches('/') != url)
    {
        return Err(error(
            "unauthenticated",
            "The credential workspace changed; run part login again.",
        ));
    }
    if current["expires_at"]
        .as_i64()
        .is_some_and(|expires| expires > now() + 30_000)
    {
        return current["token"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| error("unauthenticated", "Run part login again."));
    }
    let refresh = current["refresh_token"]
        .as_str()
        .ok_or_else(|| error("unauthenticated", "Run part login again."))?;
    // Refresh is deliberately never automatically retried after a lost response: it is single-use.
    let resource = format!("{url}/");
    let (success, value) = oauth_request(
        http,
        &format!("{url}/oauth/token"),
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("resource", &resource),
        ],
    )?;
    if !success {
        return Err(oauth_failure(&value));
    }
    let next = bundle(url, &value)?;
    write_config(path, &next)?;
    Ok(next["token"].as_str().expect("validated token").to_owned())
}

pub(super) fn run(args: &[String]) -> Result<Value> {
    let mut url = None;
    let mut agent = "part CLI".to_owned();
    let mut scope = "parts:read parts:write parts:delete data:read data:write".to_owned();
    let mut no_browser = false;
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        if flag == "--no-browser" {
            no_browser = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| error("invalid_input", format!("{flag} needs a value")))?;
        match flag.as_str() {
            "--url" => url = Some(value.clone()),
            "--agent-name" => agent = value.clone(),
            "--scope" => scope = value.clone(),
            _ => {
                return Err(error(
                    "invalid_input",
                    "Use part login [--url origin] [--agent-name name] [--scope scopes] [--no-browser].",
                ));
            }
        }
        index += 2;
    }
    let path = config_path()?;
    let cfg = read_config(&path)?;
    let url = validated_url(
        url.or_else(|| std::env::var("PARTIFACTORY_URL").ok())
            .or_else(|| cfg["url"].as_str().map(str::to_owned))
            .as_deref()
            .ok_or_else(|| error("invalid_input", "Use part login --url <workspace-origin>."))?,
    )?;
    if std::env::var_os("PARTIFACTORY_TOKEN").is_some() {
        eprintln!("PARTIFACTORY_TOKEN continues to take priority over saved login credentials.");
    } else if super::keychain(&url).is_some() {
        eprintln!(
            "A macOS keychain token continues to take priority over saved login credentials."
        );
    }
    let http = http_client()?;
    let resource = format!("{url}/");
    let software = format!("part/{}", env!("CARGO_PKG_VERSION"));
    let (success, device) = oauth_request(
        &http,
        &format!("{url}/oauth/device_authorization"),
        &[
            ("client_id", "part-cli"),
            ("agent_name", &agent),
            ("scope", &scope),
            ("software", &software),
            ("resource", &resource),
        ],
    )?;
    if !success {
        return Err(oauth_failure(&device));
    }
    let device_code = device["device_code"]
        .as_str()
        .ok_or_else(|| error("internal", "The server returned no device code."))?;
    let verification = device["verification_uri_complete"]
        .as_str()
        .ok_or_else(|| error("internal", "The server returned no approval URL."))?;
    let verify_url = reqwest::Url::parse(verification)
        .map_err(|_| error("internal", "Invalid approval URL."))?;
    let origin = reqwest::Url::parse(&url).expect("validated origin");
    if verify_url.origin() != origin.origin() || verify_url.path() != "/device" {
        return Err(error(
            "internal",
            "The server returned an approval URL outside this workspace.",
        ));
    }
    eprintln!("Approve this agent in your browser:\n{verification}");
    if let Ok(qr) = qrcode::QrCode::new(verification) {
        eprintln!(
            "{}",
            qr.render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build()
        );
    }
    if !no_browser {
        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(not(target_os = "macos"))]
        let opener = "xdg-open";
        let _ = std::process::Command::new(opener)
            .arg(verification)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    let mut interval = device["interval"].as_u64().unwrap_or(5).clamp(5, 60);
    let deadline = std::time::Instant::now()
        + Duration::from_secs(device["expires_in"].as_u64().unwrap_or(600).min(600));
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(interval));
        let (success, response) = oauth_request(
            &http,
            &format!("{url}/oauth/token"),
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", "part-cli"),
                ("device_code", device_code),
                ("resource", &resource),
            ],
        )?;
        if success {
            let next = bundle(&url, &response)?;
            let _lock = lock(&path)?;
            write_config(&path, &next)?;
            return Ok(json!({"signed_in":true,"url":url,"agent_name":agent}));
        }
        match response["error"].as_str() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = (interval + 5).min(60),
            _ => return Err(oauth_failure(&response)),
        }
    }
    Err(error(
        "unauthenticated",
        "The device code expired; run part login again.",
    ))
}

#[cfg(test)]
mod tests;
