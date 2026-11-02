use gloo_net::http::Request;
use sycamore::prelude::*;
use sycamore_playground_shared::{CompileRequest, CompileResponse, DEFAULT_CODE};
use wasm_bindgen_futures::spawn_local;

fn main() {
    sycamore::render(App);
}

#[component]
fn App() -> View {
    let code = create_signal(DEFAULT_CODE.to_owned());
    let running = create_signal(false);
    let output = create_signal("Click Run to compile and run the code.".to_owned());
    let preview = create_signal(None::<String>);

    let run = move |_| {
        if running.get() {
            return;
        }
        let submitted_code = code.get_clone();
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

    view! {
        main(class="min-h-screen bg-slate-50 px-4 py-8 text-slate-900") {
            div(class="mx-auto max-w-5xl space-y-6") {
                header {
                    h1(class="text-2xl font-semibold") { "Sycamore Playground" }
                    p(class="mt-2 text-sm text-slate-600") {
                        "Compile Rust code and run it in an isolated output frame."
                    }
                }
                div(class="space-y-2") {
                    label(r#for="code", class="block text-sm font-medium") { "Rust code" }
                    textarea(
                        id="code",
                        bind:value=code,
                        "spellcheck"="false",
                        class="min-h-96 w-full resize-y rounded border border-slate-300 bg-white p-4 font-mono text-sm focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-200",
                    )
                }
                button(
                    r#type="button",
                    on:click=run,
                    disabled=move || running.get(),
                    class="rounded bg-blue-600 px-5 py-2 font-medium text-white hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-blue-400 focus:ring-offset-2 disabled:cursor-wait disabled:opacity-50",
                ) {
                    (if running.get() { "Running…" } else { "Run" })
                }
                (match preview.get_clone() {
                    Some(document) => view! {
                        section(class="space-y-2") {
                            h2(class="text-lg font-medium") { "Output" }
                            iframe(
                                title="Playground output",
                                "sandbox"="allow-scripts",
                                referrerpolicy="no-referrer",
                                srcdoc=document,
                                class="min-h-80 w-full rounded border border-slate-300 bg-white",
                            )
                        }
                    },
                    None => View::default(),
                })
                section(class="space-y-2") {
                    h2(class="text-lg font-medium") { "Output metadata" }
                    pre(
                        aria-live="polite",
                        class="min-h-40 max-h-96 overflow-auto whitespace-pre-wrap break-words rounded border border-slate-300 bg-white p-4 font-mono text-sm",
                    ) {
                        (output.get_clone())
                    }
                }
            }
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
