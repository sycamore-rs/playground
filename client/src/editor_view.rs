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

#[derive(Prop)]
pub struct EditorViewProps {
    source: RcSignal<String>,
}

#[component]
pub fn EditorView<G: Html>(cx: Scope, props: EditorViewProps) -> View<G> {
    let source = create_ref(cx, props.source.clone());
    let editor_ref = create_node_ref(cx);
    let on_update = move |text| {
        props.source.set(text);
    };
    let on_update: Box<dyn FnMut(String)> = Box::new(on_update);
    let on_update = create_ref(cx, Closure::wrap(on_update));
    state_update(on_update);

    on_mount(cx, move || {
        init_editor(&editor_ref.get::<DomNode>().unchecked_into(), &source.get());
    });

    view! { cx,
        div(class="block h-full overflow-auto", ref=editor_ref)
    }
}
