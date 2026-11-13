use sycamore::prelude::*;
use wasm_bindgen::{JsCast, prelude::*};

const MAX_ENTRIES: usize = 200;
const MAX_TEXT_CHARS: usize = 4_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleEntry {
    level: &'static str,
    text: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ConsoleMessage {
    Entry(ConsoleEntry),
    Clear,
}

/// Validates the iframe protocol and bounds the amount of text kept in the UI.
fn parse_message(data: &str) -> Option<ConsoleMessage> {
    if data.len() > 32_000 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    if value.get("type")?.as_str()? != "sycamore-playground-console" {
        return None;
    }
    let level = match value.get("level")?.as_str()? {
        "clear" => return Some(ConsoleMessage::Clear),
        "log" => "log",
        "info" => "info",
        "warn" => "warn",
        "error" => "error",
        "debug" => "debug",
        _ => return None,
    };
    let text = value
        .get("text")?
        .as_str()?
        .chars()
        .take(MAX_TEXT_CHARS)
        .collect();
    Some(ConsoleMessage::Entry(ConsoleEntry { level, text }))
}

/// Mounts an isolated preview and listens only to messages from this iframe.
#[component]
pub fn PreviewFrame(document: String, entries: Signal<Vec<ConsoleEntry>>) -> View {
    let frame_ref = create_node_ref();
    on_mount(move || {
        let frame = frame_ref
            .get()
            .unchecked_into::<web_sys::HtmlIFrameElement>();
        let Some(source) = frame.content_window() else {
            return;
        };
        let window = web_sys::window().expect("browser window");
        let listener =
            Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
                // Sandboxed srcdoc has an opaque origin. Check the WindowProxy, not origin.
                if event.source().map(JsValue::from).as_ref() != Some(source.as_ref()) {
                    return;
                }
                let Some(data) = event.data().as_string() else {
                    return;
                };
                match parse_message(&data) {
                    Some(ConsoleMessage::Clear) => entries.set(Vec::new()),
                    Some(ConsoleMessage::Entry(entry)) => entries.update(|items| {
                        if items.len() >= MAX_ENTRIES {
                            items.remove(0);
                        }
                        items.push(entry);
                    }),
                    None => {}
                }
            });
        if window
            .add_event_listener_with_callback("message", listener.as_ref().unchecked_ref())
            .is_ok()
        {
            on_cleanup(move || {
                let _ = window.remove_event_listener_with_callback(
                    "message",
                    listener.as_ref().unchecked_ref(),
                );
                drop(listener);
            });
        }
    });
    view! {
        iframe(
            r#ref=frame_ref,
            title="Playground output",
            "sandbox"="allow-scripts",
            referrerpolicy="no-referrer",
            srcdoc=document,
            class="min-h-0 w-full flex-1 border-0 bg-white",
        )
    }
}

/// Displays bounded, text-only console output from the current preview.
#[component]
pub fn ConsoleWindow(entries: Signal<Vec<ConsoleEntry>>, preview: Signal<Option<String>>) -> View {
    view! {
        (if preview.with(Option::is_some) && !entries.with(Vec::is_empty) {
            let scroll_ref = create_node_ref();
            on_mount(move || {
                create_effect(move || {
                    entries.track();
                    let element = scroll_ref.get().unchecked_into::<web_sys::HtmlElement>();
                    element.set_scroll_top(element.scroll_height());
                });
            });
            view! {
                section(
                    aria-label="Console",
                    class="flex h-40 max-h-[28dvh] shrink-0 flex-col border-t border-gray-200 bg-gray-50/50 dark:border-gray-800 dark:bg-gray-900/30",
                ) {
                    div(class="flex h-9 shrink-0 items-center justify-between border-b border-gray-200 px-4 dark:border-gray-800") {
                        h2(class="text-xs font-medium text-gray-500 dark:text-gray-400") { "Console" }
                        button(
                            r#type="button",
                            on_click=move |_| entries.set(Vec::new()),
                            disabled=move || entries.with(Vec::is_empty),
                            class="rounded px-2 py-1 text-xs text-gray-600 hover:bg-gray-200 focus-visible:outline-2 focus-visible:outline-orange-500 disabled:opacity-40 dark:text-gray-300 dark:hover:bg-gray-800",
                        ) { "Clear" }
                    }
                    div(r#ref=scroll_ref, role="log", aria-label="Program console", class="min-h-0 flex-1 overflow-auto") {
                        ({
                            let rows = entries.get_clone().into_iter().map(|entry| {
                                let class = match entry.level {
                                    "error" => "flex gap-3 border-b border-red-100 bg-red-50 px-4 py-2 text-red-800 dark:border-red-950 dark:bg-red-950/30 dark:text-red-300",
                                    "warn" => "flex gap-3 border-b border-amber-100 bg-amber-50 px-4 py-2 text-amber-800 dark:border-amber-950 dark:bg-amber-950/30 dark:text-amber-200",
                                    _ => "flex gap-3 border-b border-gray-100 px-4 py-2 text-gray-700 dark:border-gray-800 dark:text-gray-300",
                                };
                                view! {
                                    div(class=class) {
                                        span(class="w-9 shrink-0 font-mono text-xs leading-relaxed opacity-70") { (entry.level) }
                                        pre(class="min-w-0 whitespace-pre-wrap break-words font-mono text-xs leading-relaxed") { (entry.text) }
                                    }
                                }
                            }).collect::<Vec<_>>();
                            View::from(rows)
                        })
                    }
                }
            }
        } else {
            View::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_console_messages() {
        assert_eq!(
            parse_message(
                r#"{"type":"sycamore-playground-console","level":"warn","text":"<script>hello</script>"}"#
            ),
            Some(ConsoleMessage::Entry(ConsoleEntry {
                level: "warn",
                text: "<script>hello</script>".to_owned()
            }))
        );
        assert_eq!(
            parse_message(r#"{"type":"sycamore-playground-console","level":"clear"}"#),
            Some(ConsoleMessage::Clear)
        );
        for data in [
            "not json",
            "{}",
            r#"{"type":"other","level":"log","text":"hi"}"#,
            r#"{"type":"sycamore-playground-console","level":"unknown","text":"hi"}"#,
            r#"{"type":"sycamore-playground-console","level":"log","text":42}"#,
        ] {
            assert_eq!(parse_message(data), None);
        }
    }

    #[test]
    fn bounds_console_message_sizes() {
        let data = serde_json::json!({"type":"sycamore-playground-console","level":"log","text":"🦀".repeat(5_000)}).to_string();
        let Some(ConsoleMessage::Entry(entry)) = parse_message(&data) else {
            panic!("valid message")
        };
        assert_eq!(entry.text.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(parse_message(&"x".repeat(32_001)), None);
    }
}
