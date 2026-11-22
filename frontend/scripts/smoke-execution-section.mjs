#!/usr/bin/env node
// Browser smoke of the Execution section (the bot connection) against a mock API: connect a bot,
// see the one-time configuration block, confirm and finish it, rotate and see a new block, and
// check that nothing of the block is left in the page or in browser storage afterwards. A member
// (the reduced view) gets no section. Needs `bun run build` first and CHROMIUM_BIN.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { basename, extname, join, normalize, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const chromium = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
const workspaceId = 'ws-exec-smoke';
const projectId = 'project-exec-smoke';
const now = '2026-11-21T12:00:00.000Z';
const TOKEN_1 = `opr_${'a1'.repeat(32)}`;
const TOKEN_2 = `opr_${'b2'.repeat(32)}`;

const state = {
	connection: null,
	member: false,
	unhandled: new Set(),
	posted: null,
	rotated: false,
	reads: 0
};

const project = {
	id: projectId,
	workspace_id: workspaceId,
	name: 'Execution Smoke Project',
	key: 'EX',
	description: '',
	type_key: 'code_project',
	type_settings: {},
	created_at: now,
	updated_at: now,
	issue_counts: null
};

const ok = (data) => ({ code: 0, message: 'success', data });
const page = (items) => ({
	items,
	total: items.length,
	page: 1,
	per_page: items.length,
	total_pages: 1
});
const send = (res, status, data) => {
	res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8' });
	res.end(JSON.stringify(data));
};
async function readBody(req) {
	const chunks = [];
	for await (const chunk of req) chunks.push(chunk);
	const text = Buffer.concat(chunks).toString('utf8');
	return text ? JSON.parse(text) : {};
}

function connectionView(version, cli, directory) {
	return {
		project_id: projectId,
		version,
		enabled: true,
		bot: { id: 'bot-1', name: 'Smoke bot', token_prefix: 'opr_a1a1', token_active: true },
		runner: {
			url: 'https://runner.example.net:9090',
			webhook_id: 'hook-1',
			credentials_issued_at: now,
			shared_with_projects: [{ id: projectId, name: project.name }]
		},
		execution: {
			channel: 'tmux',
			cli,
			working_directory: directory,
			on_complete_state: 'auto',
			on_fail_label_id: null,
			approval_gate: 'none'
		},
		governance: {
			max_domain_level: 'voter',
			can_veto_human_consensus: false,
			reason_min_length: 50,
			domain_overrides: {}
		}
	};
}
const waitingStatus = {
	runner: {
		state: 'waiting',
		reported_at: null,
		credentials_used_at: null,
		version: null,
		task_protocol: null
	},
	cli: { state: 'unknown', source: 'none', version: null },
	tmux: { state: 'unknown', source: 'none', version: null },
	last_delivery: {
		state: 'none',
		at: null,
		project_key: null,
		task_id: null,
		reason_code: null,
		detail: null
	},
	computed_at: now
};
const blockOf = (token) => ({
	format: 'sylvode-runner-toml-1',
	filename: 'sylvode-runner.toml',
	bot_id: 'bot-1',
	token_prefix: token.slice(0, 8),
	issued_at: now,
	text: `[[ai_task_callback.bots]]\nbot_id = "bot-1"\ntoken = "${token}"\nsurface = "mcp_stdio"\n`
});

