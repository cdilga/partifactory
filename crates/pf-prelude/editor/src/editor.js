// The formatted view (decision 54, pf-h6y.6): edit a Markdown part as it reads, like Claude Docs.
// ProseMirror with GFM tables, strikethrough and task lists. Every top-level block keeps its exact
// source bytes until the person changes it, and a save is a list of block ops (replace_block,
// insert_after, delete_block) against the server's block ids, so agents' edits to other blocks
// merge. Bundled by build.mjs into ../js/editor.js; the wrapper loads it on first Edit.
import MarkdownIt from 'markdown-it';
import { Schema } from 'prosemirror-model';
import { schema as baseSchema, MarkdownParser, MarkdownSerializer, defaultMarkdownParser, defaultMarkdownSerializer } from 'prosemirror-markdown';
import { EditorState, Plugin, TextSelection, NodeSelection } from 'prosemirror-state';
import { EditorView, Decoration, DecorationSet } from 'prosemirror-view';
import { keymap } from 'prosemirror-keymap';
import { baseKeymap, toggleMark, setBlockType, wrapIn, lift } from 'prosemirror-commands';
import { history, undo, redo } from 'prosemirror-history';
import { inputRules, wrappingInputRule, textblockTypeInputRule, InputRule } from 'prosemirror-inputrules';
import { wrapInList, splitListItem, liftListItem, sinkListItem } from 'prosemirror-schema-list';
import { tableNodes, tableEditing, goToNextCell, addRowAfter, addColumnAfter, deleteRow, deleteColumn, isInTable, selectedRect } from 'prosemirror-tables';
import { dropCursor } from 'prosemirror-dropcursor';
import { gapCursor } from 'prosemirror-gapcursor';

// ---- Schema: prosemirror-markdown's, plus GFM tables, strikethrough and task items.
const alignAttr = {
  default: null,
  getFromDOM: (dom) => dom.style.textAlign || null,
  setDOMAttr: (value, attrs) => { if (value) attrs.style = (attrs.style || '') + `text-align:${value};`; },
};
let nodes = baseSchema.spec.nodes.append(tableNodes({ tableGroup: 'block', cellContent: 'inline*', cellAttributes: { align: alignAttr } }));
nodes = nodes.update('list_item', Object.assign({}, nodes.get('list_item'), {
  attrs: { checked: { default: null } },
  toDOM: (node) => node.attrs.checked == null ? ['li', 0] : ['li', { class: 'pf-task', 'data-checked': String(node.attrs.checked) }, 0],
  parseDOM: [{ tag: 'li', getAttrs: (dom) => ({ checked: dom.hasAttribute('data-checked') ? dom.getAttribute('data-checked') === 'true' : null }) }],
}));
// Lists keep the marker they were written with ('-' for new ones).
nodes = nodes.update('bullet_list', Object.assign({}, nodes.get('bullet_list'), {
  attrs: { tight: { default: true }, bullet: { default: '-' } },
}));
const marks = baseSchema.spec.marks.addToEnd('strike', {
  parseDOM: [{ tag: 's' }, { tag: 'del' }, { style: 'text-decoration=line-through' }],
  toDOM: () => ['s', 0],
});
export const schema = new Schema({ nodes, marks });

// ---- Parser: markdown-it CommonMark + GFM tables and strikethrough. Raw HTML stays literal.
const md = MarkdownIt('commonmark', { html: false }).enable(['table', 'strikethrough']);
function listTight(tokens, i) { while (++i < tokens.length) if (tokens[i].type !== 'list_item_open') return tokens[i].hidden; return false; }
const cellAttrs = (tok) => { const m = /text-align:\s*(left|center|right)/.exec(tok.attrGet('style') || ''); return { align: m ? m[1] : null }; };
const parser = new MarkdownParser(schema, md, Object.assign({}, defaultMarkdownParser.tokens, {
  bullet_list: { block: 'bullet_list', getAttrs: (tok, tokens, i) => ({ tight: listTight(tokens, i), bullet: tok.markup || '-' }) },
  table: { block: 'table' },
  thead: { ignore: true },
  tbody: { ignore: true },
  tr: { block: 'table_row' },
  th: { block: 'table_header', getAttrs: cellAttrs },
  td: { block: 'table_cell', getAttrs: cellAttrs },
  s: { mark: 'strike' },
}));

