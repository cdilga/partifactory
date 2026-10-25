use crate::{PartHost, headers, template};
use axum::{
    body::Body,
    extract::Request,
    response::{IntoResponse, Response},
};
use http::{HeaderValue, StatusCode, header};
use pf_caps::{CapError, Claims};

pub(crate) fn error(e: pf_model::PfError) -> Response {
    (
        StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        axum::Json(e.to_json()),
    )
        .into_response()
}

pub(crate) fn authorize(s: &PartHost, req: &Request) -> Result<(String, Claims), Box<Response>> {
    let name = req
        .extensions()
        .get::<pf_serve::PartHost>()
        .map(|h| h.0.as_str())
        .unwrap_or("");
    let cap = req
        .uri()
        .path()
        .strip_prefix("/c/")
        .and_then(|p| p.split('/').next())
        .unwrap_or("");
    match pf_caps::verify(&s.deps().keys, &s.deps().epochs, cap, name, s.now()) {
        Ok(c) => {
            if let Some(id) = c.s.strip_prefix("link:") {
                let id = id.to_owned();
                let now = s.now();
                let live=s.deps().db.read_blocking(move |db| {
                    db.query_row("SELECT EXISTS(SELECT 1 FROM public_links l JOIN parts p ON p.id=l.part_id WHERE l.id=?1 AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at>?2) AND p.deleted_at IS NULL)",pf_db::rusqlite::params![id,now],|r|r.get::<_,bool>(0)).map_err(pf_db::DbError::from)
                }).unwrap_or(false);
                if !live {
                    return Err(Box::new(StatusCode::NOT_FOUND.into_response()));
                }
                if let Some(r) = pf_links::web::limit_link(req, c.s.as_str()) {
                    return Err(Box::new(r));
                }
            }
            tracing::info!(event = "cap.verify", outcome = "ok");
            Ok((cap.into(), c))
        }
        Err(e) => {
            let outcome = match e {
                CapError::Expired => "expired",
                CapError::WrongHost => "wrong_host",
                CapError::StalePartEpoch | CapError::StaleMinterEpoch | CapError::StaleBoot => {
                    "stale_epoch"
                }
                _ => "bad_mac",
            };
            tracing::info!(event = "cap.verify", outcome);
            // A valid signature identifies a link even when the live epoch has moved on.
            // Revoked/frozen public capabilities never lead to the signed-in grant flow.
            if e.is_reauth()
                && pf_caps::verify(
                    &s.deps().keys,
                    &pf_caps::EpochStore::default(),
                    cap,
                    name,
                    s.now(),
                )
                .is_ok_and(|c| c.s.starts_with("link:"))
            {
                return Err(Box::new(StatusCode::NOT_FOUND.into_response()));
            }
            if e.is_reauth() {
                let navigation = req.method() == http::Method::GET
                    && req
                        .headers()
                        .get("sec-fetch-dest")
                        .and_then(|v| v.to_str().ok())
                        .map(|dest| matches!(dest, "document" | "iframe"))
                        .unwrap_or_else(|| {
                            req.headers()
                                .get(header::ACCEPT)
                                .and_then(|v| v.to_str().ok())
                                .is_some_and(|v| v.contains("text/html"))
                        });
                if navigation {
                    let path = req
                        .uri()
                        .path()
                        .strip_prefix(&format!("/c/{cap}"))
                        .unwrap_or("/");
                    let path = format!(
                        "{path}{}",
                        req.uri()
                            .query()
                            .map(|q| format!("?{q}"))
                            .unwrap_or_default()
                    );
                    let loc = format!(
                        "{}/grant?part={}&mode=direct&path={}",
                        s.config().workspace_origin(),
                        name,
                        percent_encoding::utf8_percent_encode(
                            &path,
                            percent_encoding::NON_ALPHANUMERIC
                        )
                    );
                    return Err(Box::new(
                        (StatusCode::FOUND, [(header::LOCATION, loc)]).into_response(),
                    ));
                }
                return Err(Box::new(
                    (StatusCode::UNAUTHORIZED, [("x-pf-reauth", "1")]).into_response(),
                ));
            }
            Err(Box::new(StatusCode::NOT_FOUND.into_response()))
        }
    }
}

pub async fn bare_cap(s: PartHost, req: Request) -> Response {
    if let Err(r) = authorize(&s, &req) {
        return *r;
    }
    let mut loc = format!("{}/", req.uri().path());
    if let Some(q) = req.uri().query() {
        loc.push('?');
        loc.push_str(q);
    }
    (StatusCode::PERMANENT_REDIRECT, [(header::LOCATION, loc)]).into_response()
}