async function handler(req, res) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	const { pathname } = url;
	if (pathname === '/smoke-auth-seed') {
		res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
		res.end(`<!doctype html><meta charset="utf-8"><script>
localStorage.setItem('auth_token', 'smoke-access-token');
localStorage.setItem('refresh_token', 'smoke-refresh-token');
localStorage.setItem('locale', 'en');
location.replace('/workspace/${workspaceId}/projects/${projectId}/settings/execution?execution_smoke=1${url.searchParams.get('member') === '1' ? '&member=1' : ''}');
</script>`);
		return;
	}
	if (!pathname.startsWith('/api/'))
		return serveStatic(res, url.searchParams.has('execution_smoke'));
	if (pathname === '/api/v1/auth/me')
		return send(
			res,
			200,
			ok({
				user: { id: 'user-smoke', email: 'smoke@example.com', name: 'Smoke Admin', role: 'admin' }
			})
		);
	if (pathname === '/api/v1/notifications/unread-count') return send(res, 200, ok({ count: 0 }));
	if (pathname === '/api/v1/workspaces')
		return send(
			res,
			200,
			ok(
				page([
					{ id: workspaceId, slug: 'exec', name: 'Exec Smoke', created_at: now, updated_at: now }
				])
			)
		);
	if (pathname === `/api/v1/workspaces/${workspaceId}`)
		return send(
			res,
			200,
			ok({
				id: workspaceId,
				slug: 'exec',
				name: 'Exec Smoke',
				role: state.member ? 'member' : 'owner',
				created_at: now,
				updated_at: now
			})
		);
	if (pathname === `/api/v1/workspaces/${workspaceId}/projects`)
		return send(res, 200, ok(page([project])));
	if (pathname === `/api/v1/projects/${projectId}`) return send(res, 200, ok(project));
	if (pathname === `/api/v1/workspaces/${workspaceId}/bots`) return send(res, 200, ok([]));
	if (pathname === `/api/v1/workspaces/${workspaceId}/labels`) return send(res, 200, ok(page([])));
	if (pathname === `/api/v1/projects/${projectId}/workflow/effective`)
		return send(res, 200, ok({ workflow_id: 'w', source: 'system', name: 'Default', states: [] }));
	if (pathname === `/api/v1/projects/${projectId}/domains`) return send(res, 200, ok(page([])));
	const connection = `/api/v1/projects/${projectId}/bot-connection`;
	if (pathname === connection && req.method === 'GET') {
		state.reads += 1;
		if (state.member)
			return send(
				res,
				200,
				ok({
					connection: {
						bot: { id: 'bot-1', name: 'Smoke bot' },
						enabled: true,
						cli: 'claude',
						channel: 'tmux'
					}
				})
			);
		return send(
			res,
			200,
			ok(
				state.connection
					? { connection: state.connection, status: waitingStatus }
					: { connection: null, status: null }
			)
		);
	}
	if (pathname === connection && req.method === 'POST') {
		state.posted = await readBody(req);
		state.connection = connectionView(1, state.posted.cli, state.posted.working_directory);
		return send(
			res,
			200,
			ok({ connection: state.connection, status: waitingStatus, config_block: blockOf(TOKEN_1) })
		);
	}
	if (pathname === `${connection}/rotate` && req.method === 'POST') {
		state.rotated = true;
		state.connection = connectionView(
			2,
			state.connection.execution.cli,
			state.connection.execution.working_directory
		);
		return send(
			res,
			200,
			ok({ connection: state.connection, status: waitingStatus, config_block: blockOf(TOKEN_2) })
		);
	}
	state.unhandled.add(`${req.method} ${pathname}`);
	return send(res, 200, ok(null));
}

const types = {
	'.html': 'text/html; charset=utf-8',
	'.js': 'application/javascript; charset=utf-8',
	'.css': 'text/css; charset=utf-8',
	'.json': 'application/json; charset=utf-8',
	'.svg': 'image/svg+xml'
};
async function serveStatic(res, inject) {
	const req = res.req;
	const pathname = decodeURIComponent(new URL(req.url ?? '/', 'http://127.0.0.1').pathname);
	let filePath = normalize(join(buildDir, pathname === '/' ? '/index.html' : pathname));
	if (!filePath.startsWith(buildDir) || !existsSync(filePath))
		filePath = join(buildDir, 'index.html');
	let data = await readFile(filePath);
	if (inject && basename(filePath) === 'index.html')
		data = Buffer.from(
			data.toString('utf8').replace('</body>', `<script>${driver()}</script></body>`)
		);
	res.writeHead(200, { 'Content-Type': types[extname(filePath)] ?? 'application/octet-stream' });
	res.end(data);
}

