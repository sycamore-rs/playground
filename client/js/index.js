import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { basicSetup } from "@codemirror/basic-setup";
import { indentWithTab } from "@codemirror/commands";
import { rust } from "@codemirror/lang-rust";

let state;

window.initEditor = (elem) => {
  state = EditorState.create({
    doc: 'use sycamore::prelude::*;\n\nfn main() {\n    sycamore::render(|cx| view! { cx,\n        "Hello World!"\n    });\n}',
    extensions: [basicSetup, rust(), keymap.of([indentWithTab])],
  });

  new EditorView({
    state,
    parent: elem,
  });
};

window.getCode = () => state.doc.sliceString(0);
