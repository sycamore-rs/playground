use gloo_net::http::Request;
use serde::Serialize;
use sycamore::futures::spawn_local_scoped;
use sycamore::prelude::*;
use wasm_bindgen::prelude::*;
use web_sys::Node;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "initEditor")]
    fn init_editor(elem: &Node);

    #[wasm_bindgen(js_name = "getCode")]
    fn get_code() -> String;
}

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
    let srcdoc = create_signal(cx, String::new());
    let running = create_signal(cx, false);
    let editor_ref = create_node_ref(cx);

    let run = move || {
        spawn_local_scoped(cx, async {
            if !*running.get() {
                running.set(true);
                let html = Request::post(&format!("{BACKEND_URL}/compile"))
                    .json(&CompileReq { code: get_code() })
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

    spawn_local_scoped(cx, async {
        init_editor(&editor_ref.get::<DomNode>().unchecked_into());
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
                div(class="block flex-1", ref=editor_ref)
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
