use gloo_net::http::Request;
use serde::Serialize;
use sycamore::{futures::spawn_local_scoped, prelude::*, rt::JsCast};
use web_sys::HtmlIFrameElement;

static BACKEND_URL: &str = "https://sycamore-playground.herokuapp.com";

#[derive(Serialize)]
struct CompileReq {
    code: String,
}

#[component]
fn App<G: Html>(cx: Scope) -> View<G> {
    let code = create_signal(cx, String::new());
    let iframe = create_node_ref(cx);

    let run = move |_| {
        spawn_local_scoped(cx, async {
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

            iframe
                .get::<DomNode>()
                .inner_element()
                .unchecked_into::<HtmlIFrameElement>()
                .set_srcdoc(&html);
        });
    };

    view! { cx,
        h1 { "Sycamore Playground" }
        div {
            textarea(bind:value=code)
            button(on:click=run) { "Run" }
        }
        div {
            h2 { "Preview" }
            iframe(ref=iframe)
        }
    }
}

fn main() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();

    sycamore::render(|cx| view! { cx, App {} });
}
