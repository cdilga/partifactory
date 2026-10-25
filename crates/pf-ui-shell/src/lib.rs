//! The workspace has a hydrated Leptos shell; the untrusted part wrapper stays framework-free.
use leptos::prelude::*;

#[component]
pub fn Shell(user: String, query: String, admin: bool, body: String) -> impl IntoView {
    let avatar = user
        .chars()
        .next()
        .unwrap_or('A')
        .to_uppercase()
        .to_string();
    let theme = move |_| {
        #[cfg(feature = "hydrate")]
        if let Some(w) = web_sys::window() {
            let root = w.document().and_then(|d| d.document_element());
            if let Some(root) = root {
                let current = root.get_attribute("data-theme");
                let dark = current.as_deref() == Some("dark")
                    || current.is_none()
                        && w.match_media("(prefers-color-scheme: dark)")
                            .ok()
                            .flatten()
                            .is_some_and(|m| m.matches());
                let next = if dark { "light" } else { "dark" };
                let _ = root.set_attribute("data-theme", next);
                if let Ok(Some(storage)) = w.local_storage() {
                    let _ = storage.set_item("pf:theme", next);
                }
            }
        }
    };
    view! {
        <div id="pf-app" data-user=user data-query=query.clone() data-admin=admin.to_string()>
            <header class="topbar">
                <a class="brand" href="/">
                    <svg class="mark" viewBox="0 0 24 24" aria-hidden="true"><rect x="2" y="2" width="20" height="20" rx="3"/><path d="M6 7h12v5H6zM9 17h6"/></svg>
                    "partifactory"
                </a>
                <form class="search" method="get"><input id="search" name="q" aria-label="Search parts" placeholder="Search parts  /" value=query.clone()/></form>
                <button id="theme" aria-label="Toggle theme" on:click=theme>"◐"</button>
                <details><summary aria-label="Account menu">{avatar}</summary><div class="menu">
                    {admin.then(||view!{<a href="/admin/links">"Public links"</a>})}
                    <a href="/#connect">"Connect an agent"</a>
                    <button data-signout="/signout" on:click=move |_| signout("/signout")>"Sign out"</button>
                    <button data-signout="/signout/everywhere" on:click=move |_| signout("/signout/everywhere")>"Sign out everywhere"</button>
                </div></details>
            </header>
            <div id="pf-gallery" inner_html=body></div>
        </div>
    }
}
fn signout(path: &str) {
    #[cfg(feature = "hydrate")]
    if let Some(w) = web_sys::window() {
        let init = web_sys::RequestInit::new();
        init.set_method("POST");
        let headers = web_sys::Headers::new().expect("headers");
        let _ = headers.set("Content-Type", "application/json");
        init.set_headers(&headers);
        init.set_body(&wasm_bindgen::JsValue::from_str("{}"));
        let promise = w.fetch_with_str_and_init(path, &init);
        leptos::task::spawn_local(async move {
            if wasm_bindgen_futures::JsFuture::from(promise).await.is_ok() {
                let _ = w.location().replace("/");
            }
        });
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = path;
}

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    let w = web_sys::window().expect("window");
    let doc = w.document().expect("document");
    let Some(root) = doc.get_element_by_id("pf-app") else {
        return;
    };
    let user = root.get_attribute("data-user").unwrap_or_default();
    let query = root.get_attribute("data-query").unwrap_or_default();
    let admin = root.get_attribute("data-admin").as_deref() == Some("true");
    let body = doc
        .get_element_by_id("pf-gallery")
        .map(|e| e.inner_html())
        .unwrap_or_default();
    if let Ok(Some(s)) = w.local_storage() {
        if let Ok(Some(theme)) = s.get_item("pf:theme") {
            if let Some(html) = doc.document_element() {
                let _ = html.set_attribute("data-theme", &theme);
            }
        }
    }
    leptos::mount::hydrate_body(
        move || view! {<Shell user=user query=query admin=admin body=body/>},
    );
    if let Some(html) = doc.document_element() {
        let _ = html.set_attribute("data-pf-hydrated", "1");
    }
}
