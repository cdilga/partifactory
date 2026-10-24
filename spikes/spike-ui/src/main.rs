#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().json().flatten_event(true).init();
    // Friction: leptos_axum panics per request ("spawn before a global executor
    // was initialized") unless this runs; LeptosRoutes normally hides it.
    spike_ui::init_executor();
    let addr = std::env::var("SPIKE_UI_ADDR").unwrap_or_else(|_| "127.0.0.1:8081".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(event = "server.start", addr = %addr);
    axum::serve(listener, spike_ui::app()).await?;
    Ok(())
}
