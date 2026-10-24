//! pf-twz8 UI spike, Leptos 0.8 side: the part-view bar and the share sheet,
//! SSR on the server (`ssr`) and hydrated in the browser (`hydrate`).

use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rung {
    OnlyMe,
    People,
    Group,
    SignedIn,
    Public,
}

impl Rung {
    pub const ALL: [Rung; 5] = [Rung::OnlyMe, Rung::People, Rung::Group, Rung::SignedIn, Rung::Public];
    pub fn label(self) -> &'static str {
        match self {
            Rung::OnlyMe => "Only me",
            Rung::People => "People",
            Rung::Group => "A group",
            Rung::SignedIn => "Everyone signed in",
            Rung::Public => "Anyone with the link",
        }
    }
    pub fn chip(self) -> &'static str {
        match self {
            Rung::OnlyMe => "Only me",
            Rung::People => "3 people",
            Rung::Group => "Family",
            Rung::SignedIn => "Everyone signed in · 15",
            Rung::Public => "Public link",
        }
    }
}

/// The live sentence. Shared logic: the templates island reimplements this in JS.
pub fn sentence(rung: Rung, contribute: bool) -> String {
    match rung {
        Rung::OnlyMe => "Only you can see this.".into(),
        Rung::People => "You and 3 people can see this. A family member can also edit.".into(),
        Rung::Group => "You and the Family group (6) can see this.".into(),
        Rung::SignedIn => "Everyone signed in (15) can see this.".into(),
        Rung::Public if contribute => "Anyone with the link can open and use this, without signing in. Only you and people you add can change it.".into(),
        Rung::Public => "Anyone with the link can open this, without signing in.".into(),
    }
}

#[component]
pub fn PartBar(rung: ReadSignal<Rung>, set_open: WriteSignal<bool>) -> impl IntoView {
    let (menu, set_menu) = signal(false);
    view! {
        <header class="bar">
            <a class="back" href="/">"‹ Parts"</a>
            <span class="tag">"P-0142 TSX"</span>
            <span class="title">"Photo finder"</span>
            <code class="name">"photofinder"</code>
            <button class="rev">"REV 12 ▾"</button>
            <span class="activity"><i class="pulse"></i>"Hermes is editing app.tsx"</span>
            <span class="spacer"></span>
            <button class="chip" on:click=move |_| set_open.set(true)>{move || rung.get().chip()}</button>
            <button class="share" on:click=move |_| set_open.set(true)>"Share"</button>
            <a class="edit" href="/edit">"Edit"</a>
            <button class="comments">"Comments 3"</button>
            <button class="more" on:click=move |_| set_menu.update(|m| *m = !*m)>"⋯"</button>
            <Show when=move || menu.get()>
                <ul class="menu">
                    <li>"Open standalone ↗"</li>
                    <li>"Copy link"</li>
                    <li>"Details"</li>
                </ul>
            </Show>
        </header>
    }
}

#[component]
pub fn ShareSheet(
    rung: ReadSignal<Rung>,
    set_rung: WriteSignal<Rung>,
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
) -> impl IntoView {
    let (contribute, set_contribute) = signal(false);
    let (copied, set_copied) = signal(false);
    // Fake in-sheet step-up: idle -> asking -> confirmed, no navigation.
    let (step, set_step) = signal(0u8);
    let url = "https://photofinder-part.example.com/";
    let copy = move |_| {
        #[cfg(feature = "hydrate")]
        {
            let _ = window().navigator().clipboard().write_text(url);
        }
        set_copied.set(true);
        set_timeout(move || set_copied.set(false), std::time::Duration::from_millis(1500));
    };
    let confirm = move |_| {
        set_step.set(1);
        set_timeout(move || set_step.set(2), std::time::Duration::from_millis(800));
    };
    view! {
        <div class="sheet" class:open=move || open.get() role="dialog" aria-label="Share">
            <h2>"Share " <em>"Photo finder"</em> <span class="tag">"P-0142"</span>
                <button class="close" on:click=move |_| set_open.set(false)>"×"</button></h2>
            <div class="ladder" role="radiogroup">
                {Rung::ALL.into_iter().map(|r| view! {
                    <button role="radio"
                        class:on=move || rung.get() == r
                        class:amber=r == Rung::Public
                        aria-checked=move || (rung.get() == r).to_string()
                        on:click=move |_| { set_rung.set(r); set_step.set(0); }>
                        {r.label()}
                    </button>
                }).collect_view()}
            </div>
            <p class="sentence">{move || sentence(rung.get(), contribute.get())}</p>
            <Show when=move || rung.get() == Rung::Public>
                <p class="warn">"The worst option: no sign-in, anyone who gets the link can open it."</p>
                <div class="mode" role="radiogroup">
                    <button class:on=move || !contribute.get() on:click=move |_| set_contribute.set(false)>"Can view"</button>
                    <button class:on=move || contribute.get() on:click=move |_| set_contribute.set(true)>"Can contribute"</button>
                </div>
                <div class="url"><code>{url}</code>
                    <button on:click=copy>{move || if copied.get() { "Copied" } else { "Copy" }}</button></div>
                {move || match step.get() {
                    0 => view! { <button class="stepup" on:click=confirm>"Turn on public link"</button> }.into_any(),
                    1 => view! { <p class="stepup">"Confirm it's you… " <span class="spin"></span></p> }.into_any(),
                    _ => view! { <p class="stepup ok">"Public link is on."</p> }.into_any(),
                }}
            </Show>
            <p class="foot">"Changes save as you go."</p>
        </div>
    }
}

