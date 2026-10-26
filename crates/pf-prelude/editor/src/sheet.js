// The Edit sheet (pf-h6y.5, pf-h6y.6): source and formatted views of a part's text files, with
// drafts, block-op saves and conflict handling. It lives in the editor bundle, loaded on the first
// Edit, so the always-loaded wrapper stays small. `ctx` is {api, openComments} from the wrapper.
import { mount } from './editor.js';
import { plain } from './plain.js';

let ctx = null;
function el(tag, text, parent) { var e = document.createElement(tag); if (text) e.textContent = text; if (parent) parent.appendChild(e); return e; }
function button(text, parent, action) { var b = el('button', text, parent); b.type = 'button'; b.onclick = action; return b; }
// The source editor (Slice 2): Edit and above change text parts here, with drafts and a
// conflict notice if someone else saved first. The page reloads to the new version on save.
var editor = null;
function openEditor() {
  if (!editor) {
    editor = { panel: el('dialog', null, document.body) };
    var d = editor.panel; d.id = 'pf-editor'; d.setAttribute('aria-labelledby', 'pf-editor-title');
    var head = el('header', null, d); el('h2', 'Edit', head).id = 'pf-editor-title';
    editor.file = el('select', null, head); editor.file.setAttribute('aria-label', 'File');
    editor.modeBox = el('div', null, head); editor.modeBox.className = 'pf-mode'; editor.modeBox.setAttribute('role', 'group'); editor.modeBox.setAttribute('aria-label', 'View');
    editor.formattedButton = button('Formatted', editor.modeBox, function () { setMode('formatted'); });
    editor.sourceButton = button('Source', editor.modeBox, function () { setMode('source'); });
    editor.tools = el('div', null, head); editor.tools.className = 'pf-tools';
    button('B', editor.tools, function () { document.execCommand('bold'); }).setAttribute('aria-label', 'Bold');
    button('I', editor.tools, function () { document.execCommand('italic'); }).setAttribute('aria-label', 'Italic');
    editor.save = button('Save', head, function () { saveEditor(false); }); editor.save.className = 'pf-primary';
    button('Close', head, function () { d.close(); });
    editor.text = el('textarea', null, d); editor.text.spellcheck = false; editor.text.setAttribute('aria-label', 'Source');
    editor.formatted = el('div', null, d); editor.formatted.className = 'pf-formatted'; editor.formatted.setAttribute('aria-label', 'Formatted text'); editor.formatted.setAttribute('role', 'document');
    editor.status = el('p', null, d); editor.status.setAttribute('role', 'status');
    editor.conflict = el('div', null, d); editor.conflict.className = 'pf-conflict'; editor.conflict.hidden = true;
    el('p', 'This part changed since you started editing.', editor.conflict);
    button('Save mine anyway', editor.conflict, function () { saveEditor(true); });
    button('Discard my changes', editor.conflict, function () { discardEditor(); });
    var timer;
    editor.changed = function (value) {
      editor.edits[editor.file.value] = value; clearTimeout(timer);
      timer = setTimeout(function () {
        // A save started after this draft was queued wins: never report or keep a stale draft.
        var seq = editor.saves;
        ctx.api('/draft', { files: editor.edits, base: editor.head }).then(function () { if (seq === editor.saves) editor.status.textContent = 'Draft saved.'; }).catch(function () {});
      }, 800);
    };
    editor.saves = 0;
    editor.cancelDraft = function () { clearTimeout(timer);
    };
    editor.text.oninput = function () { editor.changed(editor.text.value); };
    editor.file.onchange = function () { setMode(editor.mode); };
  }
  editor.panel.showModal(); editor.status.textContent = 'Loading…'; editor.conflict.hidden = true;
  return ctx.api('/source').then(function (src) {
    editor.src = src; editor.head = src.head; editor.edits = {};
    var names = Object.keys(src.content || {}).filter(function (f) { return typeof src.content[f] === 'string'; });
    editor.file.replaceChildren(); names.forEach(function (f) { var o = el('option', f, editor.file); o.value = f; });
    editor.file.hidden = names.length < 2;
    editor.status.textContent = '';
    if (src.draft && src.draft.files) {
      editor.edits = src.draft.files;
      editor.status.textContent = src.draft.base === src.head ? 'Restored your unsaved changes.' : 'Restored your unsaved changes; the part has changed since.';
    }
    editor.file.value = names[0]; setMode(editor.mode || 'formatted');
  }).catch(function (e) { editor.status.textContent = e.message || String(e); });
}
function current() { var f = editor.file.value; return editor.edits[f] != null ? editor.edits[f] : editor.src.content[f]; }
// Markdown and text open formatted; the source is one tap away. Switching re-reads the file.
function setMode(mode) {
  var f = editor.file.value, kind = /\.md$/i.test(f) || editor.src.kind === 'markdown' ? 'markdown' : (editor.src.kind === 'text' || /\.txt$/i.test(f) ? 'text' : null);
  if (!kind) mode = 'source';
  if (editor.view && editor.view.destroy) editor.view.destroy();
  editor.view = null; editor.viewFile = f;
  editor.mode = mode; editor.modeBox.hidden = !kind; editor.tools.hidden = mode !== 'formatted' || kind !== 'text';
  editor.formattedButton.setAttribute('aria-pressed', String(mode === 'formatted')); editor.sourceButton.setAttribute('aria-pressed', String(mode === 'source'));
  editor.text.hidden = mode !== 'source'; editor.formatted.hidden = mode !== 'formatted';
  editor.formatted.replaceChildren();
  if (mode === 'source') { editor.text.value = current(); return Promise.resolve(); }
  if (kind === 'text') { editor.view = plain.mount(editor.formatted, current(), kind, function () { editor.changed(editor.view.value()); }); return Promise.resolve(); }
  // Server block ids only line up with the server's text, so a restored draft saves whole.
  var blocks = editor.edits[f] == null ? (editor.src.blocks || {})[f] : null;
  return Promise.resolve().then(function () {
    var v = mount(editor.formatted, current(), { blocks: blocks, onChange: function () { editor.changed(v.value()); }, onComment: commentOnSelection });
    if (!v) { editor.status.textContent = 'This file has parts the formatted view can\u2019t edit faithfully, so it opens as source.'; return setMode('source'); }
    editor.view = v;
  }).catch(function (e) { editor.status.textContent = e.message; return setMode('source'); });
}
function commentOnSelection(c) {
  var block = editor.view && editor.view.blockId ? editor.view.blockId(c.blockIndex) : null;
  ctx.openComments({ quote: c.quote, block: block });
}
function saveEditor(force) {
  editor.cancelDraft(); editor.saves++;
  var files = Object.keys(editor.edits); if (!files.length) { editor.status.textContent = 'No changes.'; return; }
  var chain = Promise.resolve(), saved = null;
  files.forEach(function (f) {
    chain = chain.then(function () {
      // The formatted view saves block ops, which merge with others' edits to other blocks.
      var ops = !force && editor.mode === 'formatted' && editor.viewFile === f && editor.view && editor.view.ops ? editor.view.ops() : null;
      if (ops && !ops.length) { delete editor.edits[f]; return; }
      if (ops) return ctx.api('/source', { file: f, ops: ops }).then(function (j) { saved = j; delete editor.edits[f]; });
      var hash = force && editor.current && editor.current[f] ? editor.current[f] : (editor.src.files[f] || {}).hash || null;
      return ctx.api('/source', { file: f, content: editor.edits[f], if_match: hash }).then(function (j) { saved = j; delete editor.edits[f]; });
    });
  });
  chain.then(function () {
    if (!saved) { editor.status.textContent = 'No changes.'; return; }
    editor.conflict.hidden = true; editor.status.textContent = 'Saved as version ' + saved.head + '.';
    ctx.api('/draft', { files: {}, base: saved.head }).catch(function () {});
    var f = document.getElementById('pf-frame'); if (f) setTimeout(function () { f.src = f.src; }, 600);
    return ctx.api('/source').then(function (src) { editor.src = src; editor.head = src.head; setMode(editor.mode); });
  }).catch(function (e) {
    if (e.code === 'if_match_failed') { editor.current = editor.current || {}; editor.current[editor.file.value] = e.details && e.details.current_sha256; editor.conflict.hidden = false; editor.status.textContent = ''; return; }
    if (e.code === 'block_changed' || e.code === 'block_not_found') {
      // Someone changed a block this person also changed: offer theirs or ours, for the file.
      return ctx.api('/source').then(function (now) { editor.current = editor.current || {}; editor.current[editor.file.value] = (now.files[editor.file.value] || {}).hash; editor.conflict.hidden = false; editor.status.textContent = ''; });
    }
    editor.status.textContent = e.message || String(e);
  });
}
function discardEditor() { editor.edits = {}; ctx.api('/draft', { files: {}, base: editor.head }).catch(function () {}); editor.panel.close(); openEditor(); }

export function openSheet(context) { ctx = context; return openEditor(); }
