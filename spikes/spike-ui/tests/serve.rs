use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn get(path: &str) -> (u16, String) {
    spike_ui::init_executor();
    let resp = spike_ui::app().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status().as_u16();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn both_variants_render_the_same_sheet() {
    for path in ["/leptos", "/templates"] {
        let (status, html) = get(path).await;
        assert_eq!(status, 200, "{path}");
        assert!(html.contains("You and the Family group (6) can see this."), "{path}: live sentence SSR");
        assert!(html.contains("Anyone with the link"), "{path}: five-rung ladder");
        assert!(html.contains("class=\"bar\""), "{path}: part-view bar");
    }
    let (_, leptos) = get("/leptos").await;
    assert!(leptos.contains("/pkg/spike_ui_app.js"), "hydration script points at the wasm bundle");
    let (_, templates) = get("/templates").await;
    assert!(templates.contains("/static/share-island.js"));
}
