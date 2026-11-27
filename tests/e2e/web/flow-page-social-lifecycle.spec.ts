import { expect, test, type APIRequestContext, type BrowserContext } from '@playwright/test';
import { flowScopePath, postWorkspaceFlowObject } from './flow-scope-support';

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
type Session = { tokens: { access_token: string; refresh_token: string }; user: { id: string } };
async function unwrap<T>(response: Awaited<ReturnType<APIRequestContext['get']>>): Promise<T> {
  expect(response.ok(), `${response.url()} HTTP ${response.status()}`).toBe(true);
  const body = await response.json();
  expect(body.code, body.message).toBe(0);
  return body.data as T;
}
async function signIn(context: BrowserContext, session: Session) {
  await context.addInitScript(({ tokens, user }) => {
    localStorage.setItem('auth_token', tokens.access_token);
    localStorage.setItem('refresh_token', tokens.refresh_token);
    localStorage.setItem('auth_user', JSON.stringify(user));
    localStorage.setItem('locale', 'en');
  }, session);
  return context.newPage();
}

test('page: comment-only block actions, public appearance, archive, restore and purge', async ({ browser, request }) => {
  test.setTimeout(120_000);
  const adminAccount = process.env.ADMIN_EMAIL && process.env.ADMIN_PASSWORD
    ? { email: process.env.ADMIN_EMAIL, password: process.env.ADMIN_PASSWORD }
    : { email: `page-admin-${runId}@e2e.sylvode.test`, password: `Page-${runId}-1!` };
  if (!process.env.ADMIN_EMAIL) {
    await unwrap(await request.post('/api/v1/auth/register', { data: { ...adminAccount, name: 'Page Admin' } }));
  }
  const admin = await unwrap<Session>(await request.post('/api/v1/auth/login', { data: adminAccount }));
  const register = async (label: string) => {
    const who = { email: `page-${label}-${runId}@e2e.sylvode.test`, password: `Page-${label}-${runId}-1!` };
    await unwrap(await request.post('/api/v1/auth/register', {
      headers: { Authorization: `Bearer ${admin.tokens.access_token}` }, data: { ...who, name: `Page ${label}` }
    }));
    return unwrap<Session>(await request.post('/api/v1/auth/login', { data: who }));
  };
  const owner = await register('owner');
  const commenter = await register('commenter');
  const auth = { Authorization: `Bearer ${owner.tokens.access_token}` };
  const memberAuth = { Authorization: `Bearer ${commenter.tokens.access_token}` };
  const workspace = await unwrap<{ id: string }>(await request.post('/api/v1/workspaces', {
    headers: auth, data: { name: `Page review ${runId}`, slug: `page-${runId}` }
  }));
  await unwrap(await request.post(`/api/v1/workspaces/${workspace.id}/members`, {
    headers: auth, data: { user_id: commenter.user.id, role: 'member' }
  }));
  await unwrap(await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
    headers: auth, data: { enabled: true, default_member_level: 'comment', idempotency_key: crypto.randomUUID() }
  }));
  const created = await unwrap<{ object: { id: string } }>(await postWorkspaceFlowObject(request, workspace.id, {
    headers: auth, data: { object_type: 'page', title: `Review page ${runId}`, idempotency_key: crypto.randomUUID() }
  }));
  const id = created.object.id;
  const url = `/workspace/${workspace.id}/flow/${id}`;
  const ownerContext = await browser.newContext();
  const memberContext = await browser.newContext();
  const publicContext = await browser.newContext();
  try {
    const page = await signIn(ownerContext, owner);
    await page.goto(url);
    const editor = page.locator('.flow-canvas .ProseMirror');
    await expect(editor).toBeVisible();
    await editor.click();
    await page.keyboard.type('A page ready for review');
    await expect(page.getByTestId('flow-sync-indicator')).toHaveAttribute('data-state', 'saved');
    await expect(page.getByTestId('flow-sync-indicator')).toHaveAttribute('data-pending', '0');

    const memberPage = await signIn(memberContext, commenter);
    await memberPage.goto(url);
    const memberEditor = memberPage.locator('.flow-canvas .ProseMirror');
    await expect(memberEditor).toHaveAttribute('contenteditable', 'false');
    await expect(memberEditor).toContainText('A page ready for review');
    await memberEditor.locator('p').hover();
    await expect(memberPage.getByRole('button', { name: 'Comment on this block', exact: true })).toBeVisible();
    await expect(memberPage.getByRole('button', {name: 'Block actions', exact: true})).toHaveCount(0);
    // Moving from the document into its gutter must not remove the anchored action.
    await memberPage.getByRole('button', { name: 'Comment on this block', exact: true }).hover();
    await expect(memberPage.getByRole('button', { name: 'Comment on this block', exact: true })).toBeVisible();
    await memberPage.getByRole('button', { name: 'Comment on this block', exact: true }).click();
    const comments = memberPage.getByTestId('flow-comments');
    await comments.getByRole('textbox').fill(`Anchored comment ${runId}`);
    await comments.getByRole('button', { name: 'Comment', exact: true }).click();
    await expect(comments).toContainText(`Anchored comment ${runId}`);
    const listed = await unwrap<{ items: Array<{ body: string; block_id: string | null }> }>(
      await request.get(`/api/v2/flow/objects/${id}/comments`, { headers: auth })
    );
    expect(listed.items.find((item) => item.body === `Anchored comment ${runId}`)?.block_id).toBeTruthy();
    await memberPage.screenshot({ path: test.info().outputPath('comment-only-block.png'), fullPage: true });
    const forbidden = await request.put(`/api/v2/flow/objects/${id}/appearance`, {
      headers: memberAuth, data: { icon: { kind: 'emoji', emoji: '🌿' } }
    });
    expect((await forbidden.json()).code).not.toBe(0);

    const imageBase64 = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aEmUAAAAASUVORK5CYII=';
    const asset = await unwrap<{ asset_id: string }>(await request.post(`/api/v2/flow/objects/${id}/appearance/assets`, {
      headers: auth, data: { data_base64: imageBase64 }
    }));
    await unwrap(await request.put(`/api/v2/flow/objects/${id}/appearance`, {
      headers: auth, data: { icon: { kind: 'emoji', emoji: '🌿' }, cover: { kind: 'upload', asset_id: asset.asset_id, position: 0.35 } }
    }));
    await page.reload();
    await expect(page.getByTestId('flow-page-cover').locator('img')).toBeVisible();
    const share = await unwrap<{ token: string; link: { id: string } }>(await request.post(`/api/v2/flow/objects/${id}/share-links`, {
      headers: auth, data: {}
    }));
    const publicPage = await publicContext.newPage();
    await publicPage.goto(`/s/${share.token}`);
    await expect(publicPage.getByTestId('flow-public-page')).toContainText('A page ready for review');
    const cover = publicPage.getByTestId('flow-public-cover').locator('img');
    await expect(cover).toBeVisible();
    await expect.poll(() => cover.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(1);
    await expect(publicPage.locator('[contenteditable=true]')).toHaveCount(0);
    await publicPage.screenshot({ path: test.info().outputPath('public-page-with-cover.png'), fullPage: true });

    const command = async (type: 'archive' | 'restore') => unwrap(await request.post(`/api/v2/flow/objects/${id}/commands`, {
      headers: auth, data: { command: { type, payload: {} }, idempotency_key: crypto.randomUUID() }
    }));
    await command('archive');
    let trash = await unwrap<{ items: Array<{ id: string }> }>(await request.get(`${await flowScopePath(request, workspace.id, auth)}/trash`, { headers: auth }));
    expect(trash.items.some((item) => item.id === id)).toBe(true);
    await command('restore');
    await page.reload();
    await expect(editor).toContainText('A page ready for review');
    await expect(page.getByTestId('flow-page-cover').locator('img')).toBeVisible();
    trash = await unwrap(await request.get(`${await flowScopePath(request, workspace.id, auth)}/trash`, { headers: auth }));
    expect(trash.items.some((item) => item.id === id)).toBe(false);
    await command('archive');
    const purged = await unwrap<{ deleted_id: string; deleted_count: number }>(await request.post(`/api/v2/flow/objects/${id}/purge`, { headers: auth, data: {} }));
    expect(purged).toEqual({ deleted_id: id, deleted_count: 1 });
    const gone = await request.get(`/api/v2/flow/objects/${id}`, { headers: auth });
    expect((await gone.json()).code).not.toBe(0);
    const assetGone = await request.get(`/api/v2/flow/public-shares/${share.token}/assets/${asset.asset_id}`);
    expect(assetGone.headers()['content-type']).toContain('application/json');
    expect(await assetGone.json()).toMatchObject({ code: 404, data: null, error_code: 'not_found' });
  } finally {
    await publicContext.close();
    await memberContext.close();
    await ownerContext.close();
  }
});
