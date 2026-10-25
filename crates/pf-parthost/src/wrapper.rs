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
    for (label, src) in [
        (pf_prelude::PRELUDE_LABEL, pf_prelude::PRELUDE_JS),
        (pf_prelude::WRAPPER_LABEL, pf_prelude::WRAPPER_JS),
    ] {
        let put = s.deps().blobs.put(src.as_bytes())?;
        seeds.push((label.to_string(), put.hash, put.size as i64));
    }
    let now = s.now();
    let rows = seeds.clone();
    let (shared,css)=s.write(move |db|{
        for (label,hash,size) in rows {
            db.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",pf_db::rusqlite::params![hash,size,now])?;
            db.execute("INSERT INTO shared_blobs(hash,source,label,added_at,content_type) VALUES(?1,'seed',?2,?3,'text/javascript; charset=utf-8') ON CONFLICT(hash) DO UPDATE SET content_type=excluded.content_type",pf_db::rusqlite::params![hash,label,now])?;
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
    let b = match s.bytes(hash) {
        Ok(b) => b,
        Err(e) => return error(e),
    };
    tracing::info!(event = "blobhost.serve", outcome = "ok");
    let etag = format!("\"{hash}\"");
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
    } else if req.uri().path() != "/" && req.uri().path() != "/_pf/" {
        return pf_serve::error_page(StatusCode::NOT_FOUND, "Nothing here.");
    } else {
        public = s.links().active(&name, s.now()).await.unwrap_or(false);
    }
    if req.method() != http::Method::GET && req.method() != http::Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let script = format!("{}/b/{}", s.config().blob_origin(), s.wrapper_hash());
    let card = if public {
        "Anyone with the link"
    } else {
        "Sign in to open this part"
    };
    let link_attr = if is_link { " data-pf-link=\"1\"" } else { "" };
    let body = format!(
        "<!doctype html><html data-pf-part=\"{}\" data-pf-ws=\"{}\"{link_attr}><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"robots\" content=\"noindex,nofollow\"><meta property=\"og:title\" content=\"partifactory\"><meta property=\"og:description\" content=\"{card}\"><title>partifactory</title><script src=\"{script}\" defer></script></head><body><div id=\"pf-card\">{card}</div></body></html>",
        name,
        s.config().workspace_origin()
    );
    let csp = format!(
        "default-src 'none'; script-src {}; style-src 'unsafe-inline'; frame-src {}; connect-src {} {}; base-uri 'none'; frame-ancestors 'none'",
        s.config().blob_origin(),
        s.config().part_origin(&name),
        s.config().workspace_origin(),
        s.config().part_origin(&name)
    );
    let mut r = axum::response::Html(if req.method() == http::Method::HEAD {
        String::new()
    } else {
        body
    })
    .into_response();
    r.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        http::HeaderValue::from_str(&csp).unwrap(),
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