// Task items: a list item whose text starts "[ ] " or "[x] " becomes a checkbox item.
function taskify(doc) {
  const json = doc.toJSON();
  (function walk(n) {
    if (n.type === 'list_item' && n.content && n.content[0] && n.content[0].type === 'paragraph') {
      const para = n.content[0], first = para.content && para.content[0];
      const m = first && first.type === 'text' && !first.marks && /^\[( |x|X)\] /.exec(first.text);
      if (m) { n.attrs = { checked: m[1] !== ' ' }; first.text = first.text.slice(4); if (!first.text) para.content.shift(); }
    }
    (n.content || []).forEach(walk);
  })(json);
  return schema.nodeFromJSON(json);
}
export function parse(text) { return taskify(parser.parse(text)); }

// ---- Serializer: prosemirror-markdown's, plus tables (GFM), strikethrough and task markers.
const inlineSerializer = () => serializer;
function cellText(cell) {
  const doc = schema.node('doc', null, [schema.node('paragraph', null, cell.content)]);
  return inlineSerializer().serialize(doc).replace(/\n+/g, ' ').replace(/\|/g, '\\|').trim();
}
export const serializer = new MarkdownSerializer(Object.assign({}, defaultMarkdownSerializer.nodes, {
  list_item(state, node) {
    if (node.attrs.checked != null) state.write(node.attrs.checked ? '[x] ' : '[ ] ');
    state.renderContent(node);
  },
  table(state, node) {
    const rows = [], aligns = [];
    node.forEach((row, _, r) => { const cells = []; row.forEach((cell, __, c) => { cells.push(cellText(cell)); if (r === 0) aligns.push(cell.attrs.align); }); rows.push(cells); });
    const width = Math.max(...rows.map((r) => r.length));
    const pad = (cells) => Array.from({ length: width }, (_, i) => cells[i] || '');
    const rule = aligns.concat(Array(width).fill(null)).slice(0, width).map((a) => a === 'center' ? ':-:' : a === 'right' ? '--:' : a === 'left' ? ':--' : '---');
    const line = (cells) => '| ' + cells.join(' | ') + ' |';
    state.write([line(pad(rows[0] || [])), line(rule)].concat(rows.slice(1).map((r) => line(pad(r)))).join('\n'));
    state.closeBlock(node);
  },
  table_row() {}, table_header() {}, table_cell() {},
}), Object.assign({}, defaultMarkdownSerializer.marks, {
  strike: { open: '~~', close: '~~', mixable: true, expelEnclosingWhitespace: true },
}));
function serializeBlock(node) {
  return serializer.serialize(schema.node('doc', null, [node])).replace(/\n+$/, '');
}

// ---- Source blocks: top-level markdown-it tokens and their exact text.
export function sourceBlocks(text) {
  const lines = [], starts = [];
  let at = 0;
  for (const l of text.split('\n')) { starts.push(at); lines.push(l); at += l.length + 1; }
  const out = [];
  for (const t of md.parse(text, {})) {
    if (t.level !== 0 || !t.map || t.nesting === -1) continue;
    const start = starts[t.map[0]], endLine = t.map[1];
    let end = endLine < starts.length ? starts[endLine] : text.length;
    while (end > start && text[end - 1] === '\n') end--;
    out.push({ start, end, text: text.slice(start, end) });
  }
  return out;
}
async function shortHash(s) {
  const bytes = new TextEncoder().encode(s);
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
  return Array.from(digest, (b) => b.toString(16).padStart(2, '0')).join('').slice(0, 16);
}

// ---- The editor.
const isMac = typeof navigator !== 'undefined' && /Mac|iP(hone|ad)/.test(navigator.platform);
function markActive(state, type) {
  const { from, $from, to, empty } = state.selection;
  return empty ? !!type.isInSet(state.storedMarks || $from.marks()) : state.doc.rangeHasMark(from, to, type);
}
function el(tag, text, parent, cls) { const e = document.createElement(tag); if (text) e.textContent = text; if (cls) e.className = cls; if (parent) parent.appendChild(e); return e; }
function button(label, parent, run, title) {
  const b = el('button', label, parent); b.type = 'button'; if (title) { b.title = title; b.setAttribute('aria-label', title); }
  b.addEventListener('mousedown', (e) => e.preventDefault()); b.addEventListener('click', run); return b;
}