#[component]
pub fn App() -> impl IntoView {
    let (rung, set_rung) = signal(Rung::Group);
    let (open, set_open) = signal(false);
    view! {
        <PartBar rung set_open />
        <ShareSheet rung set_rung open set_open />
        <main class="frame"><p>"the part, full-bleed, sandboxed on its own host"</p></main>
    }
}

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}

// ------------------------------------------------------------ CodeMirror probe

#[cfg(all(feature = "hydrate", feature = "editor"))]
mod cm {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(module = "/static/editor-glue.js")]
    extern "C" {
        #[wasm_bindgen(js_name = mountEditor)]
        pub fn mount_editor(parent: &JsValue, doc: &str, on_change: &Closure<dyn FnMut(String)>) -> JsValue;
        #[wasm_bindgen(js_name = editorInsert)]
        pub fn editor_insert(view: &JsValue, text: &str);
    }
}

/// A Leptos component that owns a CodeMirror 6 view: Leptos -> editor (Insert)
/// and editor -> Leptos (the live length) both cross the wasm-bindgen boundary.
#[component]
pub fn EditorProbe() -> impl IntoView {
    let initial = "<h1>Packing list</h1>\n<ul>\n  <li>passports</li>\n</ul>\n";
    let (len, set_len) = signal(initial.len());
    let _ = set_len; // only used by the hydrate+editor build
    let host = NodeRef::<leptos::html::Div>::new();
    // JsValue is !Send, so the view handle needs a local-only store.
    let view_handle = StoredValue::new_local(None::<wasm_bindgen_shim::Js>);
    #[cfg(all(feature = "hydrate", feature = "editor"))]
    Effect::new(move |_| {
        if let Some(el) = host.get() {
            let cb = wasm_bindgen::closure::Closure::<dyn FnMut(String)>::new(move |s: String| set_len.set(s.len()));
            let v = cm::mount_editor(el.as_ref(), initial, &cb);
            cb.forget();
            view_handle.set_value(Some(v));
        }
    });
    let insert = move |_| {
        #[cfg(all(feature = "hydrate", feature = "editor"))]
        view_handle.with_value(|v| if let Some(v) = v { cm::editor_insert(v, "<li>chargers</li>\n") });
        let _ = view_handle;
    };
    view! {
        <div class="probe">
            <p>"Editor length (from CodeMirror, via a Leptos signal): " <b class="len">{move || len.get()}</b></p>
            <button class="insert" on:click=insert>"Insert from Leptos"</button>
            <div class="cm" node_ref=host></div>
        </div>
    }
}

#[cfg(all(feature = "hydrate", feature = "editor"))]
mod wasm_bindgen_shim {
    pub type Js = wasm_bindgen::JsValue;
}
#[cfg(not(all(feature = "hydrate", feature = "editor")))]
mod wasm_bindgen_shim {
    pub type Js = ();
}

#[cfg(all(feature = "hydrate", feature = "editor"))]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate_editor() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(EditorProbe);
}
