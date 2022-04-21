use gloo_net::http::Request;
use serde::Serialize;
use sycamore::futures::spawn_local_scoped;
use sycamore::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Event, HtmlElement, KeyboardEvent};

static BACKEND_URL: &str = "https://sycamore-playground.herokuapp.com";

#[derive(Serialize)]
struct CompileReq {
    code: String,
}

#[component]
fn NavBar<G: Html>(cx: Scope) -> View<G> {
    view! { cx,
        nav(class="bg-orange-400 h-10") {
            h1(class="text-white text-lg font-bold") { "Sycamore Playground" }
        }
    }
}

#[component]
fn App<G: Html>(cx: Scope) -> View<G> {
    let code = create_signal(cx, String::new());
    let srcdoc = create_signal(cx, String::new());
    let running = create_signal(cx, false);
    let textarea_ref = create_node_ref(cx);

    let run = move || {
        spawn_local_scoped(cx, async {
            if !*running.get() {
                running.set(true);
                let html = Request::post(&format!("{BACKEND_URL}/compile"))
                    .json(&CompileReq {
                        code: code.get().as_ref().clone(),
                    })
                    .unwrap()
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();

                srcdoc.set(html);
                running.set(false);
            }
        });
    };

    let keydown = move |e: Event| {
        let e = e.unchecked_into::<KeyboardEvent>();
        if e.ctrl_key() && e.key() == "Enter" {
            run();
        }
    };

    spawn_local_scoped(cx, async {
        // FIXME: set spellcheck directly on the textarea element.
        textarea_ref
            .get::<DomNode>()
            .unchecked_into::<HtmlElement>()
            .set_spellcheck(false);
    });

    view! { cx,
        NavBar {}
        main(class="px-2 flex w-full absolute top-10 bottom-0 divide-x divide-gray-400 space-x-2") {
            div(class="flex flex-col flex-1") {
                div {
                    h2(class="font-bold text-lg inline") { "Code" }
                    button(
                        class="inline ml-5 px-3 bg-green-400 rounded font-bold text-white disabled:bg-green-200",
                        on:click=move |_| run(),
                        disabled=*running.get()
                    ) { "Run" }
                }
                textarea(
                    class="block flex-1 rounded p-1 bg-slate-200 focus-visible:outline-none font-mono",
                    bind:value=code,
                    placeholder="Enter code here...",
                    on:keydown=keydown,
                    ref=textarea_ref,
                )
            }
            div(class="flex flex-col flex-1") {
                div {
                    h2(class="font-bold text-lg") { "Preview" }
                }
                iframe(class="block flex-1", srcdoc=srcdoc.get())
            }
        }
    }
}

fn main() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();

    sycamore::render(|cx| view! { cx, App {} });
}
