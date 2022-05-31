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
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{HtmlDocument, HtmlIFrameElement};

use crate::editor_view::EditorView;

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
    let share = move |_| {
        spawn_local_scoped(cx, async {
            let url = new_paste(&props.source.get()).await.unwrap();
            log::info!("{url}");
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
                class="px-5 -my-px ml-10 bg-green-400 font-bold text-white disabled:bg-green-200"
            ) { "Run" }
            div(class="grow")
            button(
                type="button",
                on:click=share,
                class="px-5 -my-px mr-5 bg-yellow-400 font-bold text-white"
            ) { "Share" }
        }
    }
}

async fn send_compile_req(code: &str) -> Result<CompileResponse, Box<dyn Error>> {
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
fn Index<G: Html>(cx: Scope) -> View<G> {
    let preview = create_signal(cx, Preview::Initial);
    let source = create_rc_signal(String::new());
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

    // Get saved code from local storage or initialize with default code.
    // We get the code before writing the new code to local storage in the effect below.
    let code: String = LocalStorage::get("CODE").unwrap_or_else(|_| String::new());
    let code = if code.trim() == "" {
        DEFAULT_EDITOR_CODE.to_string()
    } else {
        code
    };
    source.set(code);

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
fn App<G: Html>(cx: Scope) -> View<G> {
    view! { cx,
        Index {}
    }
}

fn main() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();

    sycamore::render(|cx| view! { cx, App {} });
}
