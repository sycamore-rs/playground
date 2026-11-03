// The Rust UI will mount this editor through a wasm-bindgen bridge.
// Keep the editor dependencies in one self-hosted, Trunk-fingerprinted bundle.
export { EditorView, basicSetup } from "codemirror";
export { rust } from "@codemirror/lang-rust";
