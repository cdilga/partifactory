//! The shared rate-limit layer (§5d, on by default): one route table, one `governor` keyed
//! limiter per registered route, keyed on the vetted client address, or on the principal once an
//! auth layer has put one in the [`AccessSlot`](crate::access::AccessSlot).
//!
//! Later beads register their routes once, at startup:
//!
//! ```ignore
//! builder.rate_rule(RateRule::per_ip("grant", "grant", 30, Duration::from_secs(60)));
//! Router::new().route("/grant", post(grant)).route_layer(pf_serve::ratelimit::layer("grant"))
//! ```
//!
//! A refusal is `429` with `Retry-After`, an `event="ratelimit.trip"` line and
//! `pf_rate_limited_total{route_class}`. Only `[unsafe] rate_limits = "off"` switches it off.

use crate::access::AccessSlot;
use crate::error_page::error_page;
use crate::ingress::ClientAddr;
use crate::state::AppState;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use governor::clock::{Clock, DefaultClock};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use http::StatusCode;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// What a rule keys on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// The vetted client address only.
    Ip,
    /// The principal when one is attached, else the client address.
    PrincipalOrIp,
}

impl KeyKind {
    fn as_str(self) -> &'static str {
        match self {
            KeyKind::Ip => "ip",
            KeyKind::PrincipalOrIp => "principal_or_ip",
        }
    }
}

/// One row of the route table: `{route, route_class, key, limit, window}`.
#[derive(Debug, Clone)]
pub struct RateRule {
    /// The name routes refer to in [`layer`].
    pub route: &'static str,
    /// The `route_class` label of the refusal metric.
    pub route_class: &'static str,
    pub key: KeyKind,
    pub limit: u32,
    pub window: Duration,
}

impl RateRule {
    pub fn per_ip(
        route: &'static str,
        route_class: &'static str,
        limit: u32,
        window: Duration,
    ) -> Self {
        Self {
            route,
            route_class,
            key: KeyKind::Ip,
            limit,
            window,
        }
    }
    pub fn per_principal(
        route: &'static str,
        route_class: &'static str,
        limit: u32,
        window: Duration,
    ) -> Self {
        Self {
            route,
            route_class,
            key: KeyKind::PrincipalOrIp,
            limit,
            window,
        }
    }
}

struct Entry {
    rule: RateRule,
    limiter: DefaultKeyedRateLimiter<String>,
}

/// The route table. Cheap to clone; registration may happen at any time.
#[derive(Clone)]
pub struct RateTable {
    enabled: bool,
    entries: Arc<RwLock<HashMap<&'static str, Arc<Entry>>>>,
}

impl RateTable {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            entries: Arc::default(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Register (or replace) a rule.
    pub fn register(&self, rule: RateRule) {
        let limit = NonZeroU32::new(rule.limit.max(1)).expect("non-zero");
        let period = rule.window / limit.get();
        let quota = Quota::with_period(period.max(Duration::from_nanos(1)))
            .expect("non-zero period")
            .allow_burst(limit);
        let entry = Entry {
            rule: rule.clone(),
            limiter: RateLimiter::keyed(quota),
        };
        self.entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(rule.route, Arc::new(entry));
    }

    /// Every registered rule (for the table-driven tests later beads add).
    pub fn rules(&self) -> Vec<RateRule> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|e| e.rule.clone())
            .collect()
    }

    /// Forget idle keys (the server calls this once a minute).
    pub fn shrink(&self) {
        for e in self
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            e.limiter.retain_recent();
            e.limiter.shrink_to_fit();
        }
    }

    /// `Ok` to proceed, or `Err(retry_after)`.
    pub fn check(&self, route: &str, key: &str) -> Result<(), Duration> {
        if !self.enabled {
            return Ok(());
        }
        let Some(e) = self
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(route)
            .cloned()
        else {
            return Ok(());
        };
        e.limiter
            .check_key(&key.to_string())
            .map_err(|nu| nu.wait_time_from(DefaultClock::default().now()))
    }
}

async fn enforce(route: &'static str, req: Request, next: Next) -> Response {
    let Some(state) = req.extensions().get::<AppState>().cloned() else {
        return next.run(req).await;
    };
    let rates = state.rates();
    let Some(rule) = rates.rules().into_iter().find(|r| r.route == route) else {
        tracing::warn!(
            event = "ratelimit.unregistered",
            route,
            "route has a limit layer but no rule"
        );
        return next.run(req).await;
    };
    let ip = req
        .extensions()
        .get::<ClientAddr>()
        .map(|c| c.0.to_string())
        .unwrap_or_default();
    let principal = req
        .extensions()
        .get::<AccessSlot>()
        .and_then(|s| s.get().principal);
    let key = match (rule.key, principal) {
        (KeyKind::PrincipalOrIp, Some(p)) => format!("p:{p}"),
        _ => format!("ip:{ip}"),
    };
    match rates.check(route, &key) {
        Ok(()) => next.run(req).await,
        Err(wait) => {
            let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
            let secs = secs.max(1);
            pf_obs::metrics::metrics()
                .rate_limited
                .with_label_values(&[rule.route_class])
                .inc();
            if let Some(slot) = req.extensions().get::<AccessSlot>() {
                slot.update(|a| a.outcome = Some("rate_limited"));
            }
            tracing::warn!(
                event = pf_obs::fields::RATELIMIT_TRIP,
                route_class = rule.route_class,
                route,
                key_kind = rule.key.as_str(),
                retry_after_s = secs,
                outcome = "rate_limited",
                "rate limit hit"
            );
            let mut resp = error_page(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many requests. Try again shortly.",
            );
            resp.headers_mut().insert(
                http::header::RETRY_AFTER,
                http::HeaderValue::from_str(&secs.to_string()).expect("digits"),
            );
            resp
        }
    }
}

/// The layer for a registered route: `.route_layer(pf_serve::ratelimit::layer("grant"))`.
pub fn layer(route: &'static str) -> RateLimitLayer {
    axum::middleware::from_fn_with_state(route, enforce_boxed as EnforceFn)
}

type BoxedResponse = std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>;
type EnforceFn = fn(axum::extract::State<&'static str>, Request, Next) -> BoxedResponse;

/// The layer type [`layer`] returns.
pub type RateLimitLayer = axum::middleware::FromFnLayer<
    EnforceFn,
    &'static str,
    (axum::extract::State<&'static str>, Request),
>;

fn enforce_boxed(
    axum::extract::State(route): axum::extract::State<&'static str>,
    req: Request,
    next: Next,
) -> BoxedResponse {
    Box::pin(enforce(route, req, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_limits_per_key() {
        let t = RateTable::new(true);
        t.register(RateRule::per_ip("x", "api", 3, Duration::from_secs(60)));
        for _ in 0..3 {
            assert!(t.check("x", "ip:a").is_ok());
        }
        let wait = t.check("x", "ip:a").unwrap_err();
        assert!(wait > Duration::ZERO && wait <= Duration::from_secs(20));
        assert!(
            t.check("x", "ip:b").is_ok(),
            "other keys have their own bucket"
        );
        assert!(t.check("unregistered", "ip:a").is_ok());
    }

    #[test]
    fn disabled_table_never_limits() {
        let t = RateTable::new(false);
        t.register(RateRule::per_ip("x", "api", 1, Duration::from_secs(60)));
        for _ in 0..10 {
            assert!(t.check("x", "ip:a").is_ok());
        }
    }
}
