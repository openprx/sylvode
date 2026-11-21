#!/usr/bin/env node
/**
 * Browser check for the plugin Creator's review step. Right after the preview shows, a real click
 * on "I reviewed the plugin content" must check the box. A focus-driven scroll that lands while
 * the preview frame is still sizing moved the control under the pointer, so the click reached the
 * frame and the box stayed unchecked (the Creator is opened from a form toolbar: slot and form
 * in the address). Each entry point is repeated because the defect depended on timing.
 *
 * Serves the built app (`bun run build` first) with a small mock API. Not a replacement for the
 * acceptance suite: the preview here runs on the Creator's own sample data.
 *
 * Run: `bun run build && node scripts/smoke-plugin-creator-review.mjs`
 *   CHROMIUM_BIN           Chromium executable (default /usr/bin/chromium)
 *   PLUGIN_CREATOR_PORT    port of the mock server (default 18456)
 *   PLUGIN_CREATOR_RUNS    repetitions per entry point (default 8)
 *   PLAYWRIGHT_CORE        path to a playwright-core package directory
 */
import { createServer } from 'node:http';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { extname, join, normalize, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const root = resolve(new URL('..', import.meta.url).pathname);
const build = join(root, 'build');
const chromiumPath = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const port = Number(process.env.PLUGIN_CREATOR_PORT) || 18456;
const runs = Number(process.env.PLUGIN_CREATOR_RUNS) || 8;
const playwrightModule = process.env.PLAYWRIGHT_CORE
	? pathToFileURL(join(process.env.PLAYWRIGHT_CORE, 'index.mjs')).href
	: 'playwright-core';
const { chromium } = await import(playwrightModule);

if (!existsSync(build)) throw new Error(`Missing build directory: ${build}. Run bun run build.`);

const now = '2026-05-31T12:00:00.000Z';
const user = { id: 'user-creator', email: 'creator@example.com', name: 'Creator', role: 'admin' };
const types = {
	'.js': 'text/javascript',
	'.css': 'text/css',
	'.html': 'text/html',
	'.json': 'application/json',
	'.svg': 'image/svg+xml'
};
const ok = (data) => JSON.stringify({ code: 0, message: 'ok', data });
const list = (items) => ({ items, total: items.length, page: 1, per_page: 100 });

function api(pathname) {
	if (pathname === '/api/v1/auth/me') return ok({ user });
	if (pathname === '/api/v1/workspaces')
		return ok(list([{ id: 'ws1', slug: 'ws', name: 'WS', created_at: now, updated_at: now }]));
	if (pathname === '/api/v1/projects/p1')
		return ok({
			id: 'p1',
			workspace_id: 'ws1',
			name: 'P',
			key: 'P',
			created_at: now,
			updated_at: now
		});
	if (pathname === '/api/v1/projects/p1/plugin-packages')
		return ok({ items: [], bindings: [], extension_supported: true });
	return ok(list([]));
}

const server = createServer(async (req, res) => {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	if (url.pathname.startsWith('/api/')) {
		res.writeHead(200, { 'Content-Type': 'application/json' });
		res.end(api(url.pathname));
		return;
	}
	let file = normalize(join(build, decodeURIComponent(url.pathname)));
	if (!file.startsWith(build) || !existsSync(file) || file === build)
		file = join(build, 'index.html');
	res.writeHead(200, { 'Content-Type': types[extname(file)] ?? 'application/octet-stream' });
	res.end(await readFile(file));
});
await new Promise((done) => server.listen(port, '127.0.0.1', done));

const entries = {
	'project apps': '/workspace/ws1/projects/p1/settings/plugins/creator?slot=project.view',
	'forms toolbar':
		'/workspace/ws1/projects/p1/settings/plugins/creator?slot=form.list.toolbar&form=f1',
	personal: '/plugins/creator?slot=project.view'
};

const browser = await chromium.launch({ executablePath: chromiumPath, headless: true });
const failures = [];
try {
	const context = await browser.newContext({ viewport: { width: 1280, height: 1100 } });
	await context.addInitScript((who) => {
		try {
			localStorage.setItem('auth_token', 'creator-token');
			localStorage.setItem('refresh_token', 'creator-refresh');
			localStorage.setItem('auth_user', JSON.stringify(who));
			localStorage.setItem('locale', 'en');
			localStorage.setItem('theme', 'light');
		} catch {
			/* The sandboxed plugin frame has no storage. */
		}
	}, user);
	for (const [name, path] of Object.entries(entries)) {
		let passed = 0;
		for (let run = 0; run < runs; run++) {
			const page = await context.newPage();
			try {
				await page.goto(`http://127.0.0.1:${port}${path}`);
				await page.getByRole('textbox', { name: 'Plugin key' }).fill(`creator_smoke_${run}`);
				await page.getByRole('textbox', { name: 'Display name' }).fill(`Creator smoke ${run}`);
				await page.reload();
				await page.getByRole('button', { name: 'Build and preview' }).click();
				const inner = page.frameLocator('iframe').first().frameLocator('iframe').first();
				await inner
					.locator('#heading')
					.filter({ hasText: 'Data overview' })
					.waitFor({ timeout: 30_000 });
				// Same steps as the acceptance suite: bring the box into view, then click at once.
				const box = page.getByRole('checkbox', { name: /I reviewed the plugin content/ });
				await box.scrollIntoViewIfNeeded();
				await box.check({ timeout: 3_000 });
				passed++;
			} catch (error) {
				failures.push(`${name} run ${run}: ${String(error.message).split('\n')[0]}`);
			} finally {
				await page.close();
			}
		}
		console.log(
			`${passed === runs ? 'ok' : 'FAIL'}  ${name}: ${passed}/${runs} reviewed box checked`
		);
	}
} finally {
	await browser.close();
	server.close();
}
if (failures.length) {
	console.error(failures.join('\n'));
	process.exit(1);
}
