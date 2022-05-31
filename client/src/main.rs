mod editor_view;

use std::error::Error;

use gloo_net::http::Request;
use gloo_storage::{LocalStorage, Storage};
use js_sys::Uint8Array;
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
struct RunButtonProps<'a, F: FnMut() + 'a> {
    run: F,
    building: &'a ReadSignal<bool>,
}

#[component]
fn RunButton<'a, G: Html>(cx: Scope<'a>, mut props: RunButtonProps<'a, impl FnMut()>) -> View<G> {
    view! { cx,
        button(
            class="inline-block ml-5 px-3 bg-green-400 rounded font-bold text-white disabled:bg-green-200",
            on:click=move |_| (props.run)(),
            disabled=*props.building.get()
        ) { "Run" }
    }
}

#[derive(Prop)]
struct NavBarProps<'a, F: FnMut() + 'a> {
    run: F,
    building: &'a ReadSignal<bool>,
}

#[component]
fn NavBar<'a, G: Html>(cx: Scope<'a>, props: NavBarProps<'a, impl FnMut()>) -> View<G> {
    view! { cx,
        nav(class="bg-gray-100 px-2 border-b border-gray-300") {
            h1(class="inline-block text-xl py-1") {
                span(
                    class="font-extrabold bg-gradient-to-r from-orange-300 to-red-400 text-transparent bg-clip-text"
                ) { "Sycamore" }
                span(class="font-light") { " Playground" }
            }

            RunButton { run: props.run, building: props.building }
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
fn App<G: Html>(cx: Scope) -> View<G> {
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
        NavBar { run, building: preview.map(cx, |p| p == &Preview::Building) }
        main(class="px-2 flex w-full absolute top-10 bottom-0 divide-x divide-gray-400 space-x-2") {
            div(class="flex flex-col flex-1") {
                EditorView {
                    source,
                }
            }
            div(class="flex flex-col flex-1 {}") {
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
                        iframe(class="block flex-1", ref=iframe_ref)
                    },
                    Preview::ShowCompileError { err } => view! { cx,
                        div {
                            p {
                                "Compile error."
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

fn main() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();

    sycamore::render(|cx| view! { cx, App {} });
}