pub async fn serve(s: PartHost, req: Request) -> Response {
    let (cap, c) = match authorize(&s, &req) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let guest_path = req
        .uri()
        .path()
        .strip_prefix(&format!("/c/{cap}/"))
        .unwrap_or("");
    let guest_path = percent_encoding::percent_decode_str(guest_path).decode_utf8_lossy();
    if c.s.starts_with("link:") && matches!(guest_path.split('/').next(), Some("api" | "mcp")) {
        tracing::info!(event="rt.op",principal=%c.s,outcome="forbidden");
        return error(pf_model::PfError::new(
            "forbidden",
            "guest capabilities only access shared data",
        ));
    }
    if req.method() != http::Method::GET && req.method() != http::Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let live = match s.live(&c.h, c.v.as_deref()).await {
        Ok(Some(l)) if l.part_id == c.p => l,
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return error(e),
    };
    let raw = req
        .uri()
        .path()
        .strip_prefix(&format!("/c/{cap}/"))
        .unwrap_or("");
    let decoded = match percent_encoding::percent_decode_str(raw).decode_utf8() {
        Ok(p) => p,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    if decoded.split('/').any(|p| p == ".." || p == ".") || decoded.contains(['\\', '\0']) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = if decoded.is_empty() {
        live.entry.as_str()
    } else {
        decoded.as_ref()
    };
    let Some(file) = live.files.get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let html = file.content_type.split(';').next() == Some("text/html");
    let doc = html || file.content_type.starts_with("image/svg+xml");
    let br = req
        .headers()
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',').any(|e| {
                let mut p = e.trim().split(';');
                p.next() == Some("br") && !p.any(|q| q.trim() == "q=0")
            })
        });
    let boot = if html {
        match template::boot(&s, &cap, &c).await {
            Ok(b) => b,
            Err(e) => return error(e),
        }
    } else {
        std::sync::Arc::new(String::new())
    };
    let etag = if html {
        format!(
            "\"{}-{}-{}\"",
            file.hash,
            s.prelude_hash(),
            pf_blobs::sha256_hex(format!("{cap}:{boot}:{br}").as_bytes())
        )
    } else {
        format!(
            "\"{}{}\"",
            file.hash,
            if br && file.br_hash.is_some() {
                "-br"
            } else {
                ""
            }
        )
    };
    let not_modified = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|v| v.trim() == etag || v.trim() == "*"));
    let mut response = Response::new(Body::empty());
    if not_modified {
        *response.status_mut() = StatusCode::NOT_MODIFIED;
    } else {
        let hash = if !html && br {
            file.br_hash.as_deref().unwrap_or(&file.hash)
        } else {
            &file.hash
        };
        let mut bytes = match s.bytes(hash) {
            Ok(b) => b,
            Err(e) => return error(e),
        };
        let mut compressed = !html && br && file.br_hash.is_some();
        if html {
            let src = match std::str::from_utf8(&bytes) {
                Ok(v) => v,
                Err(_) => return error(pf_model::PfError::internal("HTML is not UTF-8")),
            };
            bytes = bytes::Bytes::from(template::html(&s, src, &cap, &boot));
            tracing::info!(event="serve.entry",part_id=%c.p,outcome="ok");
            if br {
                let cached = s
                    .inner
                    .br
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&etag);
                if let Some(b) = cached {
                    bytes = b;
                } else {
                    let mut out = Vec::new();
                    if let Err(e) = std::io::copy(
                        &mut bytes.as_ref(),
                        &mut brotli::CompressorWriter::new(&mut out, 4096, 4, 22),
                    ) {
                        return error(pf_model::PfError::internal(e.to_string()));
                    }
                    bytes = bytes::Bytes::from(out);
                    s.inner
                        .br
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .put(etag.clone(), bytes.clone());
                }
                compressed = true;
            }
        }
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
        if req.method() != http::Method::HEAD {
            *response.body_mut() = Body::from(bytes)
        }
        if compressed {
            response
                .headers_mut()
                .insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
        }
    }
    let h = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&file.content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, immutable"),
    );
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if doc
        && let Ok(v) =
            HeaderValue::from_str(&headers::canonical_csp(s.config(), &c.h, live.unsandboxed))
    {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    if req
        .uri()
        .query()
        .is_some_and(|q| q.split('&').any(|p| p == "dl=1"))
    {
        h.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment"),
        );
    }
    response
}
