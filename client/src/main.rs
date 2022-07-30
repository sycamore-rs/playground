mod editor_view;
mod pastebin;

use std::error::Error;

use gloo_net::http::Request;
use gloo_storage::{LocalStorage, Storage};
use js_sys::Uint8Array;
use pastebin::new_paste;
use playground_common::{CompileRequest, CompileResponse};
use sycamore::futures::spawn_local_scoped;
use sycamore::prelude::*;
use sycamore::suspense::Suspense;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{HtmlDocument, HtmlIFrameElement, UrlSearchParams};

use crate::editor_view::EditorView;
use crate::pastebin::get_paste;

static BACKEND_URL: &str = if cfg!(debug_assertions) {
    "http://localhost:3000"
} else {
    "https://sycamore-playground.herokuapp.com"
};

static DEFAULT_EDITOR_CODE: &str = r#"use sycamore::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();

    sycamore::render(|cx|
        view! { cx, "Hello World!" }
    );
}
"#;

#[derive(Prop)]
struct NavBarProps<'a, F: FnMut() + 'a> {
    run: F,
    building: &'a ReadSignal<bool>,
    source: &'a ReadSignal<String>,
}

#[component]
fn NavBar<'a, G: Html>(cx: Scope<'a>, mut props: NavBarProps<'a, impl FnMut()>) -> View<G> {
    let share_modal_open = create_signal(cx, false);
    let share_gist_id = create_signal(cx, String::new());
    let share_pastebin_url = share_gist_id.map(cx, |id| {
        format!("https://gist.github.com/sycamore-playground/{id}")
    });
    let share_playground_url = share_gist_id.map(cx, |id| {
        format!("https://sycamore-rs.github.io/playground?gist={id}")
    });
    let share = move |_| {
        spawn_local_scoped(cx, async {
            let id = new_paste(&props.source.get())
                .await
                .expect("could not upload code snippet to gist");
            log::info!("Generated gist with id: {id}");
            share_modal_open.set(true);
            share_gist_id.set(id.to_string());
        });
    };

    view! { cx,
        nav(class="px-2 bg-gray-100 border-gray-300 border-b flex flex-row") {
            h1(class="inline-block text-xl py-1") {
                span(
                    class="font-extrabold bg-gradient-to-r from-orange-300 to-red-400 text-transparent bg-clip-text"
                ) { "Sycamore" }
                span(class="font-light") { " Playground" }
            }
            button(
                type="button",
                on:click=move |_| (props.run)(),
                disabled=*props.building.get(),
                class="px-5 my-1 ml-10 bg-green-400 font-bold text-white disabled:bg-green-200 rounded shadow-inner"
            ) { "Run" }
            div(class="grow")
            button(
                type="button",
                on:click=share,
                class="px-5 my-1 mr-5 bg-yellow-400 font-bold text-white rounded shadow-inner"
            ) { "Share" }
        }
        // Background dim.
        div(class=format!("fixed inset-0 w-full h-full z-40 bg-gray-500 bg-opacity-75 transition-opacity {}", if *share_modal_open.get() { "" } else { "hidden" }))
        // Share modal.
        div(
            class=format!("fixed inset-0 w-full z-50 {}", if *share_modal_open.get() { "" } else { "hidden" }),
            role="dialog",
            aria-modal=true,
        ) {
            // Modal content.
            div(class="bg-white container mx-auto mt-5 px-5 py-3 rounded shadow-lg") {
                h1(class="text-xl font-bold") { "Share" }
                p { "GitHub Gist: "
                    a(class="text-blue-600 underline", href=share_pastebin_url.get()) { (share_pastebin_url.get()) }
                }
                p { "Runnable playground: "
                    a(class="text-blue-600 underline", href=share_playground_url.get()) { (share_playground_url.get()) }
                }
                button(type="button", class="px-5 bg-yellow-400 font-bold text-white rounded shadow-inner", on:click=|_| share_modal_open.set(false)) { "Done" }
            }
        }
    }
}

