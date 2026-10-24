// Renders target/sandbox-spike/results.md from results.jsonl (Playwright lanes) and
// ios-sim/results.jsonl (simulator lane). Latest result per (lane, item, check) wins.
// Exits 1 if any required item has no result on a lane.  Usage: node render.mjs [--lanes a,b]
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const OUT = process.env.SPIKE_OUT || path.resolve(HERE, '../../target/sandbox-spike');
const argv = process.argv.slice(2);
const ALL_LANES = ['chromium-desktop', 'chromium-mobile', 'webkit-desktop', 'webkit-mobile', 'firefox-desktop', 'ios-sim'];
const lanesArg = argv.includes('--lanes') ? argv[argv.indexOf('--lanes') + 1].split(',') : ALL_LANES;
const REQUIRED = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '10', '12', '13', 'L'];
const ITEM_TITLES = {
  1: "'self' matches the part host", 2: 'Origin / Sec-Fetch-* values', 3: 'fetch, modules, import() with ACAO null',
  4: 'localStorage redefinition', 5: 'blob workers, relative-URL wrapper', 6: 'popups, clipboard, share, fullscreen',
  7: 'cookie writes fail', 8: 'no Lax cookies to siblings/workspace', 9: 'sandbox from header only',
  10: 'wrapper flow (7 checks)', 11: 'Cloudflare wildcard CNAME', 12: 'WebSocket + SSE held open, per proxy',
  13: 'WebAuthn rp.id = apex from wrapper origin', L: 'known losses (§2)',
};

function load(file) {
  if (!fs.existsSync(file)) return [];
  return fs.readFileSync(file, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l));
}
const rows = [...load(path.join(OUT, 'results.jsonl')), ...load(path.join(OUT, 'ios-sim/results.jsonl'))]
  .filter((r) => r.event === 'spike.result' && r.item !== 'login');
const latest = new Map();
for (const r of rows) latest.set(`${r.lane}\u0000${r.item}\u0000${r.check}`, r);
const res = [...latest.values()].filter((r) => lanesArg.includes(r.lane));

const meta = fs.existsSync(path.join(OUT, 'lanes.json')) ? JSON.parse(fs.readFileSync(path.join(OUT, 'lanes.json'), 'utf8')) : {};
if (fs.existsSync(path.join(OUT, 'ios-sim/lane.json'))) meta['ios-sim'] = JSON.parse(fs.readFileSync(path.join(OUT, 'ios-sim/lane.json'), 'utf8'));

const itemOrder = (i) => (i === 'L' ? 99 : +i);
const checks = [...new Set(res.map((r) => `${r.item}\u0000${r.check}`))]
  .map((k) => k.split('\u0000'))
  .sort((a, b) => itemOrder(a[0]) - itemOrder(b[0]) || a[1].localeCompare(b[1]));
const cell = (o) => ({ pass: 'pass', fail: '**FAIL**', not_assertable: 'n/a', info: 'info' }[o] || o);
const get = (lane, item, check) => latest.get(`${lane}\u0000${item}\u0000${check}`);

let md = '';
md += `Generated ${new Date().toISOString()} from \`target/sandbox-spike/results.jsonl\` and \`ios-sim/results.jsonl\`.\n\n`;
md += '### Lanes\n\n| Lane | Engine | Version | OS / runtime | Device | UA |\n| --- | --- | --- | --- | --- | --- |\n';
for (const l of lanesArg) {
  const m = meta[l] || {};
  const ua = m.ua || (res.find((r) => r.lane === l) || {}).ua || '';
  md += `| ${l} | ${m.browser || ''} | ${m.version || ''} | ${m.os || ''} | ${m.device || ''} | \`${ua.slice(0, 110)}\` |\n`;
}
md += '\n### Summary (worst outcome per item; fail > pass > n/a > info)\n\n';
md += `| Item | ${lanesArg.join(' | ')} |\n| --- |${lanesArg.map(() => ' --- |').join('')}\n`;
const missing = [];
for (const item of [...REQUIRED.filter((i) => i !== 'L'), '11', 'L'].sort((a, b) => itemOrder(a) - itemOrder(b))) {
  const cols = lanesArg.map((l) => {
    if (item === '11') return 'deferred';
    const rs = res.filter((r) => r.lane === l && r.item === item);
    if (!rs.length) { missing.push(`${l} × ${item}`); return 'MISSING'; }
    const n = (o) => rs.filter((r) => r.outcome === o).length;
    if (n('fail')) return `**FAIL** (${n('fail')}/${rs.length})`;
    if (n('pass')) return `pass (${n('pass')}/${rs.length}${n('not_assertable') ? `, ${n('not_assertable')} n/a` : ''})`;
    if (n('not_assertable')) return 'n/a';
    return 'info';
  });
  md += `| ${item} ${ITEM_TITLES[item]} | ${cols.join(' | ')} |\n`;
}
md += '\n### Every check\n\n';
md += `| Item | Check | ${lanesArg.join(' | ')} |\n| --- | --- |${lanesArg.map(() => ' --- |').join('')}\n`;
for (const [item, check] of checks) {
  md += `| ${item} | \`${check}\` | ${lanesArg.map((l) => { const r = get(l, item, check); return r ? cell(r.outcome) : '—'; }).join(' | ')} |\n`;
}
md += '\n### Details for fail, info and n/a results\n\n';
for (const [item, check] of checks) {
  const ds = lanesArg.map((l) => [l, get(l, item, check)]).filter(([, r]) => r && r.outcome !== 'pass');
  if (!ds.length) continue;
  md += `- **${item} \`${check}\`**\n`;
  for (const [l, r] of ds) md += `  - ${l} (${r.outcome}): \`${String(r.detail).replace(/`/g, "'").slice(0, 400)}\`\n`;
}
fs.writeFileSync(path.join(OUT, 'results.md'), md);
console.log(`render: wrote ${path.join(OUT, 'results.md')} (${res.length} results, ${checks.length} checks)`);
const fails = res.filter((r) => r.outcome === 'fail');
console.log(`render: ${fails.length} fail results: ${fails.map((r) => `${r.lane}:${r.item}/${r.check}`).join(', ') || 'none'}`);
if (missing.length) {
  console.error(`render: MISSING results: ${missing.join(', ')}`);
  process.exit(1);
}
