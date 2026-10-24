// CodeMirror 6 glue for the Leptos interop probe. In the product, CodeMirror is
// vendored as one shared blob; the probe imports it from esm.sh.
import { EditorView, basicSetup } from "https://esm.sh/codemirror@6.0.1";
import { html } from "https://esm.sh/@codemirror/lang-html@6.4.9";

export function mountEditor(parent, doc, onChange) {
  return new EditorView({
    doc,
    parent,
    extensions: [
      basicSetup,
      html(),
      EditorView.updateListener.of((u) => { if (u.docChanged) onChange(u.state.doc.toString()); }),
    ],
  });
}

export function editorInsert(view, text) {
  const at = view.state.selection.main.head;
  view.dispatch({ changes: { from: at, insert: text }, selection: { anchor: at + text.length } });
  view.focus();
}
