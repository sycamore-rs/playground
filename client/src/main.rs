use sycamore::prelude::*;

fn main() {
    sycamore::render(|| {
        view! {
            div {
                h1 { "Sycamore Playground" }
                p { "This is a simple Sycamore application." }
            }
        }
    })
}
