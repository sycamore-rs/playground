mod console;

use std::{cell::RefCell, rc::Rc};

use console::{ConsoleEntry, ConsoleWindow, PreviewFrame};

use gloo_net::http::Request;
use sycamore::prelude::*;
use sycamore_playground_shared::{
    CompileRequest, CompileResponse, CompilerDiagnostic, DEFAULT_CODE,
};
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

    #[wasm_bindgen(js_namespace = PlaygroundEditor, js_name = goToLocation)]
    fn go_to_location(editor: &CodeEditor, line: u32, column: u32);

    #[wasm_bindgen(js_namespace = PlaygroundEditor, js_name = destroy)]
    fn destroy_editor(editor: &CodeEditor);
}

fn main() {
    sycamore::render(App);
}

type EditorHandle = Rc<RefCell<Option<CodeEditor>>>;

#[derive(Clone)]
enum CompilationState {
    Idle,
    Compiling,
    Complete(CompileResponse),
    Failed(String),
}

#[component]
fn App() -> View {
    let editor = EditorHandle::default();
    let running = create_signal(false);
    let output = create_signal(CompilationState::Idle);
    let preview = create_signal(None::<String>);
    let console_entries = create_signal(Vec::<ConsoleEntry>::new());
    let expand_diagnostics = create_memo(move || {
        preview.with(Option::is_none)
            && output.with(|state| match state {
                CompilationState::Complete(metadata) => !metadata.errors.is_empty(),
                CompilationState::Failed(_) => true,
                _ => false,
            })
    });

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
        console_entries.set(Vec::new());
        output.set(CompilationState::Compiling);
        spawn_local(async move {
            let result = compile(submitted_code).await;
            match result {
                Ok(metadata) => {
                    if metadata.errors.is_empty() {
                        if let Some(artifact_url) = metadata.artifact_url.as_deref() {
                            preview.set(Some(preview_document(artifact_url)));
                        }
                    }
                    output.set(CompilationState::Complete(metadata));
                }
                Err(error) => output.set(CompilationState::Failed(error)),
            }
            running.set(false);
        });
    };

    let header_run = run.clone();
    let metadata_editor = Rc::clone(&editor);
    view! {
        main(
            class="flex min-h-dvh flex-col bg-white text-gray-900 md:h-dvh md:overflow-hidden dark:bg-gray-950 dark:text-gray-100",
        ) {
            Header(running=running, on_run=header_run)
            div(class="grid min-h-0 flex-1 grid-cols-1 md:grid-cols-2") {
                Editor(editor=editor, on_run=run)
                div(class="flex min-h-[55dvh] min-w-0 flex-col md:min-h-0") {
                    Output(preview=preview, running=running, expanded=expand_diagnostics, console_entries=console_entries)
                    ConsoleWindow(entries=console_entries, preview=preview)
                    OutputMetadata(output=output, expanded=expand_diagnostics, editor=metadata_editor)
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
                data-umami-event="Run",
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
fn Output(
    preview: Signal<Option<String>>,
    running: Signal<bool>,
    expanded: ReadSignal<bool>,
    console_entries: Signal<Vec<ConsoleEntry>>,
) -> View {
    view! {
        section(class=move || if expanded.get() {
            "flex shrink-0 flex-col"
        } else {
            "flex min-h-72 flex-1 flex-col md:min-h-0"
        }) {
            h2(
                class="flex h-9 shrink-0 items-center border-b border-gray-200 bg-gray-50 px-4 text-xs font-medium text-gray-500 dark:border-gray-800 dark:bg-gray-900/50 dark:text-gray-400",
            ) { "Output" }
            (match preview.get_clone() {
                Some(document) => {
                    view! {
                        PreviewFrame(document=document, entries=console_entries)
                    }
                }
                None => {
                    view! {
                        div(
                            class=move || if expanded.get() {
                                "hidden"
                            } else {
                                "flex flex-1 items-center justify-center px-6 py-12 text-center text-sm text-gray-400 dark:text-gray-500"
                            },
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
fn OutputMetadata(
    output: Signal<CompilationState>,
    expanded: ReadSignal<bool>,
    editor: EditorHandle,
) -> View {
    view! {
        section(
            aria-label="Compilation details",
            aria-live="polite",
            class=move || if expanded.get() {
                "min-h-0 max-h-[75dvh] flex-1 overflow-auto border-t border-gray-200 bg-gray-50/50 px-4 py-3 md:max-h-none dark:border-gray-800 dark:bg-gray-900/30"
            } else {
                "max-h-72 shrink-0 overflow-auto border-t border-gray-200 bg-gray-50/50 px-4 py-3 md:max-h-[40dvh] dark:border-gray-800 dark:bg-gray-900/30"
            },
        ) {
            (match output.get_clone() {
                CompilationState::Idle => view! {
                    p(class="text-xs text-gray-500 dark:text-gray-400") { "Click Run to compile and run the code." }
                },
                CompilationState::Compiling => view! {
                    p(class="text-xs text-gray-500 dark:text-gray-400", role="status") { "Compiling…" }
                },
                CompilationState::Complete(metadata) => {
                    let summary = format!(
                        "Finished in {}ms{}",
                        metadata.compilation_time_ms,
                        if metadata.cache_hit { " (cached)" } else { "" },
                    );
                    let cards = metadata.errors.into_iter().map(|diagnostic| diagnostic_card(diagnostic, false, Rc::clone(&editor)))
                        .chain(metadata.warnings.into_iter().map(|diagnostic| diagnostic_card(diagnostic, true, Rc::clone(&editor))))
                        .collect::<Vec<_>>();
                    let cards = View::from(cards);
                    view! {
                        p(class="text-xs font-medium text-gray-600 dark:text-gray-300") { (summary) }
                        (cards)
                    }
                },
                CompilationState::Failed(message) => diagnostic_card(CompilerDiagnostic {
                    message,
                    rendered: None,
                    spans: Vec::new(),
                }, false, Rc::clone(&editor)),
            })
        }
    }
}

/// Displays one diagnostic as a colored card with readable compiler details.
fn diagnostic_card(diagnostic: CompilerDiagnostic, warning: bool, editor: EditorHandle) -> View {
    let class = if warning {
        "mt-3 rounded-lg border border-amber-200 bg-amber-50 p-3 text-amber-950 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-100"
    } else {
        "mt-3 rounded-lg border border-red-200 bg-red-50 p-3 text-red-950 dark:border-red-900 dark:bg-red-950/40 dark:text-red-100"
    };
    let details = diagnostic
        .rendered
        .filter(|text| !text.trim().is_empty())
        .unwrap_or(diagnostic.message);
    let mut lines = details.lines();
    let title = diagnostic_text(lines.next().unwrap_or_default(), Rc::clone(&editor));
    let body = lines.collect::<Vec<_>>().join("\n");
    let body = if body.trim().is_empty() {
        View::default()
    } else {
        let body = diagnostic_text(&body, editor);
        view! {
            pre(class="mt-2 whitespace-pre-wrap break-words font-mono text-xs leading-relaxed") { (body) }
        }
    };
    view! {
        article(class=class) {
            h3(class="break-words text-xs font-semibold") { (title) }
            (body)
        }
    }
}

/// Finds locations in the editable source, leaving dependency paths as plain text.
fn source_locations(text: &str) -> Vec<(std::ops::Range<usize>, u32, u32)> {
    let mut locations = Vec::new();
    for (file, _) in text.match_indices("main.rs:") {
        let start = text[..file]
            .rfind(|c: char| {
                !c.is_alphanumeric() && !matches!(c, '_' | '-' | '.' | '/' | '\\' | ':')
            })
            .map_or(0, |index| {
                index + text[index..].chars().next().unwrap().len_utf8()
            });
        let path = &text[start..file + "main.rs".len()];
        if !matches!(path, "main.rs" | "src/main.rs" | "./src/main.rs") {
            continue;
        }
        let number_start = file + "main.rs:".len();
        let digits = text[number_start..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        let mut end = number_start + digits;
        let Ok(line) = text[number_start..end].parse::<u32>() else {
            continue;
        };
        let mut column = 1;
        if text[end..].starts_with(':') {
            let column_start = end + 1;
            let digits = text[column_start..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            end = column_start + digits;
            let Ok(parsed) = text[column_start..end].parse::<u32>() else {
                continue;
            };
            column = parsed;
        }
        if line == 0
            || column == 0
            || text[end..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        locations.push((start..end, line, column));
    }
    locations
}

/// Turns source locations into editor links while keeping diagnostic text escaped.
fn diagnostic_text(text: &str, editor: EditorHandle) -> View {
    let mut parts = Vec::new();
    let mut end = 0;
    for (range, line, column) in source_locations(text) {
        let prefix = text[end..range.start].to_owned();
        parts.push(view! { (prefix) });
        let label = text[range.clone()].to_owned();
        let editor = Rc::clone(&editor);
        parts.push(view! {
            a(
                href="#code-label",
                title=format!("Go to line {line}, column {column}"),
                class="cursor-pointer underline decoration-dotted underline-offset-2 hover:decoration-solid focus-visible:outline-2 focus-visible:outline-offset-2",
                on:click=move |event: web_sys::MouseEvent| {
                    event.prevent_default();
                    if let Some(editor) = editor.borrow().as_ref() {
                        go_to_location(editor, line, column);
                    }
                },
            ) { (label) }
        });
        end = range.end;
    }
    let suffix = text[end..].to_owned();
    parts.push(view! { (suffix) });
    View::from(parts)
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
    // Compiler and service diagnostics remain structured even for non-2xx responses.
    if !response.ok() && metadata.errors.is_empty() {
        return Err(format!("Compilation request failed (HTTP {status})."));
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
        .replace(
            "__CONSOLE_BRIDGE__",
            include_str!("../js/preview-console.js"),
        )
        .replace("__COMPILE_URL__", &script_string(&backend))
        .replace("__ARTIFACT_URL__", &script_string(artifact_url))
}

#[cfg(test)]
mod tests {
    use super::{compile_url, preview_document, script_string, source_locations};

    #[test]
    fn source_links_preserve_paths_and_positions() {
        let text = " --> src/main.rs:4:17\n see `main.rs:12` and (./src/main.rs:2:3)";
        let locations = source_locations(text)
            .into_iter()
            .map(|(range, line, column)| (&text[range], line, column))
            .collect::<Vec<_>>();
        assert_eq!(
            locations,
            vec![
                ("src/main.rs:4:17", 4, 17),
                ("main.rs:12", 12, 1),
                ("./src/main.rs:2:3", 2, 3),
            ]
        );
    }

    #[test]
    fn source_links_ignore_dependencies_and_invalid_locations() {
        for text in [
            "/registry/dependency/src/main.rs:4:17",
            "other_main.rs:1:2",
            "main.rs:0:1",
            "main.rs:1:0",
            "main.rs:1:",
            "main.rs:abc",
            "main.rs:1:2abc",
            "main.rs:99999999999999999999:1",
        ] {
            assert!(source_locations(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn source_links_handle_unicode_and_html_as_plain_text() {
        let text = "🦀 <script> src/main.rs:2:3 </script>";
        let locations = source_locations(text);
        assert_eq!(locations.len(), 1);
        assert_eq!(&text[locations[0].0.clone()], "src/main.rs:2:3");
    }

    #[test]
    fn preview_includes_artifact_and_initializes_wasm() {
        let document = preview_document("/artifacts/test.wasm");
        assert!(document.contains("new URL(\"/artifacts/test.wasm\", backend)"));
        assert!(document.contains("await import(moduleUrl.href)"));
        assert!(document.contains("await init({ module_or_path:"));
        assert!(!document.contains("__COMPILE_URL__"));
        assert!(!document.contains("__ARTIFACT_URL__"));
        assert!(!document.contains("__CONSOLE_BRIDGE__"));
        assert!(document.contains("sycamore-playground-console"));
        assert!(
            document.find("sycamore-playground-console").unwrap()
                < document.find("await import").unwrap()
        );
    }

    #[test]
    fn artifact_url_cannot_break_out_of_inline_script() {
        let url = "</script><script>alert('injection')</script>\u{2028}&";
        let encoded = script_string(url);
        assert!(!encoded.contains('<'));
        assert!(!encoded.contains('&'));
        assert!(!encoded.contains('\u{2028}'));
        assert_eq!(serde_json::from_str::<String>(&encoded).unwrap(), url);
        assert_eq!(preview_document(url).matches("</script>").count(), 2);
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
