//! No metric carries a part name, principal, host or request id as a label.

use pf_obs::metrics::{
    ALLOWED_LABELS, FORBIDDEN_LABELS, ROUTE_CLASSES, metrics, record_http, record_op,
};
use std::time::Duration;

#[test]
fn no_high_cardinality_labels() {
    // Touch every vec so its children appear in the gather.
    for rc in ROUTE_CLASSES {
        record_http(rc, 200, Duration::from_millis(1));
    }
    record_op("part_create", "api", "ok", Duration::from_millis(2));
    metrics().rate_limited.with_label_values(&["grant"]).inc();
    metrics()
        .ready_check_ok
        .with_label_values(&["disk", "required"])
        .set(1);
    metrics().store_bytes.with_label_values(&["blobs"]).set(0);

    let families = metrics().registry.gather();
    assert!(families.len() >= 8);
    for f in &families {
        assert!(
            f.name().starts_with("pf_"),
            "metric {} lacks the pf_ prefix",
            f.name()
        );
        for m in f.get_metric() {
            for l in m.get_label() {
                let name = l.name();
                assert!(
                    !FORBIDDEN_LABELS.contains(&name),
                    "{} has forbidden label {name}",
                    f.name()
                );
                assert!(
                    ALLOWED_LABELS.contains(&name),
                    "{} has unlisted label {name}",
                    f.name()
                );
                if name == "route_class" {
                    assert!(
                        ROUTE_CLASSES.contains(&l.value()),
                        "unknown route class {}",
                        l.value()
                    );
                }
            }
        }
    }
    let text = pf_obs::metrics::render();
    assert!(text.contains("pf_http_requests_total{route_class=\"api\",status_class=\"2xx\"}"));
    assert!(text.contains("pf_nonpart_host_hits_total"));
}

#[test]
fn token_compare() {
    use pf_obs::metrics::token_matches;
    assert!(token_matches(b"abc", b"abc"));
    assert!(!token_matches(b"abd", b"abc"));
    assert!(!token_matches(b"ab", b"abc"));
    assert!(!token_matches(b"", b""));
}
