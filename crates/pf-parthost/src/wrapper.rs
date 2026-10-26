use crate::{PartHost, content::error};
use axum::{
    body::Body,
    extract::Request,
    response::{IntoResponse, Response},
};
use http::{StatusCode, header};
use pf_model::PfError;

pub async fn seed(s: &PartHost) -> Result<(), PfError> {
    let mut seeds = Vec::new();
    let css = pf_ui::wrapper_css();
    for (label, src, typ) in [
        (
            pf_prelude::PRELUDE_LABEL,
            pf_prelude::PRELUDE_JS.as_bytes(),
            "text/javascript; charset=utf-8",
        ),
        (
            pf_prelude::WRAPPER_LABEL,
            pf_prelude::WRAPPER_JS.as_bytes(),
            "text/javascript; charset=utf-8",
        ),
        ("pf-wrapper.css", css.as_bytes(), "text/css; charset=utf-8"),
        (
            pf_prelude::EDITOR_LABEL,
            pf_prelude::EDITOR_JS.as_bytes(),
            "text/javascript; charset=utf-8",
        ),
        ("pf-wrapper-mono.woff2", pf_ui::MONO_FONT, "font/woff2"),
    ] {
        let put = s.deps().blobs.put(src)?;
        seeds.push((label.to_string(), put.hash, put.size as i64, typ.to_owned()));
    }
    let now = s.now();
    let rows = seeds.clone();
    let (shared,css)=s.write(move |db|{
        for (label,hash,size,typ) in rows {
            db.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",pf_db::rusqlite::params![hash,size,now])?;
            db.execute("INSERT INTO shared_blobs(hash,source,label,added_at,content_type) VALUES(?1,'seed',?2,?3,?4) ON CONFLICT(hash) DO UPDATE SET content_type=excluded.content_type",pf_db::rusqlite::params![hash,label,now,typ])?;
        }
        let mut st=db.prepare("SELECT s.hash,s.label,s.content_type FROM shared_blobs s ORDER BY s.added_at DESC")?;
        let rows=st.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
        let css=rows.iter().find(|(_,l,_)|l=="pf-shell.css").map(|(h,_,_)|h.clone());
        Ok((rows.into_iter().map(|(h,l,t)|{let t=if !t.is_empty(){t}else if l.ends_with(".js"){"text/javascript; charset=utf-8".into()}else if l.ends_with(".css"){"text/css; charset=utf-8".into()}else{"application/octet-stream".into()};(h,t)}).collect(),css))
    }).await?;
    *s.inner.shared.write().unwrap_or_else(|e| e.into_inner()) = shared;
    *s.inner
        .shell_css_hash
        .write()
        .unwrap_or_else(|e| e.into_inner()) = css;
    *s.inner
        .prelude_hash
        .write()
        .unwrap_or_else(|e| e.into_inner()) = seeds[0].1.clone();
    *s.inner
        .wrapper_hash
        .write()
        .unwrap_or_else(|e| e.into_inner()) = seeds[1].1.clone();
    Ok(())
}

