/* partifactory formatted view (decision 54): edit a Markdown or text part as it reads.
   The file is split into blocks that keep their exact source bytes; only blocks the person
   changes are serialised again, so a save never rewrites what they didn't touch. Text is only
   ever inserted with textContent or by building elements, never as HTML. */
export const plain = (function () {
  'use strict';
  function el(tag, text, parent) { var e = document.createElement(tag); if (text) e.textContent = text; if (parent) parent.appendChild(e); return e; }

  // Blocks: runs of lines separated by blank lines, with fenced code kept whole.
  function parse(src, kind) {
    var lines = src.split('\n'), blocks = [], i = 0, offset = 0, starts = [];
    for (var n = 0; n < lines.length; n++) { starts.push(offset); offset += lines[n].length + 1; }
    function push(from, to, type) { blocks.push({ type: type, start: starts[from], end: to < lines.length ? starts[to] - 1 : src.length, text: lines.slice(from, to).join('\n') }); }
    while (i < lines.length) {
      if (lines[i].trim() === '') { i++; continue; }
      var from = i;
      if (kind === 'markdown' && /^(```|~~~)/.test(lines[i])) {
        var fence = lines[i].slice(0, 3); i++;
        while (i < lines.length && lines[i].indexOf(fence) !== 0) i++;
        push(from, Math.min(i + 1, lines.length), 'code'); i++; continue;
      }
      while (i < lines.length && lines[i].trim() !== '' && !(kind === 'markdown' && i > from && /^(```|~~~|#{1,6}\s)/.test(lines[i]))) i++;
      var first = lines[from], type = 'p';
      if (kind === 'markdown') {
        if (/^#{1,6}\s/.test(first) && i === from + 1) type = 'h';
        else if (/^\s*([-*+]|\d+[.)])\s/.test(first)) type = 'list';
        else if (/^>/.test(first)) type = 'quote';
      }
      push(from, i, type);
    }
    return blocks;
  }

  // Inline Markdown -> elements (bold, italic, code, links); everything else stays literal.
  function inline(text, parent) {
    var re = /(\*\*([^*]+)\*\*|\*([^*]+)\*|`([^`]+)`|\[([^\]]+)\]\(([^)\s]+)\))/g, last = 0, m;
    while ((m = re.exec(text))) {
      if (m.index > last) parent.appendChild(document.createTextNode(text.slice(last, m.index)));
      if (m[2]) el('strong', m[2], parent);
      else if (m[3]) el('em', m[3], parent);
      else if (m[4]) el('code', m[4], parent);
      else { var a = el('a', m[5], parent); a.setAttribute('data-href', m[6]); a.title = m[6]; }
      last = re.lastIndex;
    }
    if (last < text.length) parent.appendChild(document.createTextNode(text.slice(last)));
  }
  // Elements -> inline Markdown. Line breaks the browser inserts become newlines.
  function serialise(node) {
    var out = '';
    node.childNodes.forEach(function (c) {
      if (c.nodeType === 3) { out += c.nodeValue.replace(/ /g, ' '); return; }
      var t = c.nodeName, inner = serialise(c);
      if (t === 'STRONG' || t === 'B') out += '**' + inner + '**';
      else if (t === 'EM' || t === 'I') out += '*' + inner + '*';
      else if (t === 'CODE') out += '`' + inner + '`';
      else if (t === 'A') out += '[' + inner + '](' + (c.getAttribute('data-href') || c.getAttribute('href') || '') + ')';
      else if (t === 'BR') out += '\n';
      else if (t === 'DIV' || t === 'P') out += (out && !out.endsWith('\n') ? '\n' : '') + inner;
      else out += inner;
    });
    return out;
  }

  function render(block, kind, parent, onchange) {
    var node, prefix = '';
    if (block.type === 'h') {
      var level = /^#+/.exec(block.text)[0].length; prefix = block.text.slice(0, level + 1);
      node = el('h' + Math.min(level + 1, 6), null, parent); inline(block.text.slice(level + 1), node);
    } else if (block.type === 'list') {
      var ordered = /^\s*\d/.test(block.text); node = el(ordered ? 'ol' : 'ul', null, parent);
      block.text.split('\n').forEach(function (line) {
        var mm = /^(\s*(?:[-*+]|\d+[.)])\s)(.*)$/.exec(line), li = el('li', null, node);
        li.setAttribute('data-marker', mm ? mm[1] : '- '); inline(mm ? mm[2] : line, li);
      });
    } else if (block.type === 'quote') {
      node = el('blockquote', null, parent); inline(block.text.replace(/^>\s?/gm, ''), node);
    } else if (block.type === 'code') {
      node = el('pre', block.text, parent);
    } else {
      node = el('p', null, parent); if (kind === 'markdown') inline(block.text, node); else node.textContent = block.text;
    }
    node.contentEditable = block.type === 'code' ? 'plaintext-only' : 'true';
    node.className = 'pf-block';
    node.addEventListener('input', function () { block.dirty = true; onchange(); });
    block.toText = function () {
      if (block.type === 'code') return node.textContent;
      if (block.type === 'h') return prefix + serialise(node).replace(/\n/g, ' ');
      if (block.type === 'list') {
        return Array.prototype.map.call(node.querySelectorAll('li'), function (li) { return (li.getAttribute('data-marker') || '- ') + serialise(li).replace(/\n/g, ' '); }).join('\n');
      }
      if (block.type === 'quote') return serialise(node).split('\n').map(function (l) { return '> ' + l; }).join('\n');
      return kind === 'markdown' ? serialise(node) : node.innerText.replace(/\r/g, '');
    };
    return node;
  }

  // Mount over `parent`; returns {value()} giving the file with only changed blocks rewritten.
  function mount(parent, src, kind, onchange) {
    parent.replaceChildren();
    var blocks = parse(src, kind);
    blocks.forEach(function (b) { render(b, kind, parent, onchange); });
    var tail = el('p', null, parent); tail.className = 'pf-block pf-new'; tail.contentEditable = 'true'; tail.setAttribute('data-placeholder', 'Write something…');
    tail.addEventListener('input', onchange);
    return {
      value: function () {
        var out = '', at = 0;
        blocks.forEach(function (b) { out += src.slice(at, b.start) + (b.dirty ? b.toText() : src.slice(b.start, b.end)); at = b.end; });
        out += src.slice(at);
        var extra = kind === 'markdown' ? serialise(tail).trim() : tail.innerText.trim();
        if (extra) out = out.replace(/\n*$/, '') + '\n\n' + extra + '\n';
        return out;
      },
    };
  }
  return { parse: parse, mount: mount };
})();