function rules() {
  const s = schema, r = [];
  r.push(textblockTypeInputRule(/^(#{1,6})\s$/, s.nodes.heading, (m) => ({ level: m[1].length })));
  r.push(wrappingInputRule(/^\s*>\s$/, s.nodes.blockquote));
  r.push(wrappingInputRule(/^\s*([-+*])\s$/, s.nodes.bullet_list));
  r.push(wrappingInputRule(/^(\d+)\.\s$/, s.nodes.ordered_list, (m) => ({ order: +m[1] }), (m, n) => n.childCount + n.attrs.order === +m[1]));
  r.push(textblockTypeInputRule(/^```$/, s.nodes.code_block));
  r.push(new InputRule(/^\[( |x)\]\s$/, (state, m, start, end) => {
    const $s = state.doc.resolve(start); const item = $s.node(-1);
    if (!item || item.type !== s.nodes.list_item) return null;
    return state.tr.delete(start, end).setNodeMarkup($s.before(-1), null, { checked: m[1] === 'x' });
  }));
  const markRule = (re, type) => new InputRule(re, (state, m, start, end) => {
    const text = m[2]; const tr = state.tr.replaceWith(start, end, s.text(text, [type.create()]));
    return tr.removeStoredMark(type);
  });
  r.push(markRule(/(\*\*)([^*]+)\*\*$/, s.marks.strong));
  r.push(markRule(/(~~)([^~]+)~~$/, s.marks.strike));
  r.push(markRule(/(`)([^`]+)`$/, s.marks.code));
  return inputRules({ rules: r });
}

function tabInTable(dir) {
  return (state, dispatch, view) => {
    if (!isInTable(state)) return false;
    if (goToNextCell(dir)(state, dispatch)) return true;
    if (dir > 0 && addRowAfter(state, dispatch)) { goToNextCell(1)(view.state, view.dispatch); return true; }
    return true;
  };
}

function keys() {
  const s = schema;
  return keymap(Object.assign({}, {
    'Mod-z': undo, 'Shift-Mod-z': redo, 'Mod-y': redo,
    'Mod-b': toggleMark(s.marks.strong), 'Mod-i': toggleMark(s.marks.em), 'Mod-e': toggleMark(s.marks.code),
    'Shift-Mod-x': toggleMark(s.marks.strike),
    Enter: splitListItem(s.nodes.list_item),
    Tab: (st, d, v) => tabInTable(1)(st, d, v) || sinkListItem(s.nodes.list_item)(st, d),
    'Shift-Tab': (st, d, v) => tabInTable(-1)(st, d, v) || liftListItem(s.nodes.list_item)(st, d),
  }));
}

// Floating toolbar over a selection (Claude Docs style): Comment, style, marks, lists, link,
// and table controls when the cursor is in a table.
function toolbarPlugin(opts) {
  return new Plugin({
    view(view) {
      const bar = el('div', null, null, 'pf-toolbar'); bar.setAttribute('role', 'toolbar'); bar.setAttribute('aria-label', 'Formatting'); bar.hidden = true;
      view.dom.parentNode.appendChild(bar);
      const s = schema, run = (cmd) => () => { cmd(view.state, view.dispatch, view); view.focus(); };
      const comment = button('Comment', bar, () => {
        const { from, to } = view.state.selection; const quote = view.state.doc.textBetween(from, to, ' ').trim();
        const index = view.state.doc.resolve(from).index(0);
        if (quote && opts.onComment) opts.onComment({ quote: quote.slice(0, 500), blockIndex: index });
      }, 'Comment');
      comment.classList.add('pf-tb-comment');
      const style = el('select', null, bar); style.setAttribute('aria-label', 'Text style');
      [['p', 'Text'], ['1', 'Heading 1'], ['2', 'Heading 2'], ['3', 'Heading 3']].forEach(([v, l]) => { const o = el('option', l, style); o.value = v; });
      style.addEventListener('change', () => { (style.value === 'p' ? setBlockType(s.nodes.paragraph) : setBlockType(s.nodes.heading, { level: +style.value }))(view.state, view.dispatch); view.focus(); });
      const marksBtns = [
        ['B', s.marks.strong, 'Bold'], ['I', s.marks.em, 'Italic'], ['S', s.marks.strike, 'Strikethrough'], ['</>', s.marks.code, 'Inline code'],
      ].map(([l, t, title]) => [button(l, bar, run(toggleMark(t)), title), t]);
      const lists = el('select', null, bar); lists.setAttribute('aria-label', 'List');
      [['', 'List'], ['bullet', 'Bulleted list'], ['ordered', 'Numbered list'], ['task', 'Task list'], ['none', 'No list']].forEach(([v, l]) => { const o = el('option', l, lists); o.value = v; });
      lists.addEventListener('change', () => {
        const v = lists.value; lists.value = '';
        if (v === 'none') liftListItem(s.nodes.list_item)(view.state, view.dispatch);
        else if (v === 'bullet' || v === 'task') {
          wrapInList(s.nodes.bullet_list)(view.state, view.dispatch);
          if (v === 'task') { const $f = view.state.selection.$from; for (let d = $f.depth; d > 0; d--) if ($f.node(d).type === s.nodes.list_item) { view.dispatch(view.state.tr.setNodeMarkup($f.before(d), null, { checked: false })); break; } }
        } else if (v === 'ordered') wrapInList(s.nodes.ordered_list)(view.state, view.dispatch);
        view.focus();
      });
      const linkBox = el('span', null, bar, 'pf-tb-link');
      const linkInput = el('input', null, null); linkInput.type = 'url'; linkInput.placeholder = 'https://…'; linkInput.setAttribute('aria-label', 'Link address'); linkInput.hidden = true;
      const link = button('Link', linkBox, () => {
        if (markActive(view.state, s.marks.link)) { toggleMark(s.marks.link)(view.state, view.dispatch); return; }
        linkInput.hidden = false; linkInput.focus();
      }, 'Link');
      linkBox.appendChild(linkInput);
      linkInput.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') { e.preventDefault(); const href = linkInput.value.trim(); linkInput.hidden = true; linkInput.value = ''; if (/^(https?:|mailto:|\/|#)/.test(href)) toggleMark(s.marks.link, { href })(view.state, view.dispatch); view.focus(); }
        if (e.key === 'Escape') { linkInput.hidden = true; view.focus(); }
      });
      const table = el('span', null, bar, 'pf-tb-table');
      button('+Row', table, run(addRowAfter), 'Add row'); button('+Col', table, run(addColumnAfter), 'Add column');
      button('−Row', table, run(deleteRow), 'Delete row'); button('−Col', table, run(deleteColumn), 'Delete column');
      const align = el('select', null, table); align.setAttribute('aria-label', 'Column alignment');
      [['', 'Align'], ['left', 'Left'], ['center', 'Centre'], ['right', 'Right']].forEach(([v, l]) => { const o = el('option', l, align); o.value = v; });
      align.addEventListener('change', () => {
        const v = align.value || null; align.value = ''; const st = view.state; if (!isInTable(st)) return;
        const rect = selectedRect(st), tr = st.tr;
        for (let row = 0; row < rect.map.height; row++) { const pos = rect.tableStart + rect.map.map[row * rect.map.width + rect.left]; const cell = st.doc.nodeAt(pos); if (cell) tr.setNodeMarkup(pos, null, Object.assign({}, cell.attrs, { align: v })); }
        view.dispatch(tr); view.focus();
      });
      const update = () => {
        const st = view.state, { empty, from } = st.selection, inTable = isInTable(st);
        const show = view.hasFocus() && (!empty || inTable) && opts.editable !== false;
        bar.hidden = !show; if (!show) return;
        comment.hidden = empty || !opts.onComment; table.hidden = !inTable;
        marksBtns.forEach(([b, t]) => b.setAttribute('aria-pressed', String(markActive(st, t))));
        const parent = st.selection.$from.parent; style.value = parent.type === s.nodes.heading && parent.attrs.level <= 3 ? String(parent.attrs.level) : 'p';
        link.setAttribute('aria-pressed', String(markActive(st, s.marks.link)));
        if (window.matchMedia && window.matchMedia('(max-width: 600px)').matches) { bar.classList.add('pf-docked'); return; }
        bar.classList.remove('pf-docked');
        const box = view.dom.parentNode.getBoundingClientRect(), at = view.coordsAtPos(from);
        bar.style.left = Math.max(0, Math.min(at.left - box.left, box.width - bar.offsetWidth)) + 'px';
        bar.style.top = Math.max(0, at.top - box.top - bar.offsetHeight - 8 + view.dom.parentNode.scrollTop) + 'px';
      };
      view.dom.addEventListener('blur', () => setTimeout(() => { if (!view.hasFocus() && !bar.contains(document.activeElement)) bar.hidden = true; }, 150));
      return { update, destroy: () => bar.remove() };
    },
  });
}

// Block handles: ⋮⋮ beside each top-level block, draggable, with a menu (move, insert, delete).
function handlesPlugin() {
  const s = schema;
  const insertables = [['Text', () => s.nodes.paragraph.create()], ['Heading', () => s.nodes.heading.create({ level: 2 })],
    ['Bulleted list', () => s.nodes.bullet_list.create(null, s.nodes.list_item.create(null, s.nodes.paragraph.create()))],
    ['Task list', () => s.nodes.bullet_list.create(null, s.nodes.list_item.create({ checked: false }, s.nodes.paragraph.create()))],
    ['Quote', () => s.nodes.blockquote.create(null, s.nodes.paragraph.create())], ['Code', () => s.nodes.code_block.create()],
    ['Divider', () => s.nodes.horizontal_rule.create()],
    ['Table', () => s.nodes.table.create(null, [0, 1].map((r) => s.nodes.table_row.create(null, [0, 1].map(() => (r ? s.nodes.table_cell : s.nodes.table_header).create()))))]];
  function menu(view, pos, anchor) {
    document.querySelectorAll('.pf-block-menu').forEach((m) => m.remove());
    const m = el('div', null, anchor.ownerDocument.body, 'pf-block-menu'); m.setAttribute('role', 'menu');
    const r = anchor.getBoundingClientRect(); m.style.left = r.left + 'px'; m.style.top = (r.bottom + 4) + 'px';
    const node = view.state.doc.nodeAt(pos), index = view.state.doc.resolve(pos).index(0), count = view.state.doc.childCount;
    const close = () => m.remove();
    const move = (d) => { const tr = view.state.tr; const target = d < 0 ? pos - view.state.doc.child(index - 1).nodeSize : pos + node.nodeSize + view.state.doc.child(index + 1).nodeSize; tr.delete(pos, pos + node.nodeSize); tr.insert(d < 0 ? target : target - node.nodeSize, node); view.dispatch(tr); };
    if (index > 0) button('Move up', m, () => { move(-1); close(); }).setAttribute('role', 'menuitem');
    if (index < count - 1) button('Move down', m, () => { move(1); close(); }).setAttribute('role', 'menuitem');
    insertables.forEach(([label, make]) => button('Insert ' + label.toLowerCase() + ' below', m, () => {
      const at = pos + node.nodeSize; const tr = view.state.tr.insert(at, make());
      view.dispatch(tr.setSelection(TextSelection.near(tr.doc.resolve(at + 1)))); view.focus(); close();
    }).setAttribute('role', 'menuitem'));
    button('Delete', m, () => { view.dispatch(view.state.tr.delete(pos, pos + node.nodeSize)); close(); }).setAttribute('role', 'menuitem');
    setTimeout(() => document.addEventListener('click', function off(e) { if (!m.contains(e.target)) { close(); document.removeEventListener('click', off); } }), 0);
    m.querySelector('button').focus();
  }
  return new Plugin({
    props: {
      decorations(state) {
        const decos = [];
        state.doc.forEach((node, offset) => {
          decos.push(Decoration.widget(offset, (view) => {
            const h = el('button', '⋮⋮', null, 'pf-handle'); h.type = 'button'; h.draggable = true; h.contentEditable = 'false';
            h.setAttribute('aria-label', 'Block menu');
            h.addEventListener('mousedown', (e) => e.stopPropagation());
            h.addEventListener('click', (e) => { e.preventDefault(); menu(view, offset, h); });
            h.addEventListener('dragstart', (e) => {
              const sel = NodeSelection.create(view.state.doc, offset); view.dispatch(view.state.tr.setSelection(sel));
              e.dataTransfer.effectAllowed = 'move'; e.dataTransfer.setData('text/plain', node.textContent);
              view.dragging = { slice: sel.content(), move: true };
            });
            return h;
          }, { side: -1, key: 'h' + offset + ':' + node.nodeSize }));
        });
        return DecorationSet.create(state.doc, decos);
      },
    },
  });
}

/**
 * Mount the formatted view. `blocks` are the server's ids and hashes for this file, in order.
 * Returns {value(), ops(), destroy()}: the file with only changed blocks rewritten, and the
 * block ops that produce it (null when the blocks can't be matched to the server's).
 */
export function mount(parent, source, options = {}) {
  const orig = sourceBlocks(source);
  let doc;
  try { doc = parse(source); } catch (e) { return null; }
  // Each top-level node must line up with one source block, or the view can't keep bytes.
  if (doc.childCount !== orig.length) return null;
  const origNodes = []; doc.forEach((n) => origNodes.push(n));
  const prefix = orig.length ? source.slice(0, orig[0].start) : source;
  const gaps = orig.map((b, i) => (i === 0 ? '' : source.slice(orig[i - 1].end, b.start)));
  const tail = orig.length ? source.slice(orig[orig.length - 1].end) : '';
  const ids = { ok: false, list: [] };
  const serverBlocks = options.blocks || [];
  if (serverBlocks.length === orig.length) {
    Promise.all(orig.map((b) => shortHash(b.text))).then((hashes) => {
      ids.ok = hashes.every((h, i) => h === serverBlocks[i].hash);
      ids.list = serverBlocks;
    });
  }
  const host = el('div', null, parent, 'pf-pm');
  const view = new EditorView(host, {
    state: EditorState.create({
      doc,
      plugins: [rules(), keys(), keymap(baseKeymap), history(), dropCursor(), gapCursor(), tableEditing(), handlesPlugin(), toolbarPlugin(options)],
    }),
    handleClickOn(view, pos, node, nodePos, event, direct) {
      // Clicking a task item's box (left of its text) ticks it.
      if (node.type !== schema.nodes.list_item || node.attrs.checked == null || !direct) return false;
      const box = view.nodeDOM(nodePos); if (!box || event.clientX > box.getBoundingClientRect().left + 2) return false;
      view.dispatch(view.state.tr.setNodeMarkup(nodePos, null, { checked: !node.attrs.checked })); return true;
    },
    attributes: { class: 'pf-doc', 'aria-label': 'Formatted text', role: 'textbox', 'aria-multiline': 'true' },
    dispatchTransaction(tr) {
      view.updateState(view.state.apply(tr));
      if (tr.docChanged && options.onChange) options.onChange();
    },
  });
  // Match current blocks to original ones (unchanged nodes, in order).
  function plan() {
    const out = []; let j = 0;
    view.state.doc.forEach((node) => {
      let found = -1;
      for (let k = j; k < origNodes.length; k++) if (origNodes[k].eq(node)) { found = k; break; }
      if (found >= 0) { out.push({ orig: found, text: orig[found].text, type: node.type.name }); j = found + 1; } else out.push({ orig: null, text: serializeBlock(node), type: node.type.name });
    });
    return out;
  }
  function value() {
    const p = plan(); let text = '', prev = -1;
    p.forEach((b, i) => {
      if (i === 0) text += b.orig === 0 ? prefix : '';
      else text += b.orig != null && b.orig === prev + 1 ? gaps[b.orig] : '\n\n';
      text += b.text; prev = b.orig != null ? b.orig : -2;
    });
    const last = p[p.length - 1];
    text += last && last.orig === orig.length - 1 ? tail : '\n';
    return p.length ? text : '';
  }
  // Block ops: unchanged blocks are anchors; between anchors, changed blocks replace removed ones
  // in order, extras are deleted or inserted. A move is a delete plus an insert.
  function ops() {
    if (!ids.ok) return null;
    const p = plan(), result = [], used = new Set(p.filter((b) => b.orig != null).map((b) => b.orig));
    let i = 0, prevId = 'start', origAt = 0, pending = [];
    const flush = () => { if (pending.length) { result.push({ op: 'insert_after', block: prevId, markdown: pending.join('\n\n') }); pending = []; } };
    while (i < p.length) {
      if (p[i].orig != null) { flush(); prevId = ids.list[p[i].orig].id; origAt = p[i].orig + 1; i++; continue; }
      // A run of changed blocks between anchors: each replaces a removed block of the same kind,
      // in order, keeping its id; the rest are inserts, and leftover removed blocks are deletes.
      const next = (() => { for (let k = i; k < p.length; k++) if (p[k].orig != null) return p[k].orig; return orig.length; })();
      const removed = []; for (let k = origAt; k < next; k++) if (!used.has(k)) removed.push(k);
      while (i < p.length && p[i].orig == null) {
        const at = removed.findIndex((k) => origNodes[k].type.name === p[i].type);
        if (at >= 0) {
          const k = removed.splice(at, 1)[0]; used.add(k); flush();
          result.push({ op: 'replace_block', block: ids.list[k].id, markdown: p[i].text, if_hash: ids.list[k].hash });
          prevId = ids.list[k].id;
        } else pending.push(p[i].text);
        i++;
      }
      flush();
    }
    for (let k = 0; k < orig.length; k++) if (!used.has(k)) result.push({ op: 'delete_block', block: ids.list[k].id, if_hash: ids.list[k].hash });
    return result;
  }
  return {
    view, value, ops,
    blockId: (index) => (ids.ok && ids.list[index] ? ids.list[index].id : null),
    destroy: () => { view.destroy(); host.remove(); },
  };
}

