import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { basicSetup } from "@codemirror/basic-setup";
import { indentWithTab } from "@codemirror/commands";
import { indentUnit } from "@codemirror/language";
import { rust } from "@codemirror/lang-rust";

/**
 * @type {EditorView}
 */
let view;

const updateHandlers = [];
window.stateUpdate = (f) => updateHandlers.push(f);

window.initEditor = (elem, doc) => {
  let state = EditorState.create({
    doc,
    extensions: [
      basicSetup,
      rust(),
      keymap.of([indentWithTab]),
      indentUnit.of("    "),
      EditorView.updateListener.of((update) => {
        let text = update.state.doc.sliceString(0);
        for (const f of updateHandlers) {
          f(text);
        }
      }),
    ],
  });

  view = new EditorView({
    state,
    parent: elem,
  });
};

window.getCode = () => view.state.doc.sliceString(0);

window.getState = () => view.state;
