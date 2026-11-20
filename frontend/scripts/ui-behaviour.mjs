#!/usr/bin/env node
// Browser behaviour checks for the `lib/ui` primitives, run against the internal test page
// `/admin/ui` in headless Chromium with real key presses.
//
//   node scripts/ui-behaviour.mjs --base-url http://127.0.0.1:18201 --api http://127.0.0.1:18083
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
	viewport: { width: 1280, height: 900 },
	locale: 'en-US',
	reducedMotion: 'reduce'
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
await page.goto(`${baseUrl}/admin/ui`);
await page.waitForSelector('[data-gallery-section="button"]');

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
					inGallery: el.closest('[data-gallery]')?.getAttribute('data-gallery') ?? null
				}
			: null;
	});

async function tabTo(selector, max = 80) {
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
const outline = (selector) =>
	page.evaluate((s) => {
		const style = getComputedStyle(document.querySelector(s));
		return { width: style.outlineWidth, offset: style.outlineOffset, style: style.outlineStyle };
	}, selector);

// Focus ring (A: Tab to primary and secondary buttons).
await check('focus ring: primary button is 2px with offset 2px', async () => {
	await tabTo('[data-gallery="button:primary"] button');
	const o = await outline('[data-gallery="button:primary"] button');
	expect(o.width === '2px' && o.offset === '2px' && o.style === 'solid', JSON.stringify(o));
	return JSON.stringify(o);
});
await check('focus ring: secondary button is 2px with offset -1px', async () => {
	await tabTo('[data-gallery="button:secondary"] button');
	const o = await outline('[data-gallery="button:secondary"] button');
	expect(o.width === '2px' && o.offset === '-1px', JSON.stringify(o));
	return JSON.stringify(o);
});
await check('focus ring: danger button 2px / 2px, text field box -1px', async () => {
	await tabTo('[data-gallery="button:danger"] button');
	const d = await outline('[data-gallery="button:danger"] button');
	expect(d.width === '2px' && d.offset === '2px', JSON.stringify(d));
	await tabTo('[data-gallery="text-field:default"] input');
	const f = await page.evaluate(() => {
		const style = getComputedStyle(
			document.querySelector('[data-gallery="text-field:default"] .box')
		);
		return { width: style.outlineWidth, offset: style.outlineOffset };
	});
	expect(f.width === '2px' && f.offset === '-1px', JSON.stringify(f));
	return `danger ${JSON.stringify(d)}, field ${JSON.stringify(f)}`;
});
await check('focus ring colour is the focus token, not the base-layer ring colour', async () => {
	await tabTo('[data-gallery="button:secondary"] button');
	const { ring, token } = await page.evaluate(() => {
		const el = document.querySelector('[data-gallery="button:secondary"] button');
		const probe = document.createElement('i');
		probe.style.color = 'var(--rf-focus)';
		document.body.appendChild(probe);
		const token = getComputedStyle(probe).color;
		probe.remove();
		return { ring: getComputedStyle(el).outlineColor, token };
	});
	expect(ring === token, `ring ${ring} token ${token}`);
	return ring;
});

// Button: disabled with reason, loading.
await check('button: disabled with a visible reason, described by it', async () => {
	const r = await page.evaluate(() => {
		const root = document.querySelector('[data-gallery="button:disabled-reason"]');
		const button = root.querySelector('button');
		const reason = document.getElementById(button.getAttribute('aria-describedby') || '');
		return { disabled: button.disabled, reason: reason?.textContent?.trim() ?? null };
	});
	expect(r.disabled && r.reason, JSON.stringify(r));
});
await check('button: loading keeps its width and is aria-busy', async () => {
	const r = await page.evaluate(() => {
		const button = document.querySelector('[data-gallery="button:loading"] button');
		return { busy: button.getAttribute('aria-busy'), width: button.getBoundingClientRect().width };
	});
	expect(r.busy === 'true' && r.width > 40, JSON.stringify(r));
});

// Switch and Checkbox by keyboard.
await check('switch toggles with Space', async () => {
	await tabTo('[data-gallery="switch:off"] input');
	const before = await page.locator('[data-gallery="switch:off"] input').isChecked();
	await page.keyboard.press('Space');
	const after = await page.locator('[data-gallery="switch:off"] input').isChecked();
	expect(before === false && after === true, `${before} -> ${after}`);
	await page.keyboard.press('Space');
	expect(!(await page.locator('[data-gallery="switch:off"] input').isChecked()), 'second Space');
});
await check('checkbox toggles with Space; indeterminate clears on toggle', async () => {
	await tabTo('[data-gallery="checkbox:off"] input');
	await page.keyboard.press('Space');
	expect(await page.locator('[data-gallery="checkbox:off"] input').isChecked(), 'checked');
	await tabTo('[data-gallery="checkbox:indeterminate"] input');
	await page.keyboard.press('Space');
	const r = await page.evaluate(() => {
		const input = document.querySelector('[data-gallery="checkbox:indeterminate"] input');
		return { checked: input.checked, indeterminate: input.indeterminate };
	});
	expect(r.checked && !r.indeterminate, JSON.stringify(r));
});

// Radio roving.
await check(
	'radio group: one tab stop, arrows move focus and selection, disabled is skipped',
	async () => {
		const stops = await page.evaluate(
			() =>
				[...document.querySelectorAll('[data-gallery="radio:group"] input')].filter(
					(el) => el.tabIndex === 0
				).length
		);
		expect(stops === 1, `tab stops ${stops}`);
		await tabTo('[data-gallery="radio:group"] input');
		expect((await active()).inGallery === 'radio:group', 'focus entered the group');
		await page.keyboard.press('ArrowDown');
		let state = await page.evaluate(() => ({
			focus: document.activeElement?.value,
			checked: document.querySelector('[data-gallery="radio:group"] input:checked')?.value
		}));
		expect(
			state.focus === 'd' && state.checked === 'd',
			`after Down from b: ${JSON.stringify(state)}`
		);
		await page.keyboard.press('ArrowUp');
		state = await page.evaluate(() => document.activeElement?.value);
		expect(state === 'b', `after Up: ${state}`);
		await page.keyboard.press('Home');
		state = await page.evaluate(() => document.activeElement?.value);
		expect(state === 'a', `Home: ${state}`);
	}
);

// Dialog.
await check(
	'dialog: focus on the first field, Tab cycles inside, Escape closes and keeps the draft',
	async () => {
		await tabTo('[data-testid="dialog-opener"]');
		await page.keyboard.press('Enter');
		await page.waitForSelector('[data-testid="dialog-demo"]');
		await page.waitForFunction(() => document.activeElement?.closest('[role="dialog"]'));
		const first = await active();
		expect(first.tag === 'input' && first.inDialog, `first focus ${JSON.stringify(first)}`);
		await page.keyboard.type('draft text');
		const seen = new Set();
		for (let i = 0; i < 8; i += 1) {
			await page.keyboard.press('Tab');
			const a = await active();
			expect(a.inDialog, `Tab ${i + 1} left the dialog: ${JSON.stringify(a)}`);
			seen.add(`${a.tag}:${a.text}`);
		}
		expect(seen.size >= 3, `cycled through ${seen.size} distinct stops`);
		await page.keyboard.press('Shift+Tab');
		expect((await active()).inDialog, 'Shift+Tab stays inside');
		await page.mouse.click(4, 4);
		expect(
			await page.locator('[data-testid="dialog-demo"]').isVisible(),
			'backdrop click closed it'
		);
		await page.keyboard.press('Escape');
		await page.waitForSelector('[data-testid="dialog-demo"]', { state: 'detached' });
		const back = await active();
		expect(back.testid === 'dialog-opener', `focus returned to ${JSON.stringify(back)}`);
		await page.keyboard.press('Enter');
		await page.waitForSelector('[data-testid="dialog-demo"]');
		const value = await page.locator('[data-testid="dialog-demo"] input').inputValue();
		expect(value === 'draft text', `draft ${JSON.stringify(value)}`);
		await page.keyboard.press('Escape');
		await page.waitForSelector('[data-testid="dialog-demo"]', { state: 'detached' });
		return `first focus ${first.tag}, ${seen.size} stops, draft kept`;
	}
);
await check(
	'destructive dialog: focus starts on Cancel, Escape returns focus to the opener',
	async () => {
		await tabTo('[data-testid="destructive-opener"]');
		await page.keyboard.press('Enter');
		await page.waitForSelector('[data-testid="destructive-demo"]');
		await page.waitForFunction(() => document.activeElement?.closest('[role="dialog"]'));
		const first = await active();
		expect(first.tag === 'button' && first.text === 'Cancel', JSON.stringify(first));
		await page.keyboard.press('Escape');
		await page.waitForSelector('[data-testid="destructive-demo"]', { state: 'detached' });
		const back = await active();
		expect(back.testid === 'destructive-opener', JSON.stringify(back));
	}
);

// Menu.
await check(
	'menu: opens on the first item, arrows rove, disabled item stays reachable by rule, Escape returns',
	async () => {
		await tabTo('[data-gallery="menu:trigger"] button');
		await page.keyboard.press('Enter');
		await page.waitForSelector('[role="menu"]');
		await page.waitForFunction(() => document.activeElement?.getAttribute('role') === 'menuitem');
		const names = [];
		const read = () =>
			page.evaluate(() => document.activeElement?.textContent?.trim().slice(0, 20));
		names.push(await read());
		await page.keyboard.press('ArrowDown');
		names.push(await read());
		await page.keyboard.press('End');
		const last = await read();
		await page.keyboard.press('Home');
		const home = await read();
		expect(
			names[0] === home && names[1] !== names[0] && last.includes('Delete'),
			JSON.stringify({ names, last, home })
		);
		await page.keyboard.press('Escape');
		await page.waitForSelector('[role="menu"]', { state: 'detached' });
		const back = await active();
		expect(back.inGallery === 'menu:trigger', `focus returned to ${JSON.stringify(back)}`);
	}
);
await check('menu: Tab leaves and closes it', async () => {
	await page.keyboard.press('Enter');
	await page.waitForSelector('[role="menu"]');
	await page.waitForFunction(() => document.activeElement?.getAttribute('role') === 'menuitem');
	await page.keyboard.press('Tab');
	await page.waitForSelector('[role="menu"]', { state: 'detached' });
});

// Popover.
await check('popover: focus moves in, Escape closes and returns focus to the trigger', async () => {
	await tabTo('[data-gallery="popover:trigger"] button');
	await page.keyboard.press('Enter');
	await page.waitForSelector('[data-popover="demo"]');
	await page.waitForFunction(() => document.activeElement?.closest('[data-popover="demo"]'));
	await page.keyboard.press('Escape');
	await page.waitForSelector('[data-popover="demo"]', { state: 'detached' });
	const back = await active();
	expect(back.inGallery === 'popover:trigger', JSON.stringify(back));
});

// Tooltip.
await check('tooltip: shows at once on keyboard focus, Escape hides it', async () => {
	await tabTo('[data-gallery="icon-button:default"] button');
	await page.waitForSelector('[role="tooltip"]');
	const text = await page.locator('[role="tooltip"]').innerText();
	expect(text.includes('More actions'), text);
	await page.keyboard.press('Escape');
	await page.waitForSelector('[role="tooltip"]', { state: 'detached' });
});
await check('tooltip: not on pointer focus, shown after the hover delay', async () => {
	const box = await page.locator('[data-gallery="icon-button:pressed"] button').boundingBox();
	await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
	await page.waitForTimeout(250);
	expect(!(await page.locator('[role="tooltip"]').count()), 'visible before 500 ms');
	await page.waitForTimeout(450);
	expect(await page.locator('[role="tooltip"]').count(), 'not visible after 700 ms');
	await page.mouse.move(2, 2);
	await page.waitForSelector('[role="tooltip"]', { state: 'detached' });
});
await check(
	'tooltip: describes=true sets aria-describedby, plain tooltip is aria-hidden',
	async () => {
		await tabTo('[data-gallery="tooltip:describes"]');
		const r = await page.evaluate(() => {
			const anchor = document.querySelector('[data-gallery="tooltip:describes"]');
			const tip = document.querySelector('[role="tooltip"]');
			return {
				describedby: anchor.getAttribute('aria-describedby'),
				id: tip?.id,
				hidden: tip?.getAttribute('aria-hidden')
			};
		});
		expect(r.describedby && r.describedby === r.id && r.hidden === null, JSON.stringify(r));
		await page.keyboard.press('Escape');
	}
);

// Theme: nested dark tokens resolve (F1).
await check('dark container: derived hover token resolves to the dark value', async () => {
	const probe = (name) =>
		page.evaluate((n) => {
			const el = document.querySelector('.page');
			return getComputedStyle(el).getPropertyValue(n).trim();
		}, name);
	const light = await probe('--rf-hover');
	await page.locator('[data-testid="gallery-dark"]').check({ force: true });
	const dark = await probe('--rf-hover');
	expect(
		light.toLowerCase() === '#efefef' && dark.toLowerCase() === '#212121',
		`${light} / ${dark}`
	);
	return `${light} -> ${dark}`;
});
await check('dark container: dialog keeps the dark theme', async () => {
	await tabTo('[data-testid="dialog-opener"]');
	await page.keyboard.press('Enter');
	await page.waitForSelector('[data-testid="dialog-demo"]');
	const bg = await page.evaluate(
		() => getComputedStyle(document.querySelector('[data-testid="dialog-demo"]')).backgroundColor
	);
	await page.keyboard.press('Escape');
	expect(bg === 'rgb(30, 30, 30)', bg);
	return bg;
});

console.log(results.join('\n'));
console.log(`\n${results.length - failures} passed, ${failures} failed`);
await browser.close();
process.exit(failures > 0 ? 1 : 0);
