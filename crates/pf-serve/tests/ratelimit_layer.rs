//! A registered route returns 429 with `Retry-After` at threshold + 1, keyed per client address;
//! `event="ratelimit.trip"` is logged and `pf_rate_limited_total` increments. The `[unsafe]`
//! switch is checked through `ServeConfig::from_loaded`.

mod common;
use axum::{Router, routing::get as route_get};
use common::*;
use pf_serve::{RateRule, ratelimit};
use std::time::Duration;

fn limited(b: &mut pf_serve::ServerBuilder) {
    b.rate_rule(RateRule::per_ip(
        "self_test",
        "api",
        3,
        Duration::from_secs(60),
    ));
    b.workspace(Router::new().route(
        "/api/limited",
        route_get(|| async { "ok" }).route_layer(ratelimit::layer("self_test")),
    ));
}

#[tokio::test]
async fn self_test_route_trips_at_threshold_plus_one() {
    let s = start(|_| {}, limited).await;
    let before = pf_obs::metrics::metrics()
        .rate_limited
        .with_label_values(&["api"])
        .get();
    let from = |ip: &'static str| [("X-Forwarded-For", ip)];
    for i in 0..3 {
        let r = get(s.workspace, "/api/limited", &from("198.51.100.1")).await;
        assert_eq!(r.status, 200, "request {i}");
    }
    let r = get(s.workspace, "/api/limited", &from("198.51.100.1")).await;
    assert_eq!(r.status, 429);
    let retry: u64 = r.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=20).contains(&retry), "retry-after {retry}");
    // Another client address has its own bucket.
    let r = get(s.workspace, "/api/limited", &from("198.51.100.2")).await;
    assert_eq!(r.status, 200);
    settle().await;
    let trips = s.logs.events("ratelimit.trip");
    assert_eq!(trips.len(), 1);
    assert_eq!(trips[0]["route_class"], "api");
    assert_eq!(trips[0]["key_kind"], "ip");
    let access: Vec<_> = s
        .logs
        .events("http.request")
        .into_iter()
        .filter(|l| l["status"] == 429)
        .collect();
    assert_eq!(access[0]["outcome"], "rate_limited");
    assert_eq!(
        pf_obs::metrics::metrics()
            .rate_limited
            .with_label_values(&["api"])
            .get(),
        before + 1
    );
}

#[tokio::test]
async fn disabled_layer_never_trips() {
    let s = start(|c| c.rate_limits = false, limited).await;
    for _ in 0..6 {
        assert_eq!(get(s.workspace, "/api/limited", &[]).await.status, 200);
    }
}

#[test]
fn unsafe_key_controls_the_layer() {
    let mut env = pf_config::Env::new();
    env.insert("PF_SESSION_KEY".into(), "s".into());
    env.insert("PF_CAP_HMAC_KEY".into(), "c".into());
    env.insert("PF_METRICS_TOKEN".into(), "m".into());
    env.insert("PF_DOMAIN_APEX".into(), "pf.test".into());
    let loaded = pf_config::load("", &env).unwrap();
    assert!(
        pf_serve::ServeConfig::from_loaded(&loaded, &env)
            .unwrap()
            .rate_limits
    );
}
