use std::{cell::RefCell, rc::Rc};

use gloo_net::http::Request;
use sycamore::prelude::*;
use sycamore_playground_shared::{CompileRequest, CompileResponse, DEFAULT_CODE};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::spawn_local;

#[wasm_bindgen]
extern "C" {
    type CodeEditor;

    #[wasm_bindgen(js_namespace = PlaygroundEditor, js_name = mount)]
    fn mount_editor(
        host: &web_sys::HtmlElement,
        code: &str,
        on_run: &JsValue,
        on_change: &JsValue,
    ) -> CodeEditor;

    #[wasm_bindgen(js_namespace = PlaygroundEditor, js_name = getCode)]
    fn editor_code(editor: &CodeEditor) -> String;

    #[wasm_bindgen(js_namespace = PlaygroundEditor, js_name = destroy)]
    fn destroy_editor(editor: &CodeEditor);
}

fn main() {
    sycamore::render(App);
}

type EditorHandle = Rc<RefCell<Option<CodeEditor>>>;

#[component]
fn App() -> View {
    let editor = EditorHandle::default();
    let running = create_signal(false);
    let output = create_signal("Click Run to compile and run the code.".to_owned());
    let preview = create_signal(None::<String>);

    let run_editor = Rc::clone(&editor);
    let run = move || {
        if running.get() {
            return;
        }
        let Some(submitted_code) = run_editor.borrow().as_ref().map(editor_code) else {
            return;
        };
        running.set(true);
        // Remove the previous browsing context, including its timers and listeners.
        preview.set(None);
        output.set("Compiling…".to_owned());
        spawn_local(async move {
            let result = compile(submitted_code).await;
            match result {
                Ok(metadata) => {
                    if metadata.errors.is_empty() {
                        if let Some(artifact_url) = metadata.artifact_url.as_deref() {
                            preview.set(Some(preview_document(artifact_url)));
                        }
                    }
                    output.set(
                        serde_json::to_string_pretty(&metadata)
                            .unwrap_or_else(|error| format!("Could not display metadata: {error}")),
                    );
                }
                Err(error) => output.set(format!("Error: {error}")),
            }
            running.set(false);
        });
    };

    let header_run = run.clone();
    view! {
        main(
            class="flex min-h-dvh flex-col bg-white text-gray-900 md:h-dvh md:overflow-hidden dark:bg-gray-950 dark:text-gray-100",
        ) {
            Header(running=running, on_run=header_run)
            div(class="grid min-h-0 flex-1 grid-cols-1 md:grid-cols-2") {
                Editor(editor=editor, on_run=run)
                div(class="flex min-h-[55dvh] min-w-0 flex-col md:min-h-0") {
                    Output(preview=preview, running=running)
                    OutputMetadata(output=output)
                }
            }
        }
    }
}

#[component(inline_props)]
fn Header(running: Signal<bool>, on_run: impl Fn() + 'static) -> View {
    view! {
        header(
            class="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-gray-200 bg-gray-100 px-3 sm:px-4 dark:border-gray-800 dark:bg-gray-900",
        ) {
            div(class="flex min-w-0 items-center gap-2") {
                img(src="assets/sycamore-logo.svg", alt="", class="h-7 w-7 shrink-0")
                h1(class="truncate text-sm font-semibold tracking-tight") { "Sycamore Playground" }
            }
            button(
                r#type="button",
                on:click=move |_| on_run(),
                disabled=move || running.get(),
                title="Run (Ctrl+Enter)",
                aria-keyshortcuts="Control+Enter",
                class="inline-flex h-8 min-w-20 shrink-0 items-center justify-center gap-2 rounded-md bg-orange-700 px-3 text-xs font-semibold text-white transition-colors hover:bg-orange-800 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-orange-500 disabled:cursor-wait disabled:opacity-50 dark:bg-orange-500 dark:text-gray-950 dark:hover:bg-orange-400",
            ) {
                span(aria-hidden="true", class="text-xs") { "▶︎" }
                (if running.get() { "Running…" } else { "Run" })
            }
        }
    }
}

const CODE_STORAGE_KEY: &str = "sycamore-playground.code";

/// Returns browser localStorage when it is available and accessible.
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Restores saved code, using the default if storage is unavailable or the code is blank.
fn load_code() -> String {
    local_storage()
        .and_then(|storage| storage.get_item(CODE_STORAGE_KEY).ok().flatten())
        .filter(|code| !code.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CODE.to_owned())
}

/// Saves editor code to localStorage without propagating storage errors.
fn save_code(code: &str) {
    if let Some(storage) = local_storage() {
        // Blocked or full storage must not interrupt editing.
        let _ = storage.set_item(CODE_STORAGE_KEY, code);
    }
}

