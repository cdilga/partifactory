//! pf-twz8 UI spike server (lib, so tests can drive it): `/leptos` (Leptos 0.8 SSR + hydration) and
//! `/templates` (askama + one vanilla-JS island), the same bar and share sheet.

use std::path::PathBuf;

use askama::Template;
use axum::{Router, response::Html, routing::get};
use leptos::{config::LeptosOptions, prelude::*};
use spike_ui_app::{App, EditorProbe, Rung, sentence};
use tower_http::services::ServeDir;

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <title>"Photo finder · Leptos"</title>
                <link rel="stylesheet" href="/static/app.css"/>
                <HydrationScripts options/>
            </head>
            <body><App/></body>
        </html>
    }
}

const EDITOR_BOOT: &str = "import init, { hydrate_editor } from '/pkg-editor/spike_ui_app.js'; \
await init({ module_or_path: '/pkg-editor/spike_ui_app_bg.wasm' }); hydrate_editor();";

fn editor_shell() -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <title>"CodeMirror probe · Leptos"</title>
                <link rel="stylesheet" href="/static/app.css"/>
                <script type="module">{EDITOR_BOOT}</script>
            </head>
            <body><EditorProbe/></body>
        </html>
    }
}

#[derive(Template)]
#[template(path = "part.html")]
struct PartPage {
    rungs: Vec<(&'static str, bool, bool)>,
    chip: &'static str,
    sentence: String,
}

async fn templates() -> Html<String> {
    let current = Rung::Group;
    let page = PartPage {
        rungs: Rung::ALL.iter().map(|r| (r.label(), *r == current, *r == Rung::Public)).collect(),
        chip: current.chip(),
        sentence: sentence(current, false),
    };
    Html(page.render().expect("template renders"))
}

pub fn app() -> Router {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let options = LeptosOptions::builder()
        .output_name("spike_ui_app")
        .site_root(dir.to_string_lossy().to_string())
        .site_pkg_dir("pkg")
        .build();
    let ctx_opts = options.clone();
    let leptos = leptos_axum::render_app_to_stream_with_context(
        move || provide_context(ctx_opts.clone()),
        move || shell(options.clone()),
    );
    Router::new()
        .route("/", get(|| async { Html(r#"<p><a href="/leptos">/leptos</a> · <a href="/templates">/templates</a></p>"#) }))
        .route("/leptos", get(leptos))
        .route("/leptos/editor", get(leptos_axum::render_app_to_stream(editor_shell)))
        .route("/templates", get(templates))
        .nest_service("/static", ServeDir::new(dir.join("static")))
        .nest_service("/pkg", ServeDir::new(dir.join("pkg")))
        .nest_service("/pkg-editor", ServeDir::new(dir.join("pkg-editor")))
}

/// leptos_axum needs a global executor before the first SSR request.
pub fn init_executor() {
    let _ = any_spawner::Executor::init_tokio();
}