/** The steps, run inside the page. Marks go to attributes on <body>; the dump shows them. */
function driver() {
	return `
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const mark = (name, value = 'done') => { document.body.setAttribute(name, value); if (name !== 'data-execution-smoke') step = name; };
let step = 'start';
async function waitFor(check, label) {
  for (let i = 0; i < 150; i += 1) { const v = check(); if (v) return v; await sleep(100); }
  throw new Error('Timed out waiting for ' + label);
}
const pageText = () => Array.from(document.body.children).filter((c) => c.tagName !== 'SCRIPT').map((c) => c.textContent).join(' ');
const has = (text) => pageText().includes(text);
const button = (text) => Array.from(document.querySelectorAll('button')).find((b) => b.textContent.trim() === text);
const field = (label) => {
  const el = Array.from(document.querySelectorAll('label')).find((l) => l.textContent.trim().startsWith(label));
  const input = el && (document.getElementById(el.getAttribute('for')) || el.querySelector('input'));
  if (!input) throw new Error('field not found: ' + label);
  return input;
};
const type = (label, value) => { const i = field(label); i.value = value; i.dispatchEvent(new Event('input', { bubbles: true })); };
const T1 = 'opr_' + 'a1'.repeat(32);
const T2 = 'opr_' + 'b2'.repeat(32);
const traceOf = (t) => pageText().includes(t) || Array.from(document.querySelectorAll('textarea,input')).some((e) => e.value.includes(t)) || storage().includes(t);
const storage = () => JSON.stringify({ ...localStorage }) + JSON.stringify({ ...sessionStorage });
(async () => {
  try {
    const member = new URLSearchParams(location.search).get('member') === '1';
    if (member) {
      await waitFor(() => has('Only workspace owners and admins'), 'the member sees no section');
      mark('data-member-refused');
      mark('data-execution-smoke', 'done');
      return;
    }
    await waitFor(() => has('No bot is connected'), 'the empty state');
    button('Connect a bot').click();
    await waitFor(() => button('Connect bot'), 'the connect form');
    type('Bot name', 'Smoke bot');
    type('Runner address', 'https://runner.example.net:9090');
    type('Working directory', '/home/dev/work/smoke');
    button('Connect bot').click();
    await waitFor(() => has('Copy this configuration now'), 'the one-time block');
    const area = document.querySelector('[data-testid="config-block-text"]') || document.querySelector('textarea');
    if (!area.value.includes(T1)) throw new Error('the block does not carry the token');
    if (!has('only in projects it is connected to')) throw new Error('the token scope is not stated');
    const done = button('Done');
    if (!done.disabled) throw new Error('Done must wait for the confirmation');
    if (!button('Copy configuration') || !button('Download file')) throw new Error('copy and download are missing');
    mark('data-block-shown');
    if (storage().includes(T1)) throw new Error('the block reached browser storage');
    document.querySelector('input[type="checkbox"]').click();
    await waitFor(() => !button('Done').disabled, 'Done after the confirmation');
    button('Done').click();
    await waitFor(() => !document.querySelector('textarea'), 'the block is gone');
    if (traceOf(T1)) throw new Error('the block left a trace after Done');
    mark('data-block-gone');
    await waitFor(() => has('Waiting for the runner'), 'the status of a new connection');
    await waitFor(() => has('Smoke bot'), 'the connected view');
    mark('data-connected-view');
    button('Rotate credentials').click();
    await waitFor(() => has('Rotate credentials of Smoke bot?'), 'the rotate confirmation');
    const confirmButtons = Array.from(document.querySelectorAll('button')).filter((b) => b.textContent.trim() === 'Rotate credentials');
    confirmButtons[confirmButtons.length - 1].click();
    await waitFor(() => has('Copy this configuration now'), 'the block after rotation');
    const second = document.querySelector('textarea');
    if (!second.value.includes(T2) || second.value.includes(T1)) throw new Error('rotation did not show the new block only');
    mark('data-rotated-block');
    document.querySelector('input[type="checkbox"]').click();
    await waitFor(() => !button('Done').disabled, 'Done after the second confirmation');
    button('Done').click();
    await waitFor(() => !document.querySelector('textarea'), 'the second block is gone');
    if (traceOf(T2) || traceOf(T1)) throw new Error('a block left a trace');
    mark('data-execution-smoke', 'done');
  } catch (error) {
    mark('data-execution-smoke', 'failed');
    const pre = document.createElement('pre');
    pre.id = 'smoke-error';
    pre.textContent = error.message + ' | after: ' + step + ' | page: ' + pageText().replace(/\\s+/g, ' ').slice(0, 300) + ' | main: ' + (document.querySelector('[data-testid="execution-section"]')?.innerText ?? 'none').replace(/\\s+/g, ' ').slice(0, 900);
    document.body.append(pre);
  }
})();`;
}