#[component(inline_props)]
fn Editor(editor: EditorHandle, on_run: impl Fn() + 'static) -> View {
    let editor_host = create_node_ref();
    on_mount(move || {
        let on_run = Closure::<dyn Fn()>::new(on_run);
        let on_change = Closure::<dyn Fn(String)>::new(|code: String| save_code(&code));
        let host = editor_host.get().unchecked_into::<web_sys::HtmlElement>();
        *editor.borrow_mut() = Some(mount_editor(
            &host,
            &load_code(),
            on_run.as_ref(),
            on_change.as_ref(),
        ));
        on_cleanup(move || {
            if let Some(editor) = editor.borrow_mut().take() {
                destroy_editor(&editor);
            }
            // Keep the Rust callbacks alive until the JS listeners are removed.
            drop((on_run, on_change));
        });
    });

    view! {
        section(
            class="flex h-[55dvh] min-h-0 min-w-0 flex-col border-b border-gray-200 md:h-auto md:border-r md:border-b-0 dark:border-gray-800",
        ) {
            h2(
                id="code-label",
                class="flex h-9 shrink-0 items-center border-b border-gray-200 bg-gray-50 px-4 text-xs font-medium text-gray-500 dark:border-gray-800 dark:bg-gray-900/50 dark:text-gray-400",
            ) { "Rust code" }
            div(r#ref=editor_host, class="editor-host min-h-0 min-w-0 flex-1 overflow-hidden")
        }
    }
}

#[component(inline_props)]
fn Output(preview: Signal<Option<String>>, running: Signal<bool>) -> View {
    view! {
        section(class="flex min-h-72 flex-1 flex-col md:min-h-0") {
            h2(
                class="flex h-9 shrink-0 items-center border-b border-gray-200 bg-gray-50 px-4 text-xs font-medium text-gray-500 dark:border-gray-800 dark:bg-gray-900/50 dark:text-gray-400",
            ) { "Output" }
            (match preview.get_clone() {
                Some(document) => {
                    view! {
                        iframe(
                            title="Playground output",
                            "sandbox"="allow-scripts",
                            referrerpolicy="no-referrer",
                            srcdoc=document,
                            class="min-h-0 w-full flex-1 border-0 bg-white",
                        )
                    }
                }
                None => {
                    view! {
                        div(
                            class="flex flex-1 items-center justify-center px-6 py-12 text-center text-sm text-gray-400 dark:text-gray-500",
                        ) {
                            (if running.get() {
                                "Compiling your app…"
                            } else {
                                "Run your code to see the result."
                            })
                        }
                    }
                }
            })
        }
    }
}

#[component(inline_props)]
fn OutputMetadata(output: Signal<String>) -> View {
    view! {
        section(class="shrink-0 border-t border-gray-200 dark:border-gray-800") {
            h2(
                class="flex h-9 items-center border-b border-gray-200 bg-gray-50 px-4 text-xs font-medium text-gray-500 dark:border-gray-800 dark:bg-gray-900/50 dark:text-gray-400",
            ) { "Output metadata" }
            pre(
                aria-live="polite",
                class="max-h-48 overflow-auto whitespace-pre-wrap break-words bg-gray-50/50 px-4 py-3 font-mono text-xs leading-relaxed text-gray-600 md:max-h-40 dark:bg-gray-900/30 dark:text-gray-400",
            ) { (output.get_clone()) }
        }
    }
}

fn compile_url(base_url: &str) -> String {
    format!("{}/compile", base_url.trim_end_matches('/'))
}

async fn compile(code: String) -> Result<CompileResponse, String> {
    // Embedded at build time; absent or empty keeps local requests same-origin.
    let url = compile_url(option_env!("BACKEND_BASE_URL").unwrap_or(""));
    let request = Request::post(&url)
        .json(&CompileRequest { code })
        .map_err(|error| format!("Could not encode compile request: {error}"))?;
    let response = request
        .send()
        .await
        .map_err(|error| format!("Could not reach the compilation server: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("HTTP {status}; could not read the response: {error}"))?;
    // Compile failures use the same JSON schema as successes. Keep a fallback
    // for non-JSON errors from proxies or request validation.
    let metadata = serde_json::from_str::<CompileResponse>(&body).map_err(|error| {
        if response.ok() {
            format!("Invalid compilation metadata: {error}")
        } else {
            format!("HTTP {status}\n{body}")
        }
    })?;
    if !response.ok() {
        return Err(format!(
            "HTTP {status}\n{}",
            serde_json::to_string_pretty(&metadata).unwrap_or(body)
        ));
    }
    Ok(metadata)
}

/// Encode data for an inline script without allowing HTML parser breakouts.
fn script_string(value: &str) -> String {
    serde_json::to_string(value)
        .expect("strings can be serialized")
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn preview_document(artifact_url: &str) -> String {
    let backend = compile_url(option_env!("BACKEND_BASE_URL").unwrap_or(""));
    include_str!("preview.html")
        .replace("__COMPILE_URL__", &script_string(&backend))
        .replace("__ARTIFACT_URL__", &script_string(artifact_url))
}

#[cfg(test)]
mod tests {
    use super::{compile_url, preview_document, script_string};

    #[test]
    fn preview_includes_artifact_and_initializes_wasm() {
        let document = preview_document("/artifacts/test.wasm");
        assert!(document.contains("new URL(\"/artifacts/test.wasm\", backend)"));
        assert!(document.contains("await import(moduleUrl.href)"));
        assert!(document.contains("await init({ module_or_path:"));
        assert!(!document.contains("__COMPILE_URL__"));
        assert!(!document.contains("__ARTIFACT_URL__"));
    }

    #[test]
    fn artifact_url_cannot_break_out_of_inline_script() {
        let url = "</script><script>alert('injection')</script>\u{2028}&";
        let encoded = script_string(url);
        assert!(!encoded.contains('<'));
        assert!(!encoded.contains('&'));
        assert!(!encoded.contains('\u{2028}'));
        assert_eq!(serde_json::from_str::<String>(&encoded).unwrap(), url);
        assert_eq!(preview_document(url).matches("</script>").count(), 1);
    }

    #[test]
    fn compile_url_defaults_to_same_origin() {
        assert_eq!(compile_url(""), "/compile");
    }

    #[test]
    fn compile_url_uses_backend_base_url() {
        assert_eq!(
            compile_url("https://backend.example.com"),
            "https://backend.example.com/compile"
        );
        assert_eq!(
            compile_url("https://backend.example.com/"),
            "https://backend.example.com/compile"
        );
    }
}
