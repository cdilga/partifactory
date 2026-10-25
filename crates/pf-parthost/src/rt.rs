use crate::{Live, PartHost, content};
use axum::{
    extract::Request,
    response::{IntoResponse, Response},
};
use pf_caps::Claims;
use pf_model::PfError;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};

pub type RtResult = Result<Value, PfError>;
pub type RtOp =
    Arc<dyn Fn(RtCtx, Value) -> Pin<Box<dyn Future<Output = RtResult> + Send>> + Send + Sync>;
#[derive(Clone)]
pub struct RtCtx {
    pub host: PartHost,
    pub claims: Claims,
    pub live: Arc<Live>,
}

pub(crate) fn origin_allowed(s: &PartHost, req: &Request, c: &Claims, unsandboxed: bool) -> bool {
    let origin = req
        .headers()
        .get(http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let mode = req
        .headers()
        .get("sec-fetch-mode")
        .and_then(|v| v.to_str().ok());
    (origin == Some("null")
        || (unsandboxed && origin == Some(s.config().part_origin(&c.h).as_str())))
        && mode.is_none_or(|m| m == "cors")
}

pub async fn handle(s: PartHost, req: Request) -> Response {
    if req.method() != http::Method::POST {
        return http::StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let (_, c) = match content::authorize(&s, &req) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    // Reject foreign origins before a DB query or body read, even for an unsandboxed part.
    let origin = req
        .headers()
        .get(http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if origin != Some("null") && origin != Some(s.config().part_origin(&c.h).as_str()) {
        return http::StatusCode::FORBIDDEN.into_response();
    }
    let live = match s.live(&c.h, c.v.as_deref()).await {
        Ok(Some(l)) if l.part_id == c.p => l,
        Ok(_) => return http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return content::error(e),
    };
    if !origin_allowed(&s, &req, &c, live.unsandboxed) {
        return http::StatusCode::FORBIDDEN.into_response();
    }
    if req
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| {
            !v.split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/plain"))
        })
    {
        return http::StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let body = match axum::body::to_bytes(req.into_body(), 1 << 20).await {
        Ok(b) => b,
        Err(_) => return content::error(PfError::new("too_large", "runtime request too large")),
    };
    let input: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return content::error(PfError::invalid("body", "expected JSON")),
    };
    let name = input.get("op").and_then(Value::as_str).unwrap_or("");
    let op = s
        .inner
        .ops
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(name)
        .cloned();
    let Some(op) = op else {
        return content::error(PfError::invalid("op", "unknown runtime operation"));
    };
    let result = op(
        RtCtx {
            host: s.clone(),
            claims: c,
            live,
        },
        input.get("args").cloned().unwrap_or(json!({})),
    )
    .await;
    tracing::info!(
        event = "rt.op",
        op = name,
        outcome = if result.is_ok() { "ok" } else { "error" }
    );
    let mut r = match result {
        Ok(v) => axum::Json(json!({"ok":v})).into_response(),
        Err(e) => content::error(e),
    };
    r.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    r
}

pub(crate) fn register_builtin(s: &PartHost) {
    for name in [
        "storage.get",
        "storage.set",
        "storage.delete",
        "storage.sync",
    ] {
        let n = name.to_string();
        s.register_op(
            name,
            Arc::new(move |ctx, args| {
                let n = n.clone();
                Box::pin(crate::storage::dispatch(ctx, n, args))
            }),
        );
    }
    s.register_op("refresh", Arc::new(|ctx, _| Box::pin(refresh(ctx))));
    s.register_op(
        "errors.report",
        Arc::new(|ctx, args| Box::pin(report_error(ctx, args))),
    );
}

async fn refresh(ctx: RtCtx) -> RtResult {
    let c = &ctx.claims;
    let Some(user) = c.s.strip_prefix("user:").filter(|_| c.g.is_none()) else {
        return Err(PfError::new("forbidden", "refresh requires a person"));
    };
    let p = pf_model::Principal::person(user);
    let id = c.p.clone();
    let policy = ctx.host.deps().policy.clone();
    let now = ctx.host.now();
    let role = ctx
        .host
        .read(move |db| Ok(policy.role(db, &p, &id, now)?))
        .await?
        .ok_or_else(|| PfError::new("forbidden", "access was revoked"))?;
    let grant = pf_caps::Grant {
        part_id: c.p.clone(),
        host: c.h.clone(),
        subject: c.s.clone(),
        role: role.min(c.r),
        read_only: c.ro,
        data_write: c.dw,
        guest: c.g.clone(),
        minter: c.m.clone(),
        version: c.v.clone(),
        draft: c.d.clone(),
    };
    let cap = pf_caps::mint(&ctx.host.deps().keys, &ctx.host.deps().epochs, &grant, now)
        .map_err(|_| PfError::internal("cannot mint capability"))?;
    Ok(json!({"cap":cap}))
}

async fn report_error(ctx: RtCtx, args: Value) -> RtResult {
    let kind = args
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("error")
        .to_string();
    if !["error", "unhandledrejection", "blocked_load", "csp"].contains(&kind.as_str()) {
        return Err(PfError::invalid("kind", "unknown error kind"));
    }
    let clean = |k: &str, max: usize| -> String {
        pf_obs::redact_str(args.get(k).and_then(Value::as_str).unwrap_or(""))
            .chars()
            .take(max)
            .collect()
    };
    let message = clean("message", 4096);
    let stack = clean("stack", 8192);
    let url = clean("url", 2048);
    let fingerprint = pf_blobs::sha256_hex(
        format!("{kind}\0{message}\0{}", stack.lines().next().unwrap_or("")).as_bytes(),
    );
    let now = ctx.host.now();
    let part = ctx.claims.p.clone();
    let version = ctx.live.version_id.clone();
    let source = if ctx.claims.s == "render" {
        "render"
    } else {
        "viewer"
    };
    let log_part = part.clone();
    let log_kind = kind.clone();
    let host = ctx.host.clone();
    ctx.host.write(move|db|{
        let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_errors WHERE part_id=?1 AND version_id=?2 AND source=?3 AND fingerprint=?4)",pf_db::rusqlite::params![part,version,source,fingerprint],|r|r.get(0))?;
        if !exists {
            let mut limits=host.inner.err_limits.lock().unwrap_or_else(|e|e.into_inner());
            let n=limits.entry(part.clone()).or_insert((now,0));
            if now-n.0>=60000 {*n=(now,0)}
            if n.1>=10 {return Err(PfError::new("rate_limited","too many runtime errors").with("retry_after",60))}
            n.1+=1;
        }
        db.execute("INSERT INTO runtime_errors(part_id,version_id,source,fingerprint,kind,message,stack,url,first_at,last_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9) ON CONFLICT(part_id,version_id,source,fingerprint) DO UPDATE SET count=count+1,last_at=excluded.last_at",pf_db::rusqlite::params![part,version,source,fingerprint,kind,message,stack,url,now])?;Ok(())
    }).await?;
    tracing::info!(event="runtime_error.recorded",part_id=%log_part,kind=%log_kind,outcome="ok");
    Ok(json!({}))
}