async fn send_compile_req(code: &str) -> Result<CompileResponse<'_>, Box<dyn Error>> {
    let bytes = Request::post(&format!("{BACKEND_URL}/compile"))
        .json(&CompileRequest { code: code.into() })?
        .send()
        .await?
        .binary()
        .await?;
    // Deserialize into a `CompileResponse`.
    Ok(bincode::deserialize(&bytes)?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Preview {
    Initial,
    Building,
    ShowIFrame,
    ShowCompileError { err: String },
    ShowOtherError { err: String },
}

#[component]
fn Index<G: Html>(cx: Scope, initial_code: String) -> View<G> {
    let preview = create_signal(cx, Preview::Initial);
    let source = create_rc_signal(initial_code);
    let source_ref = create_ref(cx, source.clone());
    let iframe_ref = create_node_ref(cx);

    let run = move || {
        spawn_local_scoped(cx, async move {
            if *preview.get() != Preview::Building {
                preview.set(Preview::Building);
                let code = source_ref.get();
                let res = match send_compile_req(&code).await {
                    Ok(res) => res,
                    Err(err) => {
                        preview.set(Preview::ShowOtherError {
                            err: err.to_string(),
                        });
                        return;
                    }
                };

                match res {
                    CompileResponse::Success { js, wasm } => {
                        preview.set(Preview::ShowIFrame);
                        // Update iframe.
                        let iframe_src = format!(
                            r#"<!DOCTYPE html>
                        <html>
                            <head>
                                <meta content="text/html;charset=utf-8" http-equiv="Content-Type" />
                                <script type="module">
                                    {js}
                                    window.init = init;
                                </script>
                            </head>
                            <body>
                                <noscript>You need to enable Javascript to run this interactive app.</noscript>
                            </body>
                        </html>"#
                        );
                        let window = iframe_ref
                            .get::<DomNode>()
                            .unchecked_into::<HtmlIFrameElement>()
                            .content_window()
                            .unwrap();
                        let doc = window.document().unwrap().unchecked_into::<HtmlDocument>();
                        doc.open().unwrap();
                        doc.write(&JsValue::from(iframe_src).into()).unwrap();
                        doc.close().unwrap();
                        let buf = Uint8Array::from(&*wasm);
                        window.clone().set_onload(Some(
                            &Closure::once_into_js(move || {
                                let init = js_sys::Reflect::get(&window, &"init".into()).unwrap();
                                let init: js_sys::Function = init.unchecked_into();
                                init.call1(&window, &buf.into()).unwrap();
                            })
                            .unchecked_into(),
                        ));
                    }
                    CompileResponse::CompileError(err) => {
                        preview.set(Preview::ShowCompileError { err });
                    }
                };
            }
        });
    };

    // Save changes to code to local storage.
    create_effect(cx, || {
        LocalStorage::set("CODE", source_ref.get().as_ref())
            .expect("failed to save code to local storage");
    });

    view! { cx,
        NavBar { run, building: preview.map(cx, |p| p == &Preview::Building), source: source_ref }
        main(
            class="px-2 top-10 bottom-0 w-full absolute \
                grid grid-cols-1 grid-rows-2 md:grid-cols-2 md:grid-rows-1 \
                divide-y md:divide-y-0 md:divide-x divide-gray-400 space-y-2 md:space-x-2 \
                overflow-hidden"
        ) {
            EditorView {
                source,
            }
            div(class="block h-full w-full pb-2 overflow-auto") {
                (match preview.get().as_ref().clone() {
                    Preview::Initial => view! { cx,
                        div {
                            p {
                                "Press run to preview the app."
                            }
                        }
                    },
                    Preview::Building => view! { cx,
                        div {
                            p {
                                "Building app..."
                            }
                        }
                    },
                    Preview::ShowIFrame => view! { cx,
                        iframe(class="h-full w-full", title="preview", ref=iframe_ref)
                    },
                    Preview::ShowCompileError { err } => view! { cx,
                        div {
                            p {
                                "Compiler error."
                            }
                            pre { (err) }
                        }
                    },
                    Preview::ShowOtherError { err } => view! { cx,
                        div {
                            p {
                                "Other error."
                            }
                            pre { (err) }
                        }
                    },
                })
            }
        }
    }
}

#[component]
async fn App<G: Html>(cx: Scope<'_>) -> View<G> {
    // If we have a paste id in the query parameter, get the code from the pastebin.
    let url_params =
        UrlSearchParams::new_with_str(&web_sys::window().unwrap().location().search().unwrap())
            .unwrap();
    let initial_code = if let Some(gist_id) = url_params.get("gist") {
        let url = format!("{BACKEND_URL}/paste/{gist_id}");
        log::info!("Loading gist from {url}");
        get_paste(&url)
            .await
            .expect("could not fetch from pastebin")
    } else if let Some(_example_name) = url_params.get("example") {
        todo!("fetch example from github")
    } else {
        // Get saved code from local storage or initialize with default code.
        // We get the code before writing the new code to local storage in the effect below.
        let storage: String = LocalStorage::get("CODE").unwrap_or_else(|_| String::new());
        if storage.trim() == "" {
            DEFAULT_EDITOR_CODE.to_string()
        } else {
            storage
        }
    };

    view! { cx,
        Index(initial_code)
    }
}

fn main() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();

    sycamore::render(|cx| view! { cx, 
        Suspense {
            fallback: view!{ cx, "Loading..." },
            App {}
        }
    });
}