let consoleLines = [];
function runChromium(url) {
	return new Promise((resolveRun, reject) => {
		const child = spawn(
			chromium,
			[
				'--headless=new',
				'--disable-gpu',
				'--no-sandbox',
				'--disable-dev-shm-usage',
				'--virtual-time-budget=30000',
				'--enable-logging=stderr',
				'--v=0',
				'--dump-dom',
				url
			],
			{ stdio: ['ignore', 'pipe', 'pipe'] }
		);
		let out = '';
		let err = '';
		child.stdout.on('data', (c) => (out += c));
		child.stderr.on('data', (c) => (err += c));
		child.on('error', reject);
		child.on('close', (code) => {
			consoleLines = err
				.split('\n')
				.filter((line) => line.includes('CONSOLE'))
				.slice(-8);
			if (code === 0) resolveRun(out);
			else reject(new Error(`Chromium exited with ${code}: ${err}`));
		});
	});
}

if (!existsSync(buildDir))
	throw new Error(`Missing build directory: ${buildDir}. Run bun run build first.`);
if (!existsSync(chromium)) throw new Error(`Chromium binary not found: ${chromium}`);

const server = createServer((req, res) => {
	res.req = req;
	handler(req, res).catch((error) =>
		send(res, 500, { code: 500, message: error.message, data: null })
	);
});
const port = 18080 + Math.floor(Math.random() * 1000);
await new Promise((r) => server.listen(port, '127.0.0.1', r));
try {
	const dom = await runChromium(`http://127.0.0.1:${port}/smoke-auth-seed`);
	if (!dom.includes('data-execution-smoke="done"')) {
		const error =
			/<pre id="smoke-error">([\s\S]*?)<\/pre>/.exec(dom.slice(dom.indexOf('</script>')))?.[1] ??
			'no result';
		throw new Error(
			`Execution smoke failed: ${error}\nconsole: ${consoleLines.join(' | ')}\nunhandled: ${[...state.unhandled].join(', ')}\n${dom.slice(-1500)}`
		);
	}
	for (const mark of [
		'data-block-shown',
		'data-block-gone',
		'data-connected-view',
		'data-rotated-block'
	])
		if (!dom.includes(`${mark}="done"`)) throw new Error(`missing ${mark}`);
	if (state.posted?.bot?.name !== 'Smoke bot' || state.posted?.cli !== 'claude')
		throw new Error('unexpected connect request');
	if (state.posted?.working_directory !== '/home/dev/work/smoke')
		throw new Error('unexpected working directory');
	if (!state.rotated) throw new Error('no rotate request');
	// A member (the reduced view) gets no section.
	state.member = true;
	const reduced = await runChromium(`http://127.0.0.1:${port}/smoke-auth-seed?member=1`);
	if (!reduced.includes('data-member-refused="done"'))
		throw new Error('A member reached the Execution section');
	console.log('Execution section browser smoke passed');
} finally {
	server.close();
}
