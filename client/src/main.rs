mod editor_view;

use gloo_net::http::Request;
use gloo_storage::{LocalStorage, Storage};
use serde::Serialize;
use sycamore::futures::spawn_local_scoped;
use sycamore::prelude::*;

use crate::editor_view::EditorView;

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
struct CompileReq<'a> {
    code: &'a str,
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
    let srcdoc = create_signal(cx, String::new());
    let building = create_signal(cx, false);
    let show_first_run = create_signal(cx, true);
    let source = create_rc_signal(String::new());
    let source_ref = create_ref(cx, source.clone());

    let run = move || {
        spawn_local_scoped(cx, async {
            if !*building.get() {
                building.set(true);
                let html = Request::post(&format!("{BACKEND_URL}/compile"))
                    .json(&CompileReq {
                        code: &source_ref.get(),
                    })
                    .unwrap()
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();

                srcdoc.set(html);
                building.set(false);
                if *show_first_run.get() {
                    show_first_run.set(false);
                }
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
        NavBar { run: Box::new(run), building }
        main(class="px-2 flex w-full absolute top-10 bottom-0 divide-x divide-gray-400 space-x-2") {
            div(class="flex flex-col flex-1") {
                EditorView {
                    source,
                }
            }
            div(class="flex flex-col flex-1 {}") {
                // Preview
                (if *show_first_run.get() {
                    view! { cx,
                        "Press the \"Run\" button to preview the app."
                    }
                } else {
                    view! { cx,
                        iframe(class="block flex-1", srcdoc=srcdoc.get())
                    }
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
