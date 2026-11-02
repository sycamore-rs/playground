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
    let output = create_signal("Click Run to compile the code and view its metadata.".to_owned());

    let run = move |_| {
        if running.get() {
            return;
        }
        let submitted_code = code.get_clone();
        running.set(true);
        output.set("Compiling…".to_owned());
        spawn_local(async move {
            let result = compile(submitted_code).await;
            output.set(match result {
                Ok(metadata) => metadata,
                Err(error) => format!("Error: {error}"),
            });
            running.set(false);
        });
    };

    view! {
        main(class="min-h-screen bg-slate-50 px-4 py-8 text-slate-900") {
            div(class="mx-auto max-w-5xl space-y-6") {
                header {
                    h1(class="text-2xl font-semibold") { "Sycamore Playground" }
                    p(class="mt-2 text-sm text-slate-600") {
                        "Compile Rust code and inspect the response metadata. WASM is not downloaded or executed yet."
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

async fn compile(code: String) -> Result<String, String> {
    // Embedded at build time; absent or empty keeps local requests same-origin.
    let url = compile_url(option_env!("BACKEND_BASE_URL").unwrap_or(""));
    let request = Request::post(&url)
        .json(&CompileRequest { code })
        .map_err(|error| format!("Could not encode compile request: {error}"))?;
    let response = request
        .send()
        .await
        .map_err(|error| format!("Could not reach the compilation server: {error}"))?;
    if !response.ok() {
        let status = response.status();
        let message = response.text().await.map_err(|error| {
            format!("HTTP {status}; could not read the error response: {error}")
        })?;
        return Err(format!("HTTP {status}\n{message}"));
    }
    let metadata = response
        .json::<CompileResponse>()
        .await
        .map_err(|error| format!("Invalid compilation metadata: {error}"))?;
    serde_json::to_string_pretty(&metadata)
        .map_err(|error| format!("Could not display compilation metadata: {error}"))
}

#[cfg(test)]
mod tests {
    use super::compile_url;

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
