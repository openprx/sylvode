#!/usr/bin/env node
// Browser behaviour checks for the batch B primitives (ContextLine, TaskTabStrip, Toast,
// SidePanel, DataTable, PageHeader), run against the internal test page `/admin/ui` in headless
// Chromium with real key presses. Same set-up as `ui-behaviour.mjs`.
//
//   node scripts/ui-behaviour-batch-b.mjs --base-url http://127.0.0.1:18242 --api http://127.0.0.1:18083
//
// Read-only: every non-GET request to the API is aborted. Credentials come from the environment
// (P3X_U1_EMAIL, P3X_U1_PASSWORD) or from the env file named by UI_SHOTS_ENV_FILE; they are never
// printed. Browser: the local open-source Chromium (CHROMIUM_BIN overrides the path).

import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';

const DEFAULT_CHROMIUM = '/Applications/Chromium.app/Contents/MacOS/Chromium';

function parseFlags(argv) {
	const flags = {};
	for (let i = 0; i < argv.length; i += 1)
		if (argv[i].startsWith('--')) flags[argv[i].slice(2)] = argv[i + 1] ?? true;
	return flags;
}

async function credentials() {
	let email = process.env.P3X_U1_EMAIL;
	let password = process.env.P3X_U1_PASSWORD;
	const file = process.env.UI_SHOTS_ENV_FILE;
	if ((!email || !password) && file && existsSync(file)) {
		for (const line of (await readFile(file, 'utf8')).split('\n')) {
			const match = line.match(/^(?:export\s+)?(\w+)=(.*)$/);
			if (!match) continue;
			const value = match[2].trim().replace(/^['"]|['"]$/g, '');
			if (match[1] === 'P3X_U1_EMAIL') email ||= value;
			if (match[1] === 'P3X_U1_PASSWORD') password ||= value;
		}
	}
	if (!email || !password) throw new Error('no credentials (P3X_U1_EMAIL, P3X_U1_PASSWORD)');
	return { email, password };
}

async function signIn(api) {
	const response = await fetch(`${api}/api/v1/auth/login`, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify(await credentials())
	});
	const body = await response.json().catch(() => null);
	if (!response.ok || body?.code !== 0 || !body?.data?.tokens?.access_token)
		throw new Error(`sign-in failed with HTTP ${response.status}`);
	return body.data;
}

const flags = parseFlags(process.argv.slice(2));
const baseUrl = String(flags['base-url'] || '').replace(/\/$/, '');
const api = String(flags.api || 'http://127.0.0.1:18083').replace(/\/$/, '');
if (!baseUrl) throw new Error('--base-url is required');

const { chromium } = await import('playwright-core');
const executablePath = process.env.CHROMIUM_BIN || DEFAULT_CHROMIUM;
if (!existsSync(executablePath)) throw new Error(`Chromium not found at ${executablePath}`);

const session = await signIn(api);
const browser = await chromium.launch({ executablePath, headless: true });
const context = await browser.newContext({
	viewport: { width: 1440, height: 900 },
	locale: 'en-US'
});
await context.addInitScript(
	({ tokens, user }) => {
		try {
			localStorage.setItem('locale', 'en');
			localStorage.setItem('theme', 'light');
			localStorage.setItem('auth_token', tokens.access_token);
			localStorage.setItem('refresh_token', tokens.refresh_token);
			localStorage.setItem('auth_user', JSON.stringify(user));
		} catch {
			// storage unavailable
		}
	},
	{ tokens: session.tokens, user: session.user }
);
await context.route('**/api/**', (route) => {
	const req = route.request();
	const method = req.method();
	if (
		method === 'GET' ||
		method === 'HEAD' ||
		method === 'OPTIONS' ||
		new URL(req.url()).pathname === '/api/v1/auth/refresh'
	)
		return route.continue();
	return route.abort('blockedbyclient');
});

const page = await context.newPage();
async function load(width = 1440) {
	await page.setViewportSize({ width, height: 900 });
	await page.goto(`${baseUrl}/admin/ui`);
	await page.waitForSelector('[data-gallery-section="data-table"]');
}
await load();

let failures = 0;
const results = [];
async function check(name, body) {
	try {
		const detail = await body();
		results.push(`ok   ${name}${detail ? ` -- ${detail}` : ''}`);
	} catch (error) {
		failures += 1;
		results.push(`FAIL ${name} -- ${error instanceof Error ? error.message : error}`);
	}
}
function expect(condition, message) {
	if (!condition) throw new Error(message);
}

const active = () =>
	page.evaluate(() => {
		const el = document.activeElement;
		return el
			? {
					tag: el.tagName.toLowerCase(),
					role: el.getAttribute('role'),
					testid: el.getAttribute('data-testid'),
					text: (el.textContent || '').trim().slice(0, 40),
					type: el.getAttribute('type'),
					inDialog: !!el.closest('[role="dialog"]'),
					inGallery: el.closest('[data-gallery]')?.getAttribute('data-gallery') ?? null,
					inToast: !!el.closest('[data-toast]'),
					inMenu: !!el.closest('[role="menu"]'),
					key: el.getAttribute('data-tab-key')
				}
			: null;
	});

/** Moves focus to `selector` with the keyboard: focus a stop before it, then press Tab. */
async function tabTo(selector, max = 400) {
	await page.evaluate(() => {
		const el = document.activeElement;
		if (el && el !== document.body) el.blur();
	});
	for (let i = 0; i < max; i += 1) {
		await page.keyboard.press('Tab');
		const hit = await page.evaluate((s) => document.activeElement?.matches(s) ?? false, selector);
		if (hit) return;
	}
	throw new Error(`Tab never reached ${selector}`);
}
/** Scripts focus onto `selector` (for starting points; the key presses after it are real). */
async function focusEl(selector) {
	await page.locator(selector).first().scrollIntoViewIfNeeded();
	await page.evaluate((s) => document.querySelector(s)?.focus(), selector);
	const ok = await page.evaluate((s) => document.activeElement?.matches(s) ?? false, selector);
	if (!ok) throw new Error(`could not focus ${selector}`);
}
const tokenColour = (name) =>
	page.evaluate((n) => {
		const probe = document.createElement('i');
		probe.style.color = `var(${n})`;
		document.body.appendChild(probe);
		const value = getComputedStyle(probe).color;
		probe.remove();
		return value;
	}, name);
const wait = (ms) => page.waitForTimeout(ms);

// ---------------------------------------------------------------- context line
const CL = '[data-gallery="context-line:default"]';
await check(
	'B1 context line: nav, ol with n+1 items, aria-current on a last item without a link',
	async () => {
		const r = await page.evaluate((root) => {
			const nav = document.querySelector(`${root} nav[aria-label]`);
			const items = [...nav.querySelectorAll('ol > li')];
			const last = items.at(-1);
			return {
				label: nav.getAttribute('aria-label'),
				count: items.length,
				current:
					last.querySelector('[aria-current="page"]') !== null || last.getAttribute('aria-current'),
				link: last.querySelector('a') !== null,
				weight: getComputedStyle(last).fontWeight
			};
		}, CL);
		expect(r.label === 'Location' && r.count === 5 && r.current && !r.link, JSON.stringify(r));
		expect(r.weight === '500', `current weight ${r.weight}`);
		return JSON.stringify(r);
	}
);
await check('B3 context line: link focus ring is 2px with offset 2px', async () => {
	await tabTo(`${CL} a`);
	const o = await page.evaluate(() => {
		const s = getComputedStyle(document.activeElement);
		return { width: s.outlineWidth, offset: s.outlineOffset, colour: s.color };
	});
	expect(o.width === '2px' && o.offset === '2px', JSON.stringify(o));
	return JSON.stringify(o);
});
await check(
	'B2 context line: at 340 one "..." button; Enter opens the menu on an item; Escape returns',
	async () => {
		const COLLAPSED = '[data-gallery="context-line:collapsed"]';
		const count = await page.locator(`${COLLAPSED} nav button[aria-haspopup="menu"]`).count();
		expect(count === 1, `buttons ${count}`);
		const visible = await page.evaluate(
			(root) =>
				[...document.querySelectorAll(`${root} nav ol > li`)].map((li) => li.textContent.trim()),
			COLLAPSED
		);
		expect(
			visible[0] === 'Acme Website' && visible.at(-1) === 'Northwind order',
			JSON.stringify(visible)
		);
		await tabTo(`${COLLAPSED} nav button[aria-haspopup="menu"]`);
		await page.keyboard.press('Enter');
		await page.waitForSelector('[role="menu"]');
		await page.waitForFunction(() => document.activeElement?.getAttribute('role') === 'menuitem');
		const items = await page.locator('[role="menu"] [role="menuitem"]').allInnerTexts();
		await page.keyboard.press('Escape');
		await page.waitForSelector('[role="menu"]', { state: 'detached' });
		const back = await page.evaluate(() => document.activeElement?.getAttribute('aria-haspopup'));
		expect(back === 'menu', `focus after Escape: ${back}`);
		return `visible ${JSON.stringify(visible)}, menu ${JSON.stringify(items.map((s) => s.trim()))}`;
	}
);
await check(
	'context line: a restricted ancestor is plain text with a lock; an unavailable one says so',
	async () => {
		const r = await page.evaluate(() => {
			const root = document.querySelector('[data-gallery="context-line:restricted"] nav');
			return {
				links: root.querySelectorAll('a').length,
				text: root.textContent.replace(/\s+/g, ' ').trim()
			};
		});
		expect(
			r.links === 1 && r.text.includes('Restricted page') && r.text.includes('Unavailable'),
			JSON.stringify(r)
		);
	}
);

// ---------------------------------------------------------------- task tab strip
const TS = '[data-gallery="task-tab-strip:default"]';
const tabKeys = () =>
	page.evaluate(
		(root) =>
			[...document.querySelectorAll(`${root} [role="tab"]`)].map((t) =>
				t.getAttribute('data-tab-key')
			),
		TS
	);
await check(
	'S1 tab strip: Tab enters on the selected tab, ArrowRight moves focus and selection',
	async () => {
		await tabTo(`${TS} [role="tab"]`);
		const first = await active();
		expect(first.role === 'tab' && first.key === 'a-0', JSON.stringify(first));
		const stops = await page.evaluate(
			(root) =>
				[...document.querySelectorAll(`${root} [role="tab"]`)].filter((t) => t.tabIndex === 0)
					.length,
			TS
		);
		expect(stops === 1, `tab stops ${stops}`);
		await page.keyboard.press('ArrowRight');
		const next = await active();
		const selected = await page.evaluate(
			(root) =>
				document
					.querySelector(`${root} [role="tab"][aria-selected="true"]`)
					?.getAttribute('data-tab-key'),
			TS
		);
		expect(next.key === 'a-1' && selected === 'a-1', `${next.key} / ${selected}`);
		await page.keyboard.press('End');
		expect((await active()).key === 'a-4', 'End');
		await page.keyboard.press('ArrowRight');
		expect((await active()).key === 'a-0', 'wraps to the first tab');
		return 'one tab stop; arrows, End and wrap work';
	}
);
await check('S5 tab strip: the active tab carries a 2px primary bar', async () => {
	const r = await page.evaluate((root) => {
		const tab = document.querySelector(`${root} [role="tab"][aria-selected="true"]`);
		const s = getComputedStyle(tab, '::after');
		return { colour: s.backgroundColor, height: s.height };
	}, TS);
	const primary = await tokenColour('--rf-primary');
	expect(r.colour === primary && r.height === '2px', `${JSON.stringify(r)} vs ${primary}`);
	return JSON.stringify(r);
});
await check(
	'tab strip: markers are named in text, long titles are cut with an ellipsis',
	async () => {
		const r = await page.evaluate((root) => {
			const names = [...document.querySelectorAll(`${root} [role="tab"]`)].map((t) =>
				t.getAttribute('aria-label')
			);
			const label = document.querySelector(`${root} [data-tab-key="a-1"] .label`);
			return { names, cut: label.scrollWidth > label.clientWidth };
		}, TS);
		expect(
			r.names[1].includes('unsaved changes') && r.names[2].includes('new changes'),
			JSON.stringify(r.names)
		);
		expect(r.cut, 'long title is truncated');
		return JSON.stringify(r.names);
	}
);
await check('tab strip: Alt+Shift+Arrow moves the tab and announces it', async () => {
	await focusEl(`${TS} [role="tab"][data-tab-key="a-2"]`);
	await page.keyboard.press('Alt+Shift+ArrowRight');
	const keys = await tabKeys();
	expect(keys.join() === 'a-0,a-1,a-3,a-2,a-4', `order ${keys.join()}`);
	const heldFocus = await active();
	expect(heldFocus.key === 'a-2', `focus stays on the moved tab: ${JSON.stringify(heldFocus)}`);
	const text = await page.locator(`${TS} [data-testid="task-tab-reorder-status"]`).innerText();
	expect(/moved to position 4 of 5/.test(text), text);
	await page.keyboard.press('Alt+Shift+ArrowLeft');
	expect((await tabKeys()).join() === 'a-0,a-1,a-2,a-3,a-4', 'moved back');
	return text.trim();
});
await check('tab strip: right-click and Shift+F10 open the tab menu; Move left works', async () => {
	await focusEl(`${TS} [role="tab"][data-tab-key="a-2"]`);
	await page.keyboard.press('Shift+F10');
	await page.waitForSelector('[role="menu"]');
	const items = (await page.locator('[role="menu"] [role="menuitem"]').allInnerTexts()).map((s) =>
		s.trim()
	);
	expect(items.join('|') === 'Close|Close other saved tabs|Move left|Move right', items.join('|'));
	await page.keyboard.press('Escape');
	await page.waitForSelector('[role="menu"]', { state: 'detached' });
	return items.join(', ');
});
await check(
	'S4 tab strip: closing an unsaved tab opens the discard dialog on Keep; Escape keeps the tab',
	async () => {
		await focusEl(`${TS} [role="tab"][data-tab-key="a-1"]`);
		await page.keyboard.press('Delete');
		await page.waitForSelector('[data-testid="task-close-confirm"]');
		const role = await page.getAttribute('[data-testid="task-close-confirm"]', 'role');
		await page.waitForFunction(() => document.activeElement?.textContent?.trim() === 'Keep');
		const a = await active();
		expect(role === 'dialog' && a.text === 'Keep' && a.inDialog, `${role} ${JSON.stringify(a)}`);
		await page.keyboard.press('Tab');
		expect((await active()).text === 'Discard and close', 'Tab reaches Discard');
		await page.keyboard.press('Tab');
		expect((await active()).inDialog, 'Tab stays inside the dialog');
		await page.keyboard.press('Escape');
		await page.waitForSelector('[data-testid="task-close-confirm"]', { state: 'detached' });
		expect((await tabKeys()).includes('a-1'), 'tab still present');
		const back = await active();
		expect(back.role === 'tab', `focus returned to ${JSON.stringify(back)}`);
		return 'role=dialog, first focus Keep, Escape keeps the tab and returns focus to a tab';
	}
);
await check(
	'tab strip: Discard and close removes the tab; a saved tab closes at once',
	async () => {
		await focusEl(`${TS} [role="tab"][data-tab-key="a-1"]`);
		await page.keyboard.press('Delete');
		await page.waitForSelector('[data-testid="task-close-confirm"]');
		await page.getByRole('button', { name: 'Discard and close' }).click();
		await page.waitForSelector('[data-testid="task-close-confirm"]', { state: 'detached' });
		expect(!(await tabKeys()).includes('a-1'), 'dirty tab gone');
		await focusEl(`${TS} [role="tab"][data-tab-key="a-4"]`);
		await page.keyboard.press('Delete');
		expect(!(await tabKeys()).includes('a-4'), 'saved tab closed without a dialog');
		expect((await page.locator('[data-testid="task-close-confirm"]').count()) === 0, 'no dialog');
	}
);
await check('tab strip: seven tabs at 724 show "+n" and the active tab stays visible', async () => {
	const OV = '[data-gallery="task-tab-strip:overflow"]';
	const r = await page.evaluate((root) => {
		const el = document.querySelector(root);
		const tabs = [...el.querySelectorAll('[role="tab"]')];
		return {
			width: el.getBoundingClientRect().width,
			tabs: tabs.length,
			activeVisible: tabs.some((t) => t.getAttribute('aria-selected') === 'true'),
			overflow: el.querySelector('[data-testid="task-tab-overflow"]').textContent.trim(),
			tabWidth: Math.round(tabs[0].parentElement.getBoundingClientRect().width)
		};
	}, OV);
	expect(
		r.tabs < 7 && r.activeVisible && /^\+\d+$/.test(r.overflow) && r.tabWidth >= 120,
		JSON.stringify(r)
	);
	await page.locator(`${OV} [data-testid="task-tab-overflow"]`).click();
	await page.waitForSelector('[data-testid="task-tab-overflow-menu"]');
	const items = await page.locator('[data-testid="task-tab-overflow-item"]').count();
	expect(items === 7, `menu lists ${items} tabs`);
	await page.keyboard.press('Escape');
	await page.waitForSelector('[role="menu"]', { state: 'detached' });
	return JSON.stringify(r);
});
await check('tab strip: below 600 only the active tab and a "{n} tabs" button remain', async () => {
	const r = await page.evaluate(() => {
		const el = document.querySelector('[data-gallery="task-tab-strip:collapsed"]');
		return {
			tabs: el.querySelectorAll('[role="tab"]').length,
			mode: el.querySelector('[role="tablist"]').getAttribute('data-mode'),
			button: el.querySelector('[data-testid="task-tab-overflow"]').textContent.trim()
		};
	});
	expect(r.tabs === 1 && r.mode === 'collapsed' && r.button === '5 tabs', JSON.stringify(r));
});
await check(
	'S3 tab strip: all unsaved and a new item pending opens the blocking menu; Escape cancels it',
	async () => {
		const BL = '[data-gallery="task-tab-strip:blocking"]';
		await page.locator(`${BL} [data-testid="tab-blocking-open"] button`).click();
		await page.waitForSelector('[data-testid="task-tab-limit"]');
		await page.waitForFunction(() =>
			document.activeElement?.closest('[data-testid="task-tab-limit"]')
		);
		const a = await active();
		const heading = await page.locator('[data-testid="task-tab-limit"]').innerText();
		expect(
			a.role === 'menuitemradio' && heading.includes('Close one to continue'),
			JSON.stringify(a)
		);
		await page.keyboard.press('Escape');
		await page.waitForSelector('[data-testid="task-tab-limit"]', { state: 'detached' });
		const log = await page.getAttribute(`${BL} .demo`, 'data-log');
		expect(log === 'cancel', `pending resolved with ${log}`);
		await page.locator(`${BL} [data-testid="tab-blocking-open"] button`).click(); // toggles off
		return 'blocking menu focused on the first tab; Escape resolved the pending open with cancel';
	}
);

// ---------------------------------------------------------------- toast
const showButton = (kind) => page.locator(`[data-gallery="toast:${kind}"] button`).first();
async function clearToasts() {
	// Clicks inside the page: a toast may expire on its own while the click is being prepared.
	await page.evaluate(() =>
		document.querySelectorAll('[data-toast-close]').forEach((b) => b.click())
	);
	await page.waitForFunction(() => !document.querySelector('[data-toast]'), null, {
		timeout: 5000
	});
}
const undoCalls = () => page.getAttribute('[data-gallery="toast:calls"]', 'data-calls');
await check(
	'T1 toast: the live region exists before any toast and carries message plus "Undo available"',
	async () => {
		await clearToasts();
		const before = await page.evaluate(() => {
			const el = document.querySelector('[data-toast-live]');
			return el
				? {
						role: el.getAttribute('role'),
						live: el.getAttribute('aria-live'),
						atomic: el.getAttribute('aria-atomic'),
						text: el.textContent.trim()
					}
				: null;
		});
		expect(
			before && before.role === 'status' && before.live === 'polite' && before.text === '',
			JSON.stringify(before)
		);
		await showButton('undo').click();
		await page.waitForFunction(() =>
			document.querySelector('[data-toast-live]').textContent.includes('Undo')
		);
		const text = await page.locator('[data-toast-live]').innerText();
		expect(text.includes('Deleted “Northwind order”.') && text.includes('Undo available'), text);
		const landmark = await page.evaluate(
			() => document.querySelector('section[aria-label="Notifications"]') !== null
		);
		expect(landmark, 'notifications landmark');
		return text.trim();
	}
);
await check(
	'T3 toast: Ctrl+Z outside a text field runs Undo; inside a field it does nothing',
	async () => {
		await page.evaluate(() => document.activeElement?.blur());
		const calls0 = Number(await undoCalls());
		await page.keyboard.press('Control+z');
		await page.waitForFunction(
			(n) => Number(document.querySelector('[data-gallery="toast:calls"]').dataset.calls) === n + 1,
			calls0
		);
		await clearToasts();
		await showButton('undo').click();
		await page.locator('[data-gallery="toast:field"] input').focus();
		await page.keyboard.press('Control+z');
		await wait(250);
		const calls = Number(await undoCalls());
		expect(calls === calls0 + 1, `calls ${calls0} -> ${calls}`);
		await page.evaluate(() => document.activeElement?.blur());
		await page.keyboard.press('Control+z');
		await page.waitForFunction(
			(n) => Number(document.querySelector('[data-gallery="toast:calls"]').dataset.calls) === n + 2,
			calls0
		);
		return `one call from the page, none from the field (${calls0} -> ${calls0 + 2})`;
	}
);
await check(
	'toast: Alt+Shift+N reaches the toast, Escape dismisses it and focus goes back',
	async () => {
		await clearToasts();
		await showButton('result').focus();
		await showButton('result').click();
		await page.waitForSelector('[data-toast]');
		await page.locator('[data-gallery="toast:result"] button').focus();
		await page.keyboard.press('Alt+Shift+N');
		await page.waitForFunction(() => document.activeElement?.closest('[data-toast]'));
		const a = await active();
		expect(a.inToast && a.text === 'Open', JSON.stringify(a));
		await page.keyboard.press('Tab');
		const second = await page.evaluate(() =>
			document.activeElement?.hasAttribute('data-toast-close')
		);
		expect(second, 'Tab moves from the action to Dismiss');
		await page.keyboard.press('Escape');
		await page.waitForFunction(() => !document.querySelector('[data-toast]'));
		const back = await active();
		expect(back.inGallery === 'toast:result', `focus returned to ${JSON.stringify(back)}`);
		return 'action then close, Escape returned focus to the button that had it';
	}
);
await check('toast: Alt+Shift+N is ignored while a text field has focus', async () => {
	await showButton('neutral').click();
	await page.waitForSelector('[data-toast]');
	await page.locator('[data-gallery="toast:field"] input').focus();
	await page.keyboard.press('Alt+Shift+N');
	await wait(150);
	const a = await active();
	expect(!a.inToast && a.tag === 'input', JSON.stringify(a));
	await clearToasts();
});
await check(
	'T4 toast geometry: 24px from the corner and 360 wide at 1440; 358 at 390',
	async () => {
		await showButton('neutral').click();
		await page.waitForSelector('[data-toast]');
		await wait(300);
		const read = () =>
			page.evaluate(() => {
				const el = document.querySelector('[data-toast]');
				const r = el.getBoundingClientRect();
				const root = document.documentElement;
				return {
					right: root.clientWidth - r.right,
					bottom: root.clientHeight - r.bottom,
					width: r.width,
					z: getComputedStyle(el.closest('section')).zIndex
				};
			});
		const wide = await read();
		expect(
			wide.right === 24 && wide.bottom === 24 && wide.width === 360 && wide.z === '45',
			JSON.stringify(wide)
		);
		await page.setViewportSize({ width: 390, height: 900 });
		await wait(200);
		const narrow = await read();
		expect(narrow.width === 358 && narrow.bottom === 16, JSON.stringify(narrow));
		await page.setViewportSize({ width: 1440, height: 900 });
		await clearToasts();
		return `${JSON.stringify(wide)} / ${JSON.stringify(narrow)}`;
	}
);
await check('T2 toast timing: neutral 6 s, Undo 10 s, hover pauses', async () => {
	await clearToasts();
	await page.evaluate(() => document.activeElement?.blur());
	const started = Date.now();
	await showButton('neutral').click();
	await showButton('undo').click();
	const present = (kind) =>
		page.evaluate(
			(k) => [...document.querySelectorAll('[data-toast]')].some((t) => t.textContent.includes(k)),
			kind
		);
	await wait(5200 - (Date.now() - started));
	expect((await present('Export finished')) && (await present('Deleted')), 'both at 5 s');
	await wait(6600 - (Date.now() - started));
	expect(!(await present('Export finished')), 'neutral gone after 6 s');
	expect(await present('Deleted'), 'undo toast still there at 6.6 s');
	await wait(10800 - (Date.now() - started));
	expect(!(await present('Deleted')), 'undo toast gone after 10 s');
	// Hover pauses a neutral toast.
	await showButton('neutral').click();
	const t0 = Date.now();
	await page.waitForSelector('[data-toast]');
	await wait(4000);
	const box = await page.locator('[data-toast]').first().boundingBox();
	await page.mouse.move(box.x + 20, box.y + 20);
	await wait(8200 - (Date.now() - t0));
	expect(await present('Export finished'), 'still there at 8 s while hovered');
	await page.mouse.move(5, 5);
	await wait(2600);
	expect(!(await present('Export finished')), 'gone about 2 s after the pointer left');
	return 'neutral gone between 5.2 and 6.6 s, undo toast gone between 6.6 and 10.8 s, hover paused';
});
await check('toast: a failed Undo becomes a warning with Retry and stays', async () => {
	await clearToasts();
	await page.locator('[data-gallery="toast:fail"] input').check();
	await showButton('undo').click();
	await page.waitForSelector('[data-toast]');
	await page.locator('[data-toast-action]').click();
	await page.waitForFunction(
		() => document.querySelector('[data-toast]')?.getAttribute('data-tone') === 'warning'
	);
	const r = await page.evaluate(() => {
		const el = document.querySelector('[data-toast]');
		return {
			text: el.textContent.replace(/\s+/g, ' ').trim(),
			action: el.querySelector('[data-toast-action]')?.textContent.trim()
		};
	});
	expect(r.action === 'Retry' && r.text.includes('Could not restore'), JSON.stringify(r));
	await wait(11000);
	expect((await page.locator('[data-toast]').count()) === 1, 'persistent: still there after 11 s');
	await page.locator('[data-gallery="toast:fail"] input').uncheck();
	await clearToasts();
	return JSON.stringify(r);
});

// ---------------------------------------------------------------- side panel
const panel = (id) => `[data-gallery="side-panel:${id}"]`;
async function openPanel(id) {
	await focusEl(`[data-testid="panel-opener-${id}"]`);
	await page.keyboard.press('Enter');
	await page.waitForSelector(`${panel(id)} [data-ui="side-panel"]`);
	await page.waitForFunction(
		(root) => document.activeElement?.closest(`${root} [data-ui="side-panel"]`),
		panel(id)
	);
}
async function closePanel(id) {
	// Escape closes the topmost layer first (a tooltip on a focused tool), the panel after it.
	for (let i = 0; i < 3; i += 1) {
		await page.keyboard.press('Escape');
		await wait(120);
		if (!(await page.locator(`${panel(id)} [data-ui="side-panel"]`).count())) return;
	}
	throw new Error('panel did not close');
}
const panelInfo = (id) =>
	page.evaluate((root) => {
		const el = document.querySelector(`${root} [data-ui="side-panel"]`);
		const r = el.getBoundingClientRect();
		return {
			mode: el.getAttribute('data-panel-mode'),
			tag: el.tagName.toLowerCase(),
			role: el.getAttribute('role'),
			modal: el.getAttribute('aria-modal'),
			width: Math.round(r.width),
			labelled: !!document.getElementById(el.getAttribute('aria-labelledby') || ''),
			now: el.querySelector('[role="separator"]')?.getAttribute('aria-valuenow') ?? null
		};
	}, panel(id));
await check(
	'P2 side panel: container 1000 is docked, 999 is an overlay; heading takes focus; Escape returns focus to the row',
	async () => {
		await openPanel('docked');
		const docked = await panelInfo('docked');
		expect(
			docked.mode === 'docked' && docked.tag === 'aside' && docked.labelled,
			JSON.stringify(docked)
		);
		const heading = await active();
		expect(heading.tag === 'h2', `focus ${JSON.stringify(heading)}`);
		await page.keyboard.press('Escape');
		await page.waitForSelector(`${panel('docked')} [data-ui="side-panel"]`, { state: 'detached' });
		const back = await active();
		expect(back.testid === 'panel-opener-docked', JSON.stringify(back));
		await openPanel('overlay');
		const overlay = await panelInfo('overlay');
		expect(overlay.mode === 'overlay', JSON.stringify(overlay));
		const pos = await page.evaluate(
			(root) => getComputedStyle(document.querySelector(`${root} [data-ui="side-panel"]`)).position,
			panel('overlay')
		);
		expect(pos === 'absolute', `overlay position ${pos}`);
		await page.keyboard.press('Escape');
		await page.waitForSelector(`${panel('overlay')} [data-ui="side-panel"]`, { state: 'detached' });
		return `${docked.mode} / ${overlay.mode}`;
	}
);
await check(
	'side panel: Alt+ArrowDown goes to the next record and keeps focus on the heading',
	async () => {
		await openPanel('docked');
		await page.keyboard.press('Alt+ArrowDown');
		await page.waitForFunction(
			(root) => document.querySelector(`${root} [data-record]`).dataset.record === '2',
			panel('docked')
		);
		await page.keyboard.press('Alt+ArrowUp');
		await page.waitForFunction(
			(root) => document.querySelector(`${root} [data-record]`).dataset.record === '1',
			panel('docked')
		);
		const prev = await page.evaluate((root) => {
			const b = document.querySelector(
				`${root} [data-ui="side-panel"] button[aria-label^="Previous record"]`
			);
			return { name: b.getAttribute('aria-label'), disabled: b.disabled };
		}, panel('docked'));
		expect(prev.disabled && prev.name.includes('this is the first record'), JSON.stringify(prev));
		await page.keyboard.press('Escape');
		await page.waitForSelector(`${panel('docked')} [data-ui="side-panel"]`, { state: 'detached' });
		return prev.name;
	}
);
await check(
	'side panel: separator keys resize (End = max, Home = 360, arrows 16, Enter = default)',
	async () => {
		await openPanel('docked');
		const sep = `${panel('docked')} [role="separator"]`;
		await focusEl(sep);
		const w = async () => Number(await page.getAttribute(sep, 'aria-valuenow'));
		await page.keyboard.press('End');
		const max = await w();
		await page.keyboard.press('Home');
		const min = await w();
		await page.keyboard.press('ArrowLeft');
		const wider = await w();
		await page.keyboard.press('Shift+ArrowLeft');
		const wider2 = await w();
		await page.keyboard.press('Enter');
		const reset = await w();
		const domWidth = (await panelInfo('docked')).width;
		expect(
			max === 600 &&
				min === 360 &&
				wider === 376 &&
				wider2 === 440 &&
				reset === 600 &&
				domWidth === 600,
			JSON.stringify({ max, min, wider, wider2, reset, domWidth })
		);
		await page.keyboard.press('Escape');
		await page.waitForSelector(`${panel('docked')} [data-ui="side-panel"]`, { state: 'detached' });
		return JSON.stringify({ max, min, wider, wider2, reset });
	}
);
await check('P3 side panel width: 600 in a 1280 window, 480 in a 1279 window', async () => {
	await load(1280);
	await openPanel('docked');
	const at1280 = (await panelInfo('docked')).width;
	await load(1279);
	await openPanel('docked');
	const at1279 = (await panelInfo('docked')).width;
	expect(at1280 === 600 && at1279 === 480, `${at1280} / ${at1279}`);
	await page.keyboard.press('Escape');
	await load(1440);
	return `${at1280} at 1280, ${at1279} at 1279`;
});
await check(
	'P2 side panel: a 767 viewport makes it a modal sheet with inert siblings; Escape returns focus',
	async () => {
		await load(767);
		await openPanel('docked');
		const info = await panelInfo('docked');
		expect(
			info.mode === 'sheet' && info.role === 'dialog' && info.modal === 'true',
			JSON.stringify(info)
		);
		const inert = await page.evaluate(
			(root) => document.querySelector(`${root} .list`).inert,
			panel('docked')
		);
		expect(inert, 'list is inert');
		const stayed = [];
		for (let i = 0; i < 12; i += 1) {
			await page.keyboard.press('Tab');
			stayed.push(
				await page.evaluate(
					(root) => !!document.activeElement?.closest(`${root} [data-ui="side-panel"]`),
					panel('docked')
				)
			);
		}
		expect(stayed.every(Boolean), `Tab left the sheet: ${stayed.join()}`);
		await closePanel('docked');
		const back = await active();
		expect(back.testid === 'panel-opener-docked', JSON.stringify(back));
		const released = await page.evaluate(
			(root) => !document.querySelector(`${root} .list`).inert,
			panel('docked')
		);
		expect(released, 'inert released');
		await load(1440);
		return 'sheet, dialog, inert siblings, Tab trapped, focus returned';
	}
);
await check(
	'side panel: hosted shows only the status block (no header, no separator)',
	async () => {
		const r = await page.evaluate(() => {
			const el = document.querySelector('[data-gallery="side-panel:hosted-failed"]');
			return {
				header: !!el.querySelector('header'),
				separator: !!el.querySelector('[role="separator"]'),
				alert: !!el.querySelector('[role="alert"]'),
				retry: el.textContent.includes('Retry')
			};
		});
		expect(!r.header && !r.separator && r.alert && r.retry, JSON.stringify(r));
	}
);

// ---------------------------------------------------------------- data table
const DT = '[data-gallery="data-table:default"]';
const rowControls = (n) => `${DT} tbody tr[data-row]:nth-child(${n})`;
await check(
	'G1 data table: one row in the Tab order; ArrowDown keeps the kind of control',
	async () => {
		const stops = await page.evaluate((root) => {
			return [...document.querySelectorAll(`${root} tbody tr[data-row]`)].map(
				(tr) => [...tr.querySelectorAll('input, a, button')].filter((c) => c.tabIndex === 0).length
			);
		}, DT);
		expect(stops[0] > 0 && stops[1] === 0 && stops[2] === 0, `tab stops per row ${stops}`);
		await tabTo(`${rowControls(1)} input[type="checkbox"]`);
		await page.keyboard.press('ArrowDown');
		let a = await page.evaluate(() => ({
			type: document.activeElement.getAttribute('type'),
			row: document.activeElement.closest('tr').dataset.recordRow
		}));
		expect(a.type === 'checkbox' && a.row === 'r2', JSON.stringify(a));
		await page.keyboard.press('Tab');
		a = await page.evaluate(() => ({
			link: document.activeElement.hasAttribute('data-record-title-link'),
			row: document.activeElement.closest('tr').dataset.recordRow
		}));
		expect(a.link && a.row === 'r2', `Tab goes to the title link: ${JSON.stringify(a)}`);
		await page.keyboard.press('ArrowDown');
		a = await page.evaluate(() => ({
			link: document.activeElement.hasAttribute('data-record-title-link'),
			row: document.activeElement.closest('tr').dataset.recordRow
		}));
		expect(a.link && a.row === 'r3', `ArrowDown on the title link: ${JSON.stringify(a)}`);
		const moved = await page.evaluate(
			(root) =>
				[...document.querySelectorAll(`${root} tbody tr[data-row]`)].map(
					(tr) =>
						[...tr.querySelectorAll('input, a, button')].filter((c) => c.tabIndex === 0).length
				),
			DT
		);
		expect(moved[2] > 0 && moved[0] === 0, `roving moved: ${moved}`);
		await page.keyboard.press('Enter');
		const opened = await page.getAttribute(`${DT} [data-opened]`, 'data-opened');
		expect(opened === 'r3', `opened ${opened}`);
		return `stops ${stops} -> ${moved}; Enter opened ${opened}`;
	}
);
await check('G2 data table: a focused row shows the 2px focus ring on its cells', async () => {
	await focusEl(`${rowControls(2)} a[data-record-title-link]`);
	await page.keyboard.press('ArrowUp');
	const focusColour = await tokenColour('--rf-focus');
	const shadow = await page.evaluate(
		(s) => getComputedStyle(document.querySelector(s)).boxShadow,
		`${rowControls(1)} td:nth-child(2)`
	);
	expect(shadow.includes(focusColour) && shadow.includes('2px'), `${shadow} vs ${focusColour}`);
	return shadow;
});
await check(
	'table selection by keyboard: Space selects with the leading bar, header is indeterminate, Shift+Space extends, Escape clears',
	async () => {
		await focusEl(`${rowControls(1)} input[type="checkbox"]`);
		await page.keyboard.press('Space');
		const bar = await page.evaluate(
			(s) => {
				const style = getComputedStyle(document.querySelector(s), '::before');
				return { width: style.width, colour: style.backgroundColor, content: style.content };
			},
			`${rowControls(1)} td:first-child`
		);
		const primary = await tokenColour('--rf-primary');
		expect(bar.width === '2px' && bar.colour === primary, `${JSON.stringify(bar)} vs ${primary}`);
		const indeterminate = await page.evaluate(
			(root) => document.querySelector(`${root} thead input[type="checkbox"]`).indeterminate,
			DT
		);
		expect(indeterminate, 'header checkbox is indeterminate');
		await page.keyboard.press('ArrowDown');
		await page.keyboard.press('ArrowDown');
		await page.keyboard.press('Shift+Space');
		const selected = await page.getAttribute(`${DT} [data-selected]`, 'data-selected');
		expect(selected === 'r1,r2,r3', `selected ${selected}`);
		const status = await page.evaluate(
			(root) => document.querySelector(`${root} [role="status"]`).textContent.trim(),
			DT
		);
		expect(status === '3 rows selected', `announcement ${JSON.stringify(status)}`);
		const headerChecked = await page.evaluate(
			(root) => document.querySelector(`${root} thead input[type="checkbox"]`).checked,
			DT
		);
		expect(headerChecked, 'header checked when all are selected');
		// The first Escape closes the panel that the earlier Enter opened, the second clears the selection.
		await page.keyboard.press('Escape');
		const openRow = await page.locator(`${DT} tr[aria-current]`).count();
		expect(openRow === 0, 'first Escape closed the panel');
		await page.keyboard.press('Escape');
		const cleared = await page.getAttribute(`${DT} [data-selected]`, 'data-selected');
		expect(cleared === '', `cleared ${JSON.stringify(cleared)}`);
		return `bar ${JSON.stringify(bar)}, ${status}`;
	}
);
await check(
	'table: clicking a row checkbox does not open the row; clicking the row does',
	async () => {
		await page.locator(`${rowControls(2)} input[type="checkbox"]`).click();
		let opened = await page.getAttribute(`${DT} [data-opened]`, 'data-opened');
		expect(opened !== 'r2', `checkbox opened the row: ${opened}`);
		await page.locator(`${rowControls(2)} td:nth-child(4)`).click();
		opened = await page.getAttribute(`${DT} [data-opened]`, 'data-opened');
		expect(opened === 'r2', `row click opened ${opened}`);
		const current = await page.getAttribute(`${rowControls(2)}`, 'aria-current');
		expect(current === 'true', `aria-current ${current}`);
		await page.locator(`${rowControls(2)} input[type="checkbox"]`).click();
		await page.keyboard.press('Escape');
	}
);
await check(
	'G3 data table: a sortable header cycles aria-sort ascending, descending, none',
	async () => {
		const th = `${DT} thead th:nth-child(2)`;
		const read = () => page.getAttribute(th, 'aria-sort');
		const seen = [await read()];
		for (let i = 0; i < 3; i += 1) {
			await page.locator(`${th} button`).click();
			seen.push(await read());
		}
		expect(seen.join() === 'none,ascending,descending,none', seen.join());
		const unsortable = await page.getAttribute(`${DT} thead th:last-child`, 'aria-sort');
		expect(unsortable === null, 'columns that cannot sort have no aria-sort');
		return seen.join(' -> ');
	}
);
await check(
	'table: title column is frozen, numbers are right aligned, long titles are cut',
	async () => {
		const r = await page.evaluate((root) => {
			const cell = document.querySelector(`${root} tbody tr:nth-child(2) td:nth-child(2)`);
			const amount = document.querySelector(`${root} tbody tr:nth-child(1) td:nth-child(4)`);
			return {
				position: getComputedStyle(cell).position,
				align: getComputedStyle(amount).textAlign,
				cut: cell.querySelector('.cell').scrollWidth > cell.querySelector('.cell').clientWidth,
				rowHeight: Math.round(cell.parentElement.getBoundingClientRect().height)
			};
		}, DT);
		expect(
			r.position === 'sticky' && r.align === 'end' && r.cut && r.rowHeight === 44,
			JSON.stringify(r)
		);
		return JSON.stringify(r);
	}
);
await check('table: compact density gives 32px rows and 28px header', async () => {
	const r = await page.evaluate(() => {
		const root = document.querySelector('[data-gallery="data-table:compact"]');
		return {
			row: Math.round(root.querySelector('tbody tr[data-row]').getBoundingClientRect().height),
			head: Math.round(root.querySelector('thead th').getBoundingClientRect().height)
		};
	});
	expect(r.row === 32 && r.head === 28, JSON.stringify(r));
	return JSON.stringify(r);
});
await check(
	'G1 data table: at 340 wide there is no table, rows are two lines and still navigable',
	async () => {
		const r = await page.evaluate(() => {
			const root = document.querySelector('[data-gallery="data-table:narrow"]');
			return {
				table: root.querySelector('table') !== null,
				rows: root.querySelectorAll('li[data-row]').length,
				height: Math.round(root.querySelector('li[data-row]').getBoundingClientRect().height)
			};
		});
		expect(!r.table && r.rows === 3 && r.height >= 64, JSON.stringify(r));
		await tabTo('[data-gallery="data-table:narrow"] li[data-row] input[type="checkbox"]');
		await page.keyboard.press('ArrowDown');
		const row = await page.evaluate(() => document.activeElement.closest('li').dataset.recordRow);
		expect(row === 'r2', `ArrowDown reached ${row}`);
		return JSON.stringify(r);
	}
);
await check(
	'table states: loading is aria-busy with skeleton rows; empty, failed, no results show the block with the header',
	async () => {
		const r = await page.evaluate(() => {
			const read = (name) => {
				const root = document.querySelector(`[data-gallery="data-table:${name}"]`);
				return {
					busy: root.querySelector('table')?.getAttribute('aria-busy'),
					skeleton: root.querySelectorAll('.skeleton-row').length,
					state: root.querySelector('[data-table-state]')?.getAttribute('data-table-state') ?? null,
					role: root.querySelector('[data-table-state]')?.getAttribute('role') ?? null,
					header: root.querySelectorAll('thead th').length > 0,
					rows: root.querySelectorAll('tbody tr[data-row]').length
				};
			};
			return {
				loading: read('loading'),
				empty: read('empty'),
				failed: read('failed'),
				noResults: read('no-results'),
				offline: read('offline')
			};
		});
		expect(
			r.loading.busy === 'true' && r.loading.skeleton === 5 && r.loading.rows === 0,
			JSON.stringify(r.loading)
		);
		expect(
			r.empty.state === 'empty' && r.empty.header && r.empty.rows === 0,
			JSON.stringify(r.empty)
		);
		expect(r.failed.state === 'failed' && r.failed.role === 'alert', JSON.stringify(r.failed));
		expect(r.noResults.state === 'noResults' && r.offline.state === 'offline', 'states');
		return JSON.stringify(r);
	}
);

// ---------------------------------------------------------------- page header
await check(
	'T1 page header: standard is 88 high, compact 56; the title is focusable by script but not by Tab',
	async () => {
		const r = await page.evaluate(() => {
			const h = (name) =>
				document
					.querySelector(`[data-gallery="page-header:${name}"] header`)
					.getBoundingClientRect().height;
			const title = document.querySelector('[data-gallery="page-header:compact"] h1');
			title.focus();
			return {
				standard: h('standard'),
				compact: h('compact'),
				focusable: document.activeElement === title,
				tabindex: title.tabIndex
			};
		});
		expect(
			r.standard === 88 && r.compact === 56 && r.focusable && r.tabindex === -1,
			JSON.stringify(r)
		);
		await tabTo(
			'[data-gallery="page-header:compact"] button, [data-gallery="page-header:compact"] a'
		);
		const stop = await active();
		expect(stop.tag !== 'h1', 'the title is not a Tab stop');
		return JSON.stringify(r);
	}
);
await check('page header: at 600 the secondary actions are only in the overflow menu', async () => {
	const root = '[data-gallery="page-header:compact-600"]';
	const buttons = await page.locator(`${root} .action button`).allInnerTexts();
	expect(buttons.map((s) => s.trim()).join() === 'New record', `buttons ${buttons.join()}`);
	await page.locator(`${root} button[aria-haspopup="menu"]`).click();
	await page.waitForSelector('[role="menu"]');
	const items = (await page.locator('[role="menu"] [role="menuitem"]').allInnerTexts()).map((s) =>
		s.trim()
	);
	await page.keyboard.press('Escape');
	await page.waitForSelector('[role="menu"]', { state: 'detached' });
	expect(items.join() === 'Export,Import', `menu ${items.join()}`);
	return `buttons ${buttons.join()}, menu ${items.join()}`;
});
await check(
	'page header: at 390 the primary is an icon button named by its label; the context is hidden but described',
	async () => {
		const r = await page.evaluate(() => {
			const root = document.querySelector('[data-gallery="page-header:compact-390"]');
			const primary = root.querySelector('.action button[aria-label="New record"]');
			const context = root.querySelector('.context');
			const rect = context.getBoundingClientRect();
			const h1 = root.querySelector('h1');
			return {
				named: primary !== null && primary.textContent.trim() === '',
				size: primary ? Math.round(primary.getBoundingClientRect().height) : 0,
				hidden: rect.width <= 1 && rect.height <= 1,
				described: !!document.getElementById(h1.getAttribute('aria-describedby') || '')
			};
		});
		expect(r.named && r.size === 32 && r.hidden && r.described, JSON.stringify(r));
		return JSON.stringify(r);
	}
);
await check(
	'page header: a disabled primary shows its reason; loading shows skeletons without actions',
	async () => {
		const r = await page.evaluate(() => {
			const disabled = document.querySelector('[data-gallery="page-header:disabled-primary"]');
			const loading = document.querySelector('[data-gallery="page-header:loading"]');
			const button = disabled.querySelector('.action button');
			return {
				disabled: button.disabled,
				reason:
					document
						.getElementById(button.getAttribute('aria-describedby') || '')
						?.textContent.trim() ?? null,
				skeletons: loading.querySelectorAll('.skeleton').length,
				actions: loading.querySelectorAll('button').length
			};
		});
		expect(r.disabled && r.reason && r.skeletons === 2 && r.actions === 0, JSON.stringify(r));
	}
);
await check('page header: the compact title and context sit on one line', async () => {
	const r = await page.evaluate(() => {
		const root = document.querySelector('[data-gallery="page-header:compact"]');
		const h1 = root.querySelector('h1').getBoundingClientRect();
		const ctx = root.querySelector('.context nav').getBoundingClientRect();
		return {
			sameLine: Math.abs(h1.top + h1.height / 2 - (ctx.top + ctx.height / 2)) < 10,
			ctxRight: ctx.left >= h1.right
		};
	});
	expect(r.sameLine && r.ctxRight, JSON.stringify(r));
});

// ---------------------------------------------------------------- reserved keys and theme
await check('dark container: toast, tab strip and table keep the dark tokens', async () => {
	await page.locator('[data-testid="gallery-dark"]').check({ force: true });
	const r = await page.evaluate(() => {
		const bg = (s) => getComputedStyle(document.querySelector(s)).backgroundColor;
		return {
			strip: bg('[data-gallery="task-tab-strip:default"] .ui-tab-strip'),
			table: bg('[data-gallery="data-table:default"] .ui-data-table'),
			header: bg('[data-gallery="page-header:standard"] header')
		};
	});
	await page.locator('[data-testid="gallery-dark"]').uncheck({ force: true });
	expect(
		r.strip !== 'rgb(245, 244, 241)' && r.table === r.header && r.table !== 'rgb(252, 251, 249)',
		JSON.stringify(r)
	);
	return JSON.stringify(r);
});

// Phase 2 closing checks: composed handlers, sheet, menu roles, type-ahead, links, toasts.
await check(
	'checkbox in the table: a caller onchange composes with the checkbox (click, Shift-click, header, plural)',
	async () => {
		await load(1440);
		const boxes = `${DT} tbody input[type="checkbox"]`;
		const selected = () => page.getAttribute(`${DT} [data-selected]`, 'data-selected');
		const status = () =>
			page.evaluate(
				(root) => document.querySelector(`${root} [role="status"]`).textContent.trim(),
				DT
			);
		await page.locator(boxes).nth(0).click({ force: true });
		const one = await selected();
		const oneStatus = await status();
		expect(one === 'r1' && oneStatus === '1 row selected', `${one} / ${oneStatus}`);
		await page
			.locator(boxes)
			.nth(2)
			.click({ force: true, modifiers: ['Shift'] });
		const range = await selected();
		expect(range === 'r1,r2,r3', `range ${range}`);
		await page.locator(`${DT} thead input[type="checkbox"]`).click({ force: true });
		const afterHeader = await selected();
		expect(afterHeader !== range, `header toggled: ${afterHeader}`);
		return `${oneStatus}; ${range}; header -> ${JSON.stringify(afterHeader)}`;
	}
);
await check(
	'date picker at 390: the popover is a bottom sheet with a grabber, a title and Close',
	async () => {
		await load(390);
		const root = '[data-gallery="date:default"]';
		await page.locator(`${root} button[aria-haspopup="dialog"]`).click();
		await page.waitForSelector('[data-ui-layer="popover"]');
		const r = await page.evaluate(() => {
			const el = document.querySelector('[data-ui-layer="popover"]');
			const rect = el.getBoundingClientRect();
			const style = getComputedStyle(el);
			const grabber = el.querySelector('.grabber');
			return {
				sheet: el.className.includes('sheet'),
				title: el.querySelector('h2')?.textContent.trim(),
				grabber: grabber
					? `${getComputedStyle(grabber).width} x ${getComputedStyle(grabber).height}`
					: null,
				close: [...el.querySelectorAll('button')].some((b) => b.textContent.trim() === 'Close'),
				left: Math.round(rect.left),
				width: Math.round(rect.width),
				bottom: Math.round(window.innerHeight - rect.bottom),
				topRadius: style.borderTopLeftRadius,
				label: document.querySelector(`[data-gallery="date:default"] label`)?.textContent.trim()
			};
		});
		expect(
			r.sheet &&
				r.title &&
				r.title.startsWith(r.label.split(/\s/)[0]) &&
				r.grabber === '36px x 5px' &&
				r.close,
			JSON.stringify(r)
		);
		expect(
			r.left === 0 && r.width === 390 && r.bottom === 0 && r.topRadius === '12px',
			JSON.stringify(r)
		);
		await page.keyboard.press('Escape');
		await page.waitForSelector('[data-ui-layer="popover"]', { state: 'detached' });
		await load(1440);
		return JSON.stringify(r);
	}
);
await check('date picker at 1440 stays a floating popover without sheet chrome', async () => {
	const root = '[data-gallery="date:default"]';
	await page.locator(`${root} button[aria-haspopup="dialog"]`).click();
	await page.waitForSelector('[data-ui-layer="popover"]');
	const r = await page.evaluate(() => {
		const el = document.querySelector('[data-ui-layer="popover"]');
		return { sheet: el.className.includes('sheet'), grabber: !!el.querySelector('.grabber') };
	});
	expect(!r.sheet && !r.grabber, JSON.stringify(r));
	await page.keyboard.press('Escape');
	await page.waitForSelector('[data-ui-layer="popover"]', { state: 'detached' });
});
await check(
	'tab strip overflow menu: radio items with exactly one aria-checked, set from the active tab',
	async () => {
		const OV = '[data-gallery="task-tab-strip:overflow"]';
		await page.locator(`${OV} [data-testid="task-tab-overflow"]`).click();
		await page.waitForSelector('[data-testid="task-tab-overflow-menu"]');
		const r = await page.evaluate(() => {
			const items = [...document.querySelectorAll('[data-testid="task-tab-overflow-item"]')];
			return {
				roles: [...new Set(items.map((i) => i.getAttribute('role')))],
				checked: items.filter((i) => i.getAttribute('aria-checked') === 'true').length,
				unchecked: items.filter((i) => i.getAttribute('aria-checked') === 'false').length,
				total: items.length,
				menuRole: document
					.querySelector('[data-testid="task-tab-overflow-menu"]')
					.getAttribute('role')
			};
		});
		expect(
			r.roles.length === 1 &&
				r.roles[0] === 'menuitemradio' &&
				r.checked === 1 &&
				r.unchecked === r.total - 1 &&
				r.menuRole === 'menu',
			JSON.stringify(r)
		);
		await page.keyboard.press('Escape');
		await page.waitForSelector('[role="menu"]', { state: 'detached' });
		return JSON.stringify(r);
	}
);
await check('menu type-ahead: a letter focuses the next item that starts with it', async () => {
	const trigger = '[data-gallery="menu:trigger"] button';
	await tabTo(trigger);
	await page.keyboard.press('Enter');
	await page.waitForSelector('[role="menu"][data-popover="demo"]');
	await page.waitForFunction(() => document.activeElement?.getAttribute('role') === 'menuitem');
	await page.keyboard.press('d');
	const first = await active();
	await page.keyboard.press('z');
	const none = await active();
	await wait(700);
	await page.keyboard.press('s');
	const second = await active();
	await page.keyboard.press('Escape');
	await page.waitForSelector('[role="menu"]', { state: 'detached' });
	expect(
		first.text.startsWith('Delete') && none.text.startsWith('Delete'),
		JSON.stringify({ first, none })
	);
	expect(second.text.startsWith('Sample'), JSON.stringify(second));
	return `d -> ${first.text}; z keeps; after the pause s -> ${second.text}`;
});
await check(
	'collapsed breadcrumb items are real links: Cmd-click opens a new tab, a plain click navigates',
	async () => {
		await load(1440);
		const COLLAPSED = '[data-gallery="context-line:collapsed"]';
		const open = async () => {
			await page.locator(`${COLLAPSED} nav button[aria-haspopup="menu"]`).click();
			await page.waitForSelector('[data-popover="context-line-menu"]');
		};
		await open();
		const tags = await page.evaluate(() =>
			[...document.querySelectorAll('[data-popover="context-line-menu"] [role="menuitem"]')].map(
				(el) => ({
					tag: el.tagName.toLowerCase(),
					href: el.getAttribute('href')
				})
			)
		);
		expect(tags.length > 0 && tags.every((t) => t.tag === 'a' && t.href), JSON.stringify(tags));
		const before = page.url();
		const opened = context.waitForEvent('page', { timeout: 5000 });
		await page
			.locator('[data-popover="context-line-menu"] [role="menuitem"]')
			.first()
			.click({ modifiers: ['Meta'] });
		const tab = await opened;
		await tab.close();
		expect(page.url() === before, `current tab navigated to ${page.url()}`);
		await page.keyboard.press('Escape');
		await page.waitForSelector('[data-popover="context-line-menu"]', { state: 'detached' });
		await open();
		await page.locator('[data-popover="context-line-menu"] [role="menuitem"]').first().click();
		await page.waitForSelector('[data-popover="context-line-menu"]', { state: 'detached' });
		expect(page.url() !== before && page.url().includes('#'), `plain click went to ${page.url()}`);
		return JSON.stringify(tags);
	}
);
await check('toasts wait while a dialog is open and appear when it closes', async () => {
	await load(1440);
	await tabTo('[data-testid="dialog-opener"]');
	await page.keyboard.press('Enter');
	await page.waitForSelector('[data-testid="dialog-demo"]');
	await page.evaluate(() =>
		document.querySelector('[data-gallery="toast:neutral"] button').click()
	);
	await wait(400);
	const during = await page.locator('[data-toast]').count();
	expect(during === 0, `${during} toasts shown behind the dialog`);
	await page.keyboard.press('Escape');
	await page.waitForSelector('[data-testid="dialog-demo"]', { state: 'detached' });
	await page.waitForSelector('[data-toast]');
	const after = await page.locator('[data-toast]').count();
	expect(after === 1, `${after} toasts after close`);
	await page.evaluate(() =>
		document.querySelector('[data-gallery="toast:neutral"] button').click()
	);
	await wait(200);
	return 'held while open (0), shown after close (1)';
});

console.log(results.join('\n'));
console.log(`\n${results.length - failures} passed, ${failures} failed`);
await browser.close();
process.exit(failures > 0 ? 1 : 0);
