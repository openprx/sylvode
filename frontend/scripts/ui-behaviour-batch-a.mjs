#!/usr/bin/env node
// Browser behaviour checks for the batch A primitives (Select, MultiSelect, Segmented, DatePicker,
// DateRangePicker, form validation, Pagination), run against `/admin/ui` in headless Chromium
// with real key presses. Same style and set-up as `ui-behaviour.mjs`.
//
//   UI_SHOTS_ENV_FILE=<env file> node scripts/ui-behaviour-batch-a.mjs --base-url http://127.0.0.1:18241
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

/** A signed-in page on the gallery with the given browser locale and app language. */
async function openGallery({ browserLocale = 'en-US', appLanguage = 'en', width = 1280 } = {}) {
	const context = await browser.newContext({
		viewport: { width, height: 900 },
		locale: browserLocale,
		reducedMotion: 'reduce'
	});
	await context.addInitScript(
		({ tokens, user, language }) => {
			try {
				localStorage.setItem('locale', language);
				localStorage.setItem('theme', 'light');
				localStorage.setItem('auth_token', tokens.access_token);
				localStorage.setItem('refresh_token', tokens.refresh_token);
				localStorage.setItem('auth_user', JSON.stringify(user));
			} catch {
				// storage unavailable
			}
		},
		{ tokens: session.tokens, user: session.user, language: appLanguage }
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
	await page.waitForSelector('[data-gallery-section="date"]');
	return { context, page };
}

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

const main = await openGallery();
const page = main.page;

const active = () =>
	page.evaluate(() => {
		const el = document.activeElement;
		return el
			? {
					tag: el.tagName.toLowerCase(),
					role: el.getAttribute('role'),
					date: el.getAttribute('data-date'),
					text: (el.textContent || '').trim().slice(0, 40),
					value: el.value ?? null,
					label: el.getAttribute('aria-label'),
					inGallery: el.closest('[data-gallery]')?.getAttribute('data-gallery') ?? null,
					inPopover: !!el.closest('[data-ui-layer="popover"]')
				}
			: null;
	});

async function tabTo(selector, max = 120) {
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
const attr = (selector, name) =>
	page.evaluate(([s, n]) => document.querySelector(s)?.getAttribute(n) ?? null, [selector, name]);
const count = (selector) => page.evaluate((s) => document.querySelectorAll(s).length, selector);
const activeDate = async () => (await active()).date;
const popoverOpen = () => count('[data-ui-layer="popover"]').then((n) => n > 0);
async function settle() {
	await page.waitForTimeout(60);
}

// ---- Date picker ---------------------------------------------------------------------------

await check(
	'date: ArrowDown opens the grid on the selected day; grid keys move the focus',
	async () => {
		await tabTo('[data-gallery="date:default"] input');
		await page.keyboard.press('ArrowDown');
		await page.waitForSelector('td[role="gridcell"][tabindex="0"]');
		await page.waitForFunction(() => document.activeElement?.matches('td[role="gridcell"]'));
		const seen = [];
		seen.push(await activeDate());
		expect(seen[0] === '2026-11-24', `opened on ${seen[0]}`);
		await page.keyboard.press('ArrowRight');
		seen.push(await activeDate());
		expect(seen[1] === '2026-11-25', `ArrowRight ${seen[1]}`);
		await page.keyboard.press('ArrowDown');
		seen.push(await activeDate());
		expect(seen[2] === '2026-12-02', `ArrowDown ${seen[2]} (next week, view moves to December)`);
		await page.keyboard.press('ArrowUp');
		await page.keyboard.press('PageDown');
		seen.push(await activeDate());
		expect(seen[3] === '2026-12-25', `PageDown ${seen[3]}`);
		await page.keyboard.press('Shift+PageUp');
		seen.push(await activeDate());
		expect(seen[4] === '2025-12-25', `Shift+PageUp ${seen[4]}`);
		await page.keyboard.press('Home');
		seen.push(await activeDate());
		expect(seen[5] === '2025-12-22', `Home ${seen[5]} (Monday)`);
		await page.keyboard.press('End');
		seen.push(await activeDate());
		expect(seen[6] === '2025-12-28', `End ${seen[6]} (Sunday)`);
		return seen.join(' ');
	}
);
await check('date: PageDown clamps to the last day of a shorter month', async () => {
	// From 2025-12-28: PageDown to 2026-01-28, again to 2026-02-28, never an invalid date.
	await page.keyboard.press('PageDown');
	await page.keyboard.press('ArrowRight');
	await page.keyboard.press('ArrowRight');
	await page.keyboard.press('ArrowRight');
	const d = await activeDate();
	await page.keyboard.press('PageDown');
	const e = await activeDate();
	expect(d === '2026-01-31' && e === '2026-02-28', `${d} then ${e}`);
	return `${d} -> ${e}`;
});
await check('date: Escape closes, returns focus to the field and keeps the value', async () => {
	await page.keyboard.press('Escape');
	await settle();
	const a = await active();
	expect(a.tag === 'input' && a.inGallery === 'date:default', JSON.stringify(a));
	expect(a.value === '2026-11-24', `value ${a.value}`);
	expect(!(await popoverOpen()), 'popover still open');
});
await check('date: Enter on a day selects it, closes and returns focus to the field', async () => {
	await page.keyboard.press('ArrowDown');
	await page.waitForFunction(() => document.activeElement?.matches('td[role="gridcell"]'));
	await page.keyboard.press('ArrowLeft');
	await page.keyboard.press('Enter');
	await settle();
	const a = await active();
	expect(a.tag === 'input' && a.value === '2026-11-23', JSON.stringify(a));
	expect(!(await popoverOpen()), 'popover still open');
});
await check('date: the grid has 6 rows, one tab stop, and the popover traps Tab', async () => {
	await page.keyboard.press('ArrowDown');
	await page.waitForFunction(() => document.activeElement?.matches('td[role="gridcell"]'));
	const rows = await count('[data-ui="calendar"] tbody tr');
	const stops = await count('[data-ui="calendar"] td[tabindex="0"]');
	expect(rows === 6 && stops === 1, `rows ${rows}, stops ${stops}`);
	const trail = [];
	for (let i = 0; i < 7; i += 1) {
		await page.keyboard.press('Tab');
		const a = await active();
		expect(a.inPopover, `Tab ${i + 1} left the popover: ${JSON.stringify(a)}`);
		trail.push(a.text || a.role || a.tag);
	}
	await page.keyboard.press('Escape');
	await settle();
	return trail.join(' | ');
});
await check('date: aria-current marks exactly one cell; disabled days ignore Enter', async () => {
	await tabTo('[data-gallery="date:empty"] input');
	await page.keyboard.press('ArrowDown');
	await page.waitForFunction(() => document.activeElement?.matches('td[role="gridcell"]'));
	const current = await count('td[aria-current="date"]');
	expect(current === 1, `aria-current cells ${current}`);
	await page.keyboard.press('Escape');
	await settle();
	await tabTo('[data-gallery="date:bounded"] input');
	await page.keyboard.press('ArrowDown');
	await page.waitForFunction(() => document.activeElement?.matches('td[role="gridcell"]'));
	await page.keyboard.press('PageDown');
	const focused = await page.evaluate(() => {
		const el = document.activeElement;
		return { date: el.getAttribute('data-date'), disabled: el.getAttribute('aria-disabled') };
	});
	expect(focused.disabled === 'true', `focused ${JSON.stringify(focused)}`);
	await page.keyboard.press('Enter');
	await settle();
	expect(await popoverOpen(), 'Enter on a disabled day closed the popover');
	const value = await page.locator('[data-gallery="date:bounded"] input').inputValue();
	expect(value === '', `value ${JSON.stringify(value)}`);
	await page.keyboard.press('Escape');
	await settle();
	return focused.date;
});
await check(
	'date: typed values normalise on blur; an ambiguous one is rejected with a hint',
	async () => {
		const input = page.locator('[data-gallery="date:empty"] input');
		await input.focus();
		await page.keyboard.type('2026/1/5');
		expect((await input.getAttribute('aria-invalid')) === null, 'no error while typing');
		await page.keyboard.press('Tab');
		const first = await input.inputValue();
		expect(first === '2026-01-05', `normalised to ${first}`);
		await input.focus();
		await input.fill('');
		await page.keyboard.type('24/11/2026');
		expect(
			(await input.getAttribute('aria-invalid')) === null,
			'no error while typing the first time'
		);
		await page.keyboard.press('Tab');
		const invalid = await input.getAttribute('aria-invalid');
		const message = await page
			.locator('[data-gallery="date:empty"] [aria-live="polite"]')
			.first()
			.textContent();
		expect(invalid === 'true' && /YYYY-MM-DD/.test(message ?? ''), `${invalid} ${message}`);
		await input.focus();
		await input.fill('');
		await page.keyboard.type('2026-11-');
		expect(
			(await input.getAttribute('aria-invalid')) === 'true',
			'still invalid mid-typing once shown'
		);
		await page.keyboard.type('24');
		expect(
			(await input.getAttribute('aria-invalid')) === null,
			'cleared by the input that fixes it'
		);
		await page.keyboard.press('Tab');
		return message?.trim();
	}
);
await check(
	'datetime: date pick moves focus to the time field; arrows step it; Enter closes',
	async () => {
		await tabTo('[data-gallery="date:datetime"] input');
		await page.keyboard.press('ArrowDown');
		await page.waitForFunction(() => document.activeElement?.matches('td[role="gridcell"]'));
		await page.keyboard.press('ArrowRight');
		await page.keyboard.press('Enter');
		await page.waitForFunction(() => document.activeElement?.matches('input.time'));
		expect(await popoverOpen(), 'popover closed after choosing the day');
		await page.keyboard.press('ArrowUp');
		await page.keyboard.press('Shift+ArrowUp');
		await page.keyboard.press('Enter');
		await settle();
		const a = await active();
		expect(a.tag === 'input' && a.inGallery === 'date:datetime', JSON.stringify(a));
		expect(a.value === '2026-11-25 15:45', `value ${a.value}`);
		expect(!(await popoverOpen()), 'popover still open');
		return a.value;
	}
);
await check('range: two Enters give an ordered range, whichever is chosen first', async () => {
	await tabTo('[data-gallery="date:range"] input');
	await page.keyboard.press('ArrowDown');
	await page.waitForFunction(() => document.activeElement?.matches('td[role="gridcell"]'));
	const opened = await activeDate();
	await page.keyboard.press('Enter');
	expect(await popoverOpen(), 'closed after the first pick');
	for (let i = 0; i < 5; i += 1) await page.keyboard.press('ArrowLeft');
	const preview = await page.evaluate(() => document.querySelectorAll('td.preview, td.sel').length);
	expect(preview >= 5, `preview band cells ${preview}`);
	await page.keyboard.press('Enter');
	await settle();
	const values = await page.evaluate(() =>
		[...document.querySelectorAll('[data-gallery="date:range"] input')].map((i) => i.value)
	);
	expect(
		values[0] === '2026-11-05' && values[1] === '2026-11-10',
		`opened ${opened}, values ${values.join(' / ')}`
	);
	expect(!(await popoverOpen()), 'popover still open');
	return values.join(' / ');
});

// ---- Select --------------------------------------------------------------------------------

const selectTrigger = (name) => `[data-gallery="${name}"] [role="combobox"]`;
const selectState = (selector) =>
	page.evaluate((s) => {
		const el = document.querySelector(s);
		const active = el.getAttribute('aria-activedescendant');
		const option = active ? document.getElementById(active) : null;
		return {
			expanded: el.getAttribute('aria-expanded'),
			active,
			activeText: option?.textContent?.trim() ?? null,
			activeSelected: option?.getAttribute('aria-selected') ?? null,
			text: el.textContent?.trim() ?? ''
		};
	}, selector);

await check(
	'select: ArrowDown opens with the selected option active; Escape restores focus and value',
	async () => {
		await tabTo(selectTrigger('select:default'));
		await page.keyboard.press('ArrowDown');
		await page.waitForSelector('[role="listbox"]');
		const s = await selectState(selectTrigger('select:default'));
		expect(s.expanded === 'true' && s.activeSelected === 'true', JSON.stringify(s));
		expect(/Review/.test(s.activeText ?? ''), `active ${s.activeText}`);
		await page.keyboard.press('ArrowUp');
		await page.keyboard.press('Escape');
		await page.waitForSelector('[role="listbox"]', { state: 'detached' });
		const after = await selectState(selectTrigger('select:default'));
		const a = await active();
		expect(after.expanded === 'false' && /Review/.test(after.text), JSON.stringify(after));
		expect(a.role === 'combobox' && a.inGallery === 'select:default', `focus ${JSON.stringify(a)}`);
		return `active ${s.activeText}, value ${after.text}`;
	}
);
await check(
	'select: type-ahead moves the active option; Enter chooses and returns focus',
	async () => {
		await page.keyboard.press('Enter');
		await page.waitForSelector('[role="listbox"]');
		await page.keyboard.type('d');
		const typed = await selectState(selectTrigger('select:default'));
		expect(/^Done/.test(typed.activeText ?? ''), `type-ahead gave ${typed.activeText}`);
		await page.keyboard.press('Home');
		const home = await selectState(selectTrigger('select:default'));
		expect(/Backlog/.test(home.activeText ?? ''), `Home gave ${home.activeText}`);
		await page.keyboard.press('End');
		const end = await selectState(selectTrigger('select:default'));
		expect(/Blocked/.test(end.activeText ?? ''), `End gave ${end.activeText}`);
		await page.keyboard.press('ArrowUp');
		await page.keyboard.press('Enter');
		await page.waitForSelector('[role="listbox"]', { state: 'detached' });
		const chosen = await selectState(selectTrigger('select:default'));
		const a = await active();
		expect(/Done/.test(chosen.text), `value ${chosen.text}`);
		expect(a.role === 'combobox' && a.inGallery === 'select:default', JSON.stringify(a));
		return chosen.text;
	}
);
await check('select: a disabled option stays in the list and cannot be chosen', async () => {
	await page.keyboard.press('Enter');
	await page.waitForSelector('[role="listbox"]');
	await page.keyboard.press('End');
	const disabled = await page.evaluate(() => {
		const el = document.activeElement;
		const option = document.getElementById(el.getAttribute('aria-activedescendant'));
		return { disabled: option.getAttribute('aria-disabled'), text: option.textContent.trim() };
	});
	await page.keyboard.press('Enter');
	const stillOpen = await count('[role="listbox"]');
	expect(disabled.disabled === 'true' && stillOpen === 1, JSON.stringify({ disabled, stillOpen }));
	expect(/Locked by the workflow/.test(disabled.text), `reason in the name: ${disabled.text}`);
	await page.keyboard.press('Escape');
	await page.waitForSelector('[role="listbox"]', { state: 'detached' });
});
await check('select: Tab closes the menu without a change and moves on', async () => {
	await page.keyboard.press('Enter');
	await page.waitForSelector('[role="listbox"]');
	await page.keyboard.press('ArrowUp');
	await page.keyboard.press('Tab');
	await page.waitForSelector('[role="listbox"]', { state: 'detached' });
	const after = await selectState(selectTrigger('select:default'));
	const a = await active();
	expect(/Done/.test(after.text), `value changed to ${after.text}`);
	expect(a.inGallery !== 'select:default', `focus stayed: ${JSON.stringify(a)}`);
});
await check(
	'select: more than eight options show a search field that filters; empty result says so',
	async () => {
		await tabTo(selectTrigger('select:search'));
		await page.keyboard.press('Enter');
		await page.waitForSelector('[role="listbox"]');
		const a = await active();
		expect(a.tag === 'input' && a.inPopover, `focus ${JSON.stringify(a)}`);
		await page.keyboard.type('11');
		const options = await page.evaluate(() =>
			[...document.querySelectorAll('[role="option"]')].map((o) => o.textContent.trim())
		);
		expect(options.length === 1 && /11/.test(options[0]), JSON.stringify(options));
		await page.locator('input[aria-label="Search"]').fill('');
		await page.keyboard.type('zzz');
		await page.waitForSelector('[role="status"]');
		const empty = await page.evaluate(() =>
			[...document.querySelectorAll('[role="status"]')].map((s) => s.textContent.trim()).join('|')
		);
		expect(/zzz/.test(empty), `empty message ${empty}`);
		await page.locator('input[aria-label="Search"]').fill('');
		await page.keyboard.type('Status 5');
		await page.keyboard.press('Enter');
		await page.waitForSelector('[role="listbox"]', { state: 'detached' });
		const s = await selectState(selectTrigger('select:search'));
		const back = await active();
		expect(
			/Status 5/.test(s.text) && back.role === 'combobox',
			`${s.text} ${JSON.stringify(back)}`
		);
		return s.text;
	}
);
await check(
	'select: Escape in the search field closes and returns focus to the trigger',
	async () => {
		await page.keyboard.press('Enter');
		await page.waitForSelector('[role="listbox"]');
		await page.keyboard.press('Escape');
		await page.waitForSelector('[role="listbox"]', { state: 'detached' });
		const a = await active();
		expect(a.role === 'combobox' && a.inGallery === 'select:search', JSON.stringify(a));
	}
);
await check('select: invalid and disabled states are exposed to assistive technology', async () => {
	const invalid = await attr(selectTrigger('select:invalid'), 'aria-invalid');
	const required = await attr(selectTrigger('select:invalid'), 'aria-required');
	const disabled = await page.evaluate(
		() => document.querySelector('[data-gallery="select:disabled"] [role="combobox"]').disabled
	);
	expect(
		invalid === 'true' && required === 'true' && disabled,
		`${invalid} ${required} ${disabled}`
	);
});

// ---- Multi-select --------------------------------------------------------------------------

const multi = '[data-gallery="multi-select:default"] [role="combobox"]';
await check(
	'multi-select: Space toggles an option and the list stays open; Escape keeps the choice',
	async () => {
		await tabTo(multi);
		await page.keyboard.press('Enter');
		await page.waitForSelector('[role="listbox"][aria-multiselectable="true"]');
		await page.keyboard.press('ArrowDown');
		await page.keyboard.press('Space');
		const selected = await count('[role="option"][aria-selected="true"]');
		expect((await count('[role="listbox"]')) === 1 && selected === 1, `selected ${selected}`);
		await page.keyboard.press('ArrowDown');
		await page.keyboard.press('Space');
		const two = await count('[role="option"][aria-selected="true"]');
		const footer = await page.evaluate(() =>
			document.querySelector('.footer .count')?.textContent?.trim()
		);
		expect(two === 2 && /2/.test(footer ?? ''), `two ${two}, footer ${footer}`);
		await page.keyboard.press('Escape');
		await page.waitForSelector('[role="listbox"]', { state: 'detached' });
		const a = await active();
		const name = await attr(multi, 'aria-label');
		expect(a.role === 'combobox' && /Labels: .+/.test(name ?? ''), `${JSON.stringify(a)} ${name}`);
		return name;
	}
);
await check(
	'multi-select: the footer Clear empties the selection and the list stays open',
	async () => {
		await page.keyboard.press('Enter');
		await page.waitForSelector('[role="listbox"]');
		await page.locator('.footer .clear-text').click();
		const selected = await count('[role="option"][aria-selected="true"]');
		const open = await count('[role="listbox"]');
		expect(selected === 0 && open === 1, `selected ${selected}, open ${open}`);
		await page.keyboard.press('Escape');
		await page.waitForSelector('[role="listbox"]', { state: 'detached' });
		expect(
			(await count('[data-gallery="multi-select:default"] .clear-all')) === 0,
			'Clear all still shown'
		);
	}
);
await check(
	'multi-select: Backspace twice removes the last chip; "+n" lists the hidden labels',
	async () => {
		const chips = '[data-gallery="multi-select:chips"]';
		const info = () =>
			page.evaluate((root) => {
				const el = document.querySelector(root);
				return {
					chips: [...el.querySelectorAll('.chip')].map((c) => c.textContent.trim()),
					more: el.querySelector('.more')?.getAttribute('title') ?? null,
					moreText: el.querySelector('.more')?.textContent?.trim() ?? null,
					name: el.querySelector('[role="combobox"]').getAttribute('aria-label')
				};
			}, chips);
		await tabTo(`${chips} [role="combobox"]`);
		const before = await info();
		expect(
			before.chips.length === 2 && before.moreText === '+1' && before.more,
			JSON.stringify(before)
		);
		await page.keyboard.press('Backspace');
		const marked = await count(`${chips} .chip.marked, ${chips} .more.marked`);
		expect(marked === 1, `marked chips ${marked}`);
		expect(
			(await info()).chips.length === 2 && (await info()).moreText === '+1',
			'first press removed'
		);
		await page.keyboard.press('Backspace');
		const after = await info();
		expect(after.moreText === null && after.chips.length === 2, JSON.stringify(after));
		await page.keyboard.press('Backspace');
		await page.keyboard.press('Backspace');
		const fewer = await info();
		expect(fewer.chips.length === 1, JSON.stringify(fewer));
		return `${before.name} -> ${after.name}`;
	}
);

// ---- Segmented control ---------------------------------------------------------------------

const seg = (name) => `[data-gallery="segmented:${name}"]`;
const checkedSegment = (name) =>
	page.evaluate(
		(s) =>
			document.querySelector(`${s} [role="radio"][aria-checked="true"]`)?.dataset.value ?? null,
		seg(name)
	);
await check(
	'segmented: one tab stop; arrows move focus and selection; Home and End; wraps',
	async () => {
		const stops = await page.evaluate(
			(s) =>
				[...document.querySelectorAll(`${s} [role="radio"]`)].filter((e) => e.tabIndex === 0)
					.length,
			seg('three')
		);
		expect(stops === 1, `tab stops ${stops}`);
		await tabTo(`${seg('three')} [role="radio"]`);
		expect((await checkedSegment('three')) === 'table', 'starts on table');
		await page.keyboard.press('ArrowRight');
		const a = await page.evaluate(() => document.activeElement.dataset.value);
		expect(a === 'board' && (await checkedSegment('three')) === 'board', `ArrowRight ${a}`);
		await page.keyboard.press('End');
		expect((await checkedSegment('three')) === 'calendar', 'End');
		await page.keyboard.press('ArrowRight');
		expect((await checkedSegment('three')) === 'table', 'wraps to the first');
		await page.keyboard.press('ArrowLeft');
		expect((await checkedSegment('three')) === 'calendar', 'wraps back');
		await page.keyboard.press('Home');
		expect((await checkedSegment('three')) === 'table', 'Home');
	}
);
await check('segmented: a disabled segment is skipped and aria-disabled', async () => {
	await tabTo(`${seg('disabled-segment')} [role="radio"]`);
	await page.keyboard.press('ArrowRight');
	const a = await page.evaluate(() => document.activeElement.dataset.value);
	const disabled = await page.evaluate(
		(s) => document.querySelector(`${s} [data-value="board"]`).getAttribute('aria-disabled'),
		seg('disabled-segment')
	);
	expect(a === 'calendar' && disabled === 'true', `focus ${a}, disabled ${disabled}`);
});
await check('segmented: focus ring is the focus token, 2px', async () => {
	await tabTo(`${seg('two')} [role="radio"]`);
	const o = await page.evaluate(() => {
		const style = getComputedStyle(document.activeElement);
		return { width: style.outlineWidth, style: style.outlineStyle };
	});
	expect(o.width === '2px' && o.style === 'solid', JSON.stringify(o));
});

// ---- Validation ----------------------------------------------------------------------------

// The earlier Tab walks passed through the live field and left it, which (correctly) showed its
// required error; start the validation checks from a fresh page.
await page.reload();
await page.waitForSelector('[data-gallery-section="form"]');
const live = '[data-gallery="form:live"]';
await check(
	'validation: no error while typing the first time; error on blur; clears without blur',
	async () => {
		const input = page.locator(`${live} input`);
		await input.focus();
		await page.keyboard.type('a');
		await page.keyboard.press('Backspace');
		expect((await input.getAttribute('aria-invalid')) === null, 'error shown while typing');
		await page.keyboard.press('Tab');
		expect((await input.getAttribute('aria-invalid')) === 'true', 'no error after blur');
		const message = await page.locator(`${live} [aria-live="polite"]`).first().textContent();
		expect((message ?? '').trim().length > 0, 'no message');
		await input.focus();
		await page.keyboard.type('x');
		expect(
			(await input.getAttribute('aria-invalid')) === null,
			'still invalid after a valid input'
		);
		const required = await input.getAttribute('aria-required');
		expect(required === 'true', `aria-required ${required}`);
	}
);
await check('validation: reason line lists missing fields as links that focus them', async () => {
	await page.locator(`${live} input`).fill('');
	await page.locator(`${live} input`).blur();
	const reason = page.locator('[data-gallery="form:reason"] [data-ui="submit-reason"]');
	const text = (await reason.textContent())?.replace(/\s+/g, ' ').trim();
	expect(/Fill in .+ to create the record\./.test(text ?? ''), `reason ${text}`);
	const disabled = await page
		.locator('[data-gallery="form:reason"] button:not(.link)')
		.first()
		.isDisabled();
	expect(disabled, 'submit is enabled while a field is missing');
	await reason.locator('button.link').first().click();
	const a = await page.evaluate(() => document.activeElement?.id);
	expect(a === 'gallery-form-title', `focus went to ${a}`);
	const many = await page
		.locator('[data-gallery="form:reason-many"] [data-ui="submit-reason"]')
		.textContent();
	expect(/and 2 more/.test(many ?? ''), `many ${many}`);
	expect(
		(await page.locator('[data-gallery="form:reason-many"] button.link').count()) === 3,
		'link count'
	);
	return text;
});
await check('validation: required marking follows the form marking', async () => {
	const required = await page.locator('[data-gallery="form:required"] label').textContent();
	const optional = await page.locator('[data-gallery="form:optional"] label').textContent();
	expect(
		/\(required\)/.test(required ?? '') && /\(optional\)/.test(optional ?? ''),
		`${required} / ${optional}`
	);
});

// ---- Pagination ----------------------------------------------------------------------------

await check('pagination: first page disables previous with the extended name', async () => {
	const prev = await page.evaluate(() => {
		const b = document.querySelector('[data-gallery="pagination:first"] .nav-button');
		return { disabled: b.getAttribute('aria-disabled'), name: b.getAttribute('aria-label') };
	});
	expect(prev.disabled === 'true' && /first page/.test(prev.name), JSON.stringify(prev));
});
await check('pagination: reaching the last page moves focus to previous', async () => {
	const root = '[data-gallery="pagination:default"]';
	const next = `${root} .nav-button[aria-label^="Next"]`;
	await page.locator(next).focus();
	await page.keyboard.press('Enter');
	await page.keyboard.press('Enter');
	await page.keyboard.press('Enter');
	await settle();
	const a = await page.evaluate(() => document.activeElement?.getAttribute('aria-label'));
	const range = await page.locator(`${root} .range`).textContent();
	const position = await page.locator(`${root} .position`).textContent();
	expect(/^Previous page$/.test(a ?? ''), `focus on ${a}; ${range}; ${position}`);
	expect(
		/81.+95 of 95/.test(range ?? '') && /Page 5 of 5/.test(position ?? ''),
		`${range}; ${position}`
	);
	return `${range.trim()} / ${position.trim()}`;
});
await check('pagination: a 340 wide bar hides rows per page and uses the short form', async () => {
	const root = '[data-gallery="pagination:narrow"]';
	const r = await page.evaluate((s) => {
		const el = document.querySelector(s);
		return {
			width: el.querySelector('nav').getBoundingClientRect().width,
			size: !!el.querySelector('.size'),
			range: el.querySelector('.range').textContent.trim(),
			position: el.querySelector('.position').textContent.trim()
		};
	}, root);
	expect(
		r.width <= 340 && !r.size && /\//.test(r.range) && /\//.test(r.position),
		JSON.stringify(r)
	);
	return `${r.range} ${r.position}`;
});
await check('pagination: unknown total drops "of" and keeps next enabled', async () => {
	const r = await page.evaluate(() => {
		const el = document.querySelector('[data-gallery="pagination:unknown-total"]');
		return {
			range: el.querySelector('.range').textContent.trim(),
			position: el.querySelector('.position').textContent.trim(),
			next: el.querySelector('.nav-button[aria-label^="Next"]').getAttribute('aria-disabled')
		};
	});
	expect(!/ of /.test(r.range + r.position) && r.next === null, JSON.stringify(r));
});

// ---- Locale independence (separate contexts) -------------------------------------------------

await check(
	'date: the field text is the same ISO value under browser locales de-DE and en-US',
	async () => {
		const texts = [];
		for (const browserLocale of ['de-DE', 'en-US']) {
			const { context, page: p } = await openGallery({ browserLocale });
			texts.push(await p.locator('[data-gallery="date:default"] input').inputValue());
			texts.push(await p.locator('[data-gallery="date:datetime"] input').inputValue());
			await context.close();
		}
		expect(texts[0] === texts[2] && texts[1] === texts[3], texts.join(' | '));
		expect(texts[0] === '2026-11-24' && texts[1] === '2026-11-24 14:30', texts.join(' | '));
		return texts.slice(0, 2).join(' | ');
	}
);
await check('date: with the app language zh the month label reads like 2026年11月', async () => {
	const { context, page: p } = await openGallery({ browserLocale: 'de-DE', appLanguage: 'zh' });
	await p.locator('[data-gallery="date:default"] input').focus();
	await p.keyboard.press('ArrowDown');
	await p.waitForSelector('[data-ui="calendar"] .heading');
	const heading = (await p.locator('[data-ui="calendar"] .heading').textContent())?.trim();
	const weekdays = await p.evaluate(() =>
		[...document.querySelectorAll('[data-ui="calendar"] th')]
			.map((t) => t.textContent.trim())
			.join('')
	);
	await context.close();
	expect(/^\d{4}年\d{1,2}月$/.test(heading ?? ''), `heading ${heading}`);
	expect(weekdays === '一二三四五六日', `weekdays ${weekdays}`);
	return `${heading} ${weekdays}`;
});

console.log(results.join('\n'));
console.log(`\n${results.length - failures} passed, ${failures} failed`);
await browser.close();
process.exit(failures > 0 ? 1 : 0);
