//! Canary values (caps, tokens, session ids, share keys) logged every way a careless caller might
//! never reach the output. On failure the captured log is kept under `target/test-logs/`.

use pf_obs::testing::Capture;
use pf_obs::{BearerToken, CapToken, RefreshToken, SessionId};

const CAP: &str = "CANARYcapVALUE0123456789";
const AT: &str = "pf_at_CANARYaccess0123456789";
const RT: &str = "pf_rt_CANARYrefresh0123456789";
const PAT: &str = "pf_pat_CANARYpersonal0123456789";
const SESS: &str = "CANARYsession0123456789";
const SHARE: &str = "CANARYsharekey0123456789";

#[test]
fn canary_values_never_logged() {
    let cap = Capture::new("trace");
    {
        let _g = cap.set_default();
        let c = CapToken::new(CAP);
        let b = BearerToken::new(AT);
        let r = RefreshToken::new(RT);
        let s = SessionId::new(SESS);
        // By type.
        tracing::info!(event = "test.types", cap = ?c, bearer = %b, refresh = ?r, session = %s, "types");
        // By pattern: raw strings in fields and messages.
        tracing::info!(
            event = "test.path",
            path = format!("/c/{CAP}/index.html"),
            "fetch /c/{CAP}/a.js"
        );
        tracing::warn!(
            event = "test.tokens",
            detail = format!("got {AT} and {RT}"),
            "pat {PAT}"
        );
        tracing::info!(event = "test.bearer", "Authorization: Bearer {AT}");
        tracing::info!(
            event = "test.location",
            location = format!("https://x-part.pf.test/#pf={SHARE}"),
            "redirect"
        );
        pf_obs::fields::access(&pf_obs::fields::Access {
            request_id: "r1",
            route_class: "content_entry",
            method: "GET",
            path: &format!("/c/{CAP}/"),
            status: 200,
            outcome: "ok",
            client_ip: "192.0.2.1",
            ..Default::default()
        });
    }
    let text = cap.text();
    let leaked: Vec<&str> = [
        "CANARYcap",
        "CANARYaccess",
        "CANARYrefresh",
        "CANARYpersonal",
        "CANARYsession",
        "CANARYsharekey",
    ]
    .into_iter()
    .filter(|c| text.contains(c))
    .collect();
    if !leaked.is_empty() {
        let p = cap.keep("canary_values_never_logged");
        panic!(
            "canary values {leaked:?} leaked; log kept at {}",
            p.display()
        );
    }
    // The lines are still there, in shape.
    assert_eq!(cap.events("http.request").len(), 1);
    assert!(text.contains("/c/[cap]/"));
}

#[test]
fn log_level_runtime_reload() {
    let cap = Capture::new("info");
    let _g = cap.set_default();
    tracing::debug!(event = "test.before", "hidden");
    cap.handle().set("debug").expect("reload");
    tracing::debug!(event = "test.after", "shown");
    cap.handle().set("warn").expect("reload");
    tracing::info!(event = "test.muted", "hidden again");
    assert!(cap.events("test.before").is_empty());
    assert_eq!(cap.events("test.after").len(), 1);
    assert!(cap.events("test.muted").is_empty());
    assert_eq!(cap.handle().current(), "warn");
    assert!(cap.handle().set("not a [valid filter").is_err());
}

#[test]
fn lines_are_flat_json_with_convention_fields() {
    let cap = Capture::new("info");
    let _g = cap.set_default();
    tracing::info!(
        event = "test.shape",
        request_id = "abc",
        outcome = "ok",
        "hello"
    );
    let l = &cap.events("test.shape")[0];
    for k in [
        "timestamp",
        "level",
        "target",
        "message",
        "event",
        "request_id",
        "outcome",
    ] {
        assert!(l.get(k).is_some(), "missing {k} in {l}");
    }
}