pub async fn blob(s: PartHost, req: Request) -> Response {
    let hash = req.uri().path().strip_prefix("/b/").unwrap_or("");
    let Some(typ) = s
        .inner
        .shared
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(hash)
        .cloned()
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut b = match s.bytes(hash) {
        Ok(b) => b,
        Err(e) => return error(e),
    };
    let accepts_br = req
        .headers()
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',').any(|e| {
                let mut p = e.trim().split(';');
                p.next() == Some("br") && !p.any(|q| q.trim() == "q=0")
            })
        });
    let compressed = if hash == s.wrapper_hash() {
        Some(pf_prelude::WRAPPER_BR)
    } else if hash == pf_blobs::sha256_hex(pf_ui::wrapper_css().as_bytes()) {
        Some(pf_ui::WRAPPER_CSS_BR)
    } else if hash == pf_blobs::sha256_hex(pf_prelude::EDITOR_JS.as_bytes()) {
        Some(pf_prelude::EDITOR_BR)
    } else {
        None
    };
    let use_br = accepts_br && compressed.is_some();
    if use_br {
        b = compressed.unwrap().to_vec().into();
    }
    tracing::info!(event = "blobhost.serve", outcome = "ok");
    let etag = format!("\"{hash}{}\"", if use_br { "-br" } else { "" });
    let mut r = if req
        .headers()
        .get(header::IF_NONE_MATCH)
        .is_some_and(|v| v == etag.as_str())
    {
        StatusCode::NOT_MODIFIED.into_response()
    } else if req.method() == http::Method::HEAD {
        Body::empty().into_response()
    } else {
        Body::from(b).into_response()
    };
    if use_br {
        r.headers_mut().insert(
            header::CONTENT_ENCODING,
            http::HeaderValue::from_static("br"),
        );
    }
    r.headers_mut().insert(
        header::VARY,
        http::HeaderValue::from_static("Accept-Encoding"),
    );
    r.headers_mut().insert(
        header::CACHE_CONTROL,
        http::HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    if let Ok(t) = http::HeaderValue::from_str(&typ) {
        r.headers_mut().insert(header::CONTENT_TYPE, t);
    }
    r.headers_mut()
        .insert(header::ETAG, http::HeaderValue::from_str(&etag).unwrap());
    r
}

