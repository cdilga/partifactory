import { test, expect, waitForLog } from './harness/fixtures';
import { signIn } from './lib/public-auth';
import { oauthToken } from './lib/agent-tokens';

// The source editor (pf-h6y.5): an Edit member changes a Markdown part in the browser, an agent
// sees the change, a concurrent agent edit raises the conflict notice, and View sees no Edit.
test('an Edit member changes a Markdown part in the browser, with drafts and conflicts', async ({ page, browser }, info) => {
  test.skip(!['chromium-desktop', 'webkit-mobile', 'firefox-desktop'].includes(info.project.name), 'Fixed part hosts exist for these lanes');
  test.setTimeout(150000);
  const name = `editor-${info.project.name}`;
  const open = async (who: string) => {
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    const p = await context.newPage();
    await signIn(p, who);
    return { context, page: p };
  };
  const editor = await open('member-b');
  const viewer = await open('member-a');
  await signIn(page, 'admin');
  const token = (await oauthToken(page, page.request, 'Editor gate', ['parts:read', 'parts:write', 'share:household', 'comments:write'])).access_token;
  const call = async (op: string, data: unknown, ok = true) => {
    const r = await page.request.post(`/api/v1/${op}`, { headers: { Authorization: `Bearer ${token}` }, data });
    if (ok) expect(r.status(), await r.text()).toBe(200);
    return r.json();
  };
  const created = await page.request.post('/api/v1/part_create', { headers: { Authorization: `Bearer ${token}` }, data: { name, title: `Notes ${info.project.name}`, kind: 'markdown', content: '# Shopping  list\n\n- milk\n\n| Item | Qty |\n|---|--:|\n| eggs | 6 |\n', request_id: `${name}-${Date.now()}` } });
  expect([200, 409].includes(created.status()), await created.text()).toBeTruthy();
  await call('part_share', { name, action: 'set', subject: 'user:member-b', role: 'edit' });
  await call('part_share', { name, action: 'set', subject: 'user:member-a', role: 'view' });

  await viewer.page.goto(`https://${name}-part.pf.test/`);
  await expect(viewer.page.getByRole('button', { name: 'Share', exact: true })).toBeVisible();
  await expect(viewer.page.getByRole('button', { name: 'Edit', exact: true })).toBeHidden();

  const p = editor.page;
  await p.goto(`https://${name}-part.pf.test/`);
  await p.getByRole('button', { name: 'Edit', exact: true }).click();
  // Markdown opens in the formatted view (pf-h6y.6): edit a list item and add a table row with
  // Tab; the save is block ops, so the untouched heading keeps its exact bytes.
  const doc = p.locator('#pf-editor .ProseMirror');
  await expect(doc.locator('h1')).toHaveText('Shopping');
  await doc.locator('li').first().click();
  await p.keyboard.press('End');
  await p.keyboard.type(' (2 litres)');
  await doc.locator('td').last().click({ position: { x: 4, y: 4 } });
  await p.keyboard.press('End');
  await p.keyboard.press('Tab');
  await p.keyboard.type('bread');
  await p.keyboard.press('Tab');
  await p.keyboard.type('1');
  await expect(p.locator('#pf-editor [role=status]')).toHaveText('Draft saved.');
  await p.locator('#pf-editor').getByRole('button', { name: 'Save', exact: true }).click();
  await expect(p.locator('#pf-editor [role=status]')).toContainText('Saved as version');
  await expect.poll(async () => (await call('part_get', { name })).content['index.md']).toBe('# Shopping  list\n\n- milk (2 litres)\n\n| Item | Qty |\n| --- | --: |\n| eggs | 6 |\n| bread | 1 |\n');
  await p.screenshot({ path: info.outputPath('editor-formatted.png'), fullPage: true });
  // Select a word: the floating toolbar offers Comment, which opens the rail on that quote.
  await doc.locator('h1').dblclick();
  await expect(p.locator('#pf-editor .pf-toolbar')).toBeVisible();
  await p.screenshot({ path: info.outputPath('editor-toolbar.png') });
  await p.locator('#pf-editor .pf-toolbar').getByRole('button', { name: 'Comment', exact: true }).click();
  await expect(p.locator('#pf-comments .pf-on')).toBeVisible();
  await p.getByLabel('Add a comment').fill('Which shop?');
  await p.getByRole('button', { name: 'Comment', exact: true }).last().click();
  await expect.poll(async () => {
    const t = (await call('part_comments', { name })).threads.find((x: any) => x.comment.body === 'Which shop?');
    return t ? [t.quote, typeof t.block === 'string' && t.block.length === 8] : null;
  }).toEqual(['Shopping', true]);
  await p.keyboard.press('Escape');
  await p.getByRole('button', { name: 'Source', exact: true }).click();
  const source = p.getByLabel('Source');
  await expect(source).toHaveValue(/- milk \(2 litres\)/);
  await source.fill('# Shopping  list\n\n- milk (2 litres)\n- bread\n');
  await expect(p.locator('#pf-editor [role=status]')).toHaveText('Draft saved.');
  // Reopening restores the draft.
  await p.getByRole('button', { name: 'Close', exact: true }).last().click();
  await p.getByRole('button', { name: 'Edit', exact: true }).click();
  await p.getByRole('button', { name: 'Source', exact: true }).click();
  await expect(source).toHaveValue(/- bread/);
  await expect(p.locator('#pf-editor [role=status]')).toContainText('Restored your unsaved changes');
  await p.locator('#pf-editor').getByRole('button', { name: 'Save', exact: true }).click();
  await expect(p.locator('#pf-editor [role=status]')).toContainText('Saved as version');
  await p.screenshot({ path: info.outputPath('editor-saved.png'), fullPage: true });
  await expect.poll(async () => (await call('part_get', { name })).content['index.md']).toContain('- bread');

  // Someone else saves while this person is editing: the save stops and offers a choice.
  await source.fill('# Shopping  list\n\n- milk (2 litres)\n- bread\n- eggs\n');
  const got = await call('part_get', { name });
  await call('part_edit', { name, edits: [{ op: 'replace', old_str: '# Shopping  list', new_str: '# Shopping' }], request_id: `${name}-agent-${Date.now()}` });
  expect(got.head).toBeGreaterThan(1);
  await p.locator('#pf-editor').getByRole('button', { name: 'Save', exact: true }).click();
  await expect(p.locator('.pf-conflict')).toBeVisible();
  await p.getByRole('button', { name: 'Save mine anyway', exact: true }).click();
  await expect(p.locator('#pf-editor [role=status]')).toContainText('Saved as version');
  await expect.poll(async () => (await call('part_get', { name })).content['index.md']).toContain('- eggs');
  expect((await waitForLog({ event: 'part.edit' })).some(l => l.surface === 'web')).toBe(true);
  await editor.context.close();
  await viewer.context.close();
});
