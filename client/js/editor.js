import { basicSetup } from "codemirror";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { HighlightStyle, indentUnit, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";
import { rust } from "@codemirror/lang-rust";

// Standard Tailwind gray/orange colors, matching the surrounding playground.
function theme(dark) {
    const foreground = dark ? "#f3f4f6" : "#111827";
    const background = dark ? "#111827" : "#ffffff";
    const muted = dark ? "#9ca3af" : "#6b7280";
    const accent = dark ? "#fb923c" : "#c2410c";
    return [
        EditorView.theme({
            "&": { height: "100%", color: foreground, backgroundColor: background, fontSize: "13px" },
            "&.cm-focused": { outline: "none" },
            ".cm-scroller": { fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace", lineHeight: "1.65", overflow: "auto" },
            ".cm-content": { padding: "12px 0", caretColor: foreground },
            ".cm-line": { padding: "0 12px" },
            ".cm-gutters": { backgroundColor: background, color: muted, borderRight: "none" },
            ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: dark ? "#1f2937" : "#f3f4f6" },
            ".cm-cursor, .cm-dropCursor": { borderLeftColor: foreground },
            "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
                backgroundColor: dark ? "#374151" : "#fed7aa",
            },
            ".cm-panels, .cm-tooltip": {
                backgroundColor: dark ? "#1f2937" : "#f3f4f6",
                color: foreground,
                borderColor: dark ? "#374151" : "#d1d5db",
            },
            ".cm-textfield": { backgroundColor: background, color: foreground },
        }, { dark }),
        syntaxHighlighting(HighlightStyle.define([
            { tag: tags.keyword, color: accent },
            { tag: [tags.string, tags.character], color: dark ? "#86efac" : "#166534" },
            { tag: [tags.number, tags.bool], color: dark ? "#fdba74" : "#9a3412" },
            { tag: tags.comment, color: muted, fontStyle: "italic" },
            { tag: [tags.typeName, tags.className], color: dark ? "#93c5fd" : "#1d4ed8" },
            { tag: tags.function(tags.variableName), color: dark ? "#fde68a" : "#854d0e" },
            { tag: tags.meta, color: accent },
        ])),
    ];
}

export function mount(host, code, onRun, onChange) {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const appearance = new Compartment();
    const view = new EditorView({
        parent: host,
        doc: code,
        extensions: [
            basicSetup,
            EditorView.updateListener.of((update) => {
                if (update.docChanged) onChange(update.state.doc.toString());
            }),
            rust(),
            indentUnit.of("    "),
            EditorState.tabSize.of(4),
            EditorView.contentAttributes.of({
                "aria-labelledby": "code-label",
                "aria-multiline": "true",
                spellcheck: "false",
            }),
            appearance.of(theme(media.matches)),
        ],
    });
    // Leave Tab free for keyboard navigation (CodeMirror's default).
    const updateTheme = () => view.dispatch({ effects: appearance.reconfigure(theme(media.matches)) });
    media.addEventListener("change", updateTheme);
    const document = host.ownerDocument;
    const runShortcut = (event) => {
        if (event.key === "Enter" && event.ctrlKey && !event.altKey && !event.shiftKey && !event.metaKey && !event.isComposing) {
            // Capture before CodeMirror's own Ctrl+Enter command can insert a line.
            event.preventDefault();
            event.stopPropagation();
            if (!event.repeat) onRun();
        }
    };
    document.addEventListener("keydown", runShortcut, true);
    return { view, media, updateTheme, document, runShortcut };
}

export function getCode(editor) {
    return editor.view.state.doc.toString();
}

export function destroy(editor) {
    editor.document.removeEventListener("keydown", editor.runShortcut, true);
    editor.media.removeEventListener("change", editor.updateTheme);
    editor.view.destroy();
}
