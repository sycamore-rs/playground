use gloo_net::http::Request;
use gloo_storage::{LocalStorage, Storage};
use serde::Serialize;
use sycamore::futures::spawn_local_scoped;
use sycamore::prelude::*;
use wasm_bindgen::prelude::*;
use web_sys::Node;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "stateUpdate")]
    fn state_update(cb: &Closure<dyn FnMut(String)>);

    #[wasm_bindgen(js_name = "initEditor")]
    fn init_editor(elem: &Node, doc: &str);

    #[wasm_bindgen(js_name = "getCode")]
    fn get_code() -> String;
}

static BACKEND_URL: &str = "https://sycamore-playground.herokuapp.com";

static DEFAULT_EDITOR_CODE: &str = r#"use sycamore::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();

    sycamore::render(|cx|
        view! { cx, "Hello World!" }
    );
}
"#;

#[derive(Serialize)]
struct CompileReq {
    code: String,
}

#[derive(Prop)]
struct NavBarProps<'a> {
    run: Box<dyn FnMut() + 'a>,
    building: &'a ReadSignal<bool>,
}

#[component]
fn NavBar<'a, G: Html>(cx: Scope<'a>, mut props: NavBarProps<'a>) -> View<G> {
    view! { cx,
        nav(class="bg-gray-100 px-2 border-b border-gray-300") {
            h1(class="inline-block text-xl py-1") {
                span(
                    class="font-extrabold bg-gradient-to-r from-orange-300 to-red-400 text-transparent bg-clip-text"
                ) { "Sycamore" }
                span(class="font-light") { " Playground" }
            }

            button(
                class="inline-block ml-5 px-3 bg-green-400 rounded font-bold text-white disabled:bg-green-200",
                on:click=move |_| (props.run)(),
                disabled=*props.building.get()
            ) { "Run" }
        }
    }
}

#[component]
fn App<G: Html>(cx: Scope) -> View<G> {
    let srcdoc = create_signal(
        cx,
        "Press the \"Run\" button to preview the app.".to_string(),
    );
    let building = create_signal(cx, false);
    let editor_ref = create_node_ref(cx);
    let source = create_rc_signal(String::new());
    let source_ref = create_ref(cx, source.clone());

    let on_update = move |text| {
        source.set(text);
    };
    let on_update: Box<dyn FnMut(String)> = Box::new(on_update);
    let on_update = create_ref(cx, Closure::wrap(on_update));
    state_update(on_update);

    let run = move || {
        spawn_local_scoped(cx, async {
            if !*building.get() {
                building.set(true);
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
                building.set(false);
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

    // Save changes to code to local storage.
    create_effect(cx, || {
        LocalStorage::set("CODE", source_ref.get().as_ref())
            .expect("failed to save code to local storage");
    });

    spawn_local_scoped(cx, async move {
        init_editor(&editor_ref.get::<DomNode>().unchecked_into(), &code);
    });

    view! { cx,
        NavBar { run: Box::new(run), building }
        main(class="px-2 flex w-full absolute top-10 bottom-0 divide-x divide-gray-400 space-x-2") {
            div(class="flex flex-col flex-1") {
                div(class="block flex-1", ref=editor_ref)
            }
            div(class="flex flex-col flex-1 {}") {
                // Loading bar container
                div(class="h-0.5 w-full")
                // Preview
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