pub async fn page(s: PartHost, req: Request) -> Response {
    let req = if req.method() == http::Method::POST && req.uri().path().starts_with("/l/") {
        let (parts, body) = req.into_parts();
        match axum::body::to_bytes(body, 1024).await {
            Ok(bytes) => Request::from_parts(parts, Body::from(bytes)),
            Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        }
    } else {
        req
    };
    let Some(p) = req.extensions().get::<pf_serve::PartHost>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let name = p.0.clone();
    let active = s
        .deps()
        .db
        .read({
            let name = name.clone();
            move |db| {
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM parts WHERE name=?1 AND deleted_at IS NULL)",
                    [name],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(pf_db::DbError::from)
            }
        })
        .await;
    match active {
        Ok(true) => {}
        Ok(false) => return pf_serve::error_page(StatusCode::NOT_FOUND, "Nothing here."),
        Err(e) => return error(e.into()),
    }
    if req.uri().path() == "/_pf/manifest.webmanifest" {
        return ([(header::CONTENT_TYPE,"application/manifest+json"),(header::CACHE_CONTROL,"public, max-age=3600")],r#"{"id":"/","scope":"/","start_url":"/","name":"partifactory","short_name":"part","display":"standalone","icons":[{"src":"/_pf/icon.svg","sizes":"any","type":"image/svg+xml"}]}"#).into_response();
    }
    if req.uri().path() == "/_pf/icon.svg" {
        return ([(header::CONTENT_TYPE,"image/svg+xml"),(header::CACHE_CONTROL,"public, max-age=31536000, immutable")],r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="2" y="2" width="20" height="20" rx="3" fill="#1c6a47"/><path d="M6 7h12v5H6zM9 17h6" fill="none" stroke="white" stroke-width="1.5"/></svg>"##).into_response();
    }
    let mut is_link = false;
    let public;
    if let Some(rest) = req.uri().path().strip_prefix("/l/") {
        let (token, action) = rest.split_once('/').unwrap_or((rest, ""));
        if !matches!(action, "" | "go") {
            return StatusCode::NOT_FOUND.into_response();
        }
        let links = s.links();
        let link = match links.lookup(token, &name, s.now()).await {
            Err(e) => {
                return if e.code == "link_expired" {
                    pf_serve::error_page(StatusCode::GONE, "LINK EXPIRED")
                } else {
                    StatusCode::NOT_FOUND.into_response()
                };
            }
            Ok(l) => l,
        };
        if let Some(r) = pf_links::web::limit_link(&req, &format!("link:{}", link.id)) {
            return r;
        }
        if action == "go" {
            if req.method() != http::Method::POST {
                return StatusCode::METHOD_NOT_ALLOWED.into_response();
            }
            if req
                .headers()
                .get(header::ORIGIN)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|o| o != s.config().part_origin(&name))
            {
                return StatusCode::FORBIDDEN.into_response();
            }
            return match links.go(token, &name, s.now()).await {
                Ok(cap) => {
                    let mut r = axum::Json(serde_json::json!({"cap": cap})).into_response();
                    r.headers_mut().insert(
                        header::CACHE_CONTROL,
                        http::HeaderValue::from_static("no-store"),
                    );
                    r
                }
                Err(e) => error(e),
            };
        }
        is_link = true;
        public = true;
    } else if req.uri().path().starts_with("/_pf/") && req.uri().path() != "/_pf/" {
        return pf_serve::error_page(StatusCode::NOT_FOUND, "Nothing here.");
    } else {
        public = s.links().active(&name, s.now()).await.unwrap_or(false);
    }
    if req.method() != http::Method::GET && req.method() != http::Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let unsandboxed = s
        .live(&name, None)
        .await
        .ok()
        .flatten()
        .is_some_and(|live| live.unsandboxed);
    let unsandboxed_attr = if unsandboxed {
        " data-pf-unsandboxed=\"1\""
    } else {
        ""
    };
    let script = format!("{}/b/{}", s.config().blob_origin(), s.wrapper_hash());
    let editor = format!(
        "{}/b/{}",
        s.config().blob_origin(),
        pf_blobs::sha256_hex(pf_prelude::EDITOR_JS.as_bytes())
    );
    let css = format!(
        "{}/b/{}",
        s.config().blob_origin(),
        pf_blobs::sha256_hex(pf_ui::wrapper_css().as_bytes())
    );
    let card = if public {
        "Anyone with the link"
    } else {
        "Sign in to open"
    };
    let link_attr = if is_link { " data-pf-link=\"1\"" } else { "" };
    let og = if public {
        pf_ui::og::Card::Public
    } else {
        pf_ui::og::Card::Private
    }
    .tags(&s.config().workspace_origin());
    let body = format!(
        "<!doctype html><html data-pf-part=\"{}\" data-pf-ws=\"{}\" data-pf-editor=\"{editor}\"{link_attr}{unsandboxed_attr}><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"robots\" content=\"noindex,nofollow\">{og}<link rel=\"stylesheet\" href=\"{css}\"><link rel=\"manifest\" href=\"/_pf/manifest.webmanifest\"><link rel=\"icon\" href=\"/_pf/icon.svg\"><title>partifactory</title><script src=\"{script}\" defer></script></head><body><div id=\"pf-card\">{card}</div><noscript>Sign in to open</noscript></body></html>",
        name,
        s.config().workspace_origin()
    );
    let csp = format!(
        "default-src 'none'; script-src {}; style-src {}; font-src {}; img-src 'self'; manifest-src 'self'; frame-src 'self'; connect-src {}{}; base-uri 'none'; frame-ancestors {} {}",
        s.config().blob_origin(),
        s.config().blob_origin(),
        s.config().blob_origin(),
        s.config().workspace_origin(),
        if is_link { " 'self'" } else { "" },
        s.config().workspace_origin(),
        s.config().embedders.join(" ")
    );
    let mut r = axum::response::Html(if req.method() == http::Method::HEAD {
        String::new()
    } else {
        body
    })
    .into_response();
    r.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        http::HeaderValue::from_str(csp.trim()).unwrap(),
    );
    r.headers_mut().insert(
        header::REFERRER_POLICY,
        http::HeaderValue::from_static("no-referrer"),
    );
    tracing::info!(
        event = "wrapper.serve",
        audience = if public { "public" } else { "private" },
        outcome = "ok"
    );
    r.headers_mut().insert(
        "x-robots-tag",
        http::HeaderValue::from_static("noindex, nofollow"),
    );
    r.headers_mut().insert(
        header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    r
}
