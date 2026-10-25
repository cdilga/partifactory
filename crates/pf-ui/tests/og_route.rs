use axum::body::{Body, to_bytes};
use http::{Request, StatusCode};
use tower::ServiceExt;
#[tokio::test]
async fn cards_are_content_addressed_immutable_and_never_set_cookies() {
    let dir = tempfile::tempdir().unwrap();
    let mut b =
        pf_serve::ServerBuilder::new(pf_serve::ServeConfig::for_tests(dir.path().into(), "test"));
    pf_ui::install(&mut b);
    let app = b.build().workspace;
    for card in [pf_ui::og::Card::Private, pf_ui::og::Card::Public] {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/og/{}.png", card.hash()))
                    .header("host", "parts.pf.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["content-type"], "image/png");
        assert_eq!(
            r.headers()["cache-control"],
            "public, max-age=31536000, immutable"
        );
        assert!(!r.headers().contains_key("set-cookie"));
        assert_eq!(
            &to_bytes(r.into_body(), 150_000).await.unwrap()[..],
            card.png()
        );
    }
    let r = app
        .oneshot(
            Request::builder()
                .uri("/og/unknown.png")
                .header("host", "parts.pf.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
