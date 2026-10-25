use crate::PartHostConfig;
use axum::{extract::Request, middleware::Next, response::Response};
use http::{HeaderValue, header};

/// The single document policy, including explicit origins for opaque WebKit documents.
pub fn canonical_csp(cfg: &PartHostConfig, name: &str, unsandboxed: bool) -> String {
    let own = cfg.part_origin(name);
    let blob = cfg.blob_origin();
    let ws = own
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let sandbox = if unsandboxed {
        ""
    } else {
        "sandbox allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals allow-downloads allow-top-navigation-by-user-activation; "
    };
    format!(
        "{sandbox}default-src 'none'; script-src 'self' {own} 'unsafe-inline' 'unsafe-eval' blob: {blob}; style-src 'self' {own} 'unsafe-inline' {blob}; img-src 'self' {own} data: blob: {blob}; media-src 'self' {own} data: blob: {blob}; font-src 'self' {own} data: {blob}; connect-src 'self' {own} {ws} {blob}; worker-src 'self' {own} blob:; frame-src 'self' {own}; form-action 'self' {own}; base-uri 'none'; object-src 'none'; frame-ancestors {own} {} {}",
        cfg.workspace_origin(),
        cfg.embedders.join(" ")
    ).trim().to_owned()
}

fn common(r: &mut Response) {
    let h = r.headers_mut();
    for k in [
        "set-cookie",
        "access-control-allow-credentials",
        "cross-origin-resource-policy",
    ] {
        h.remove(k);
    }
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
}

pub async fn part_host_guard(req: Request, next: Next) -> Response {
    let null = req
        .headers()
        .get(header::ORIGIN)
        .is_some_and(|v| v == "null");
    let mut r = next.run(req).await;
    common(&mut r);
    if null {
        r.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("null"),
        );
        r.headers_mut()
            .append(header::VARY, HeaderValue::from_static("Origin"));
        r.headers_mut().insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("X-PF-Reauth"),
        );
    }
    r
}

pub async fn blob_host_guard(req: Request, next: Next) -> Response {
    let mut r = next.run(req).await;
    common(&mut r);
    r.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    r
}
