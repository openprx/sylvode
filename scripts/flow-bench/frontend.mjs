/**
 * Logged-in Flow page timings: navigation, Web Vitals, typing, screenshots, Lighthouse.
 * The stack is already up. Credentials come from FLOW_BENCH_IDS and never go to stdout.
 */
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { chromium } from '@playwright/test';

const idsPath = process.env.FLOW_BENCH_IDS;
const outDir = process.env.FLOW_BENCH_OUT;
const stateDir = process.env.FLOW_BENCH_STATE || outDir;
const webOrigin = process.env.FLOW_BENCH_WEB_ORIGIN;
const lighthouseRoot = process.env.LIGHTHOUSE_ROOT || '';
const debugPort = 9333;

if (!idsPath || !outDir || !webOrigin) {
	throw new Error('FLOW_BENCH_IDS, FLOW_BENCH_OUT, and FLOW_BENCH_WEB_ORIGIN are required');
}

const ids = JSON.parse(await readFile(idsPath, 'utf8'));
const shotDir = path.join(outDir, 'screenshots');
await mkdir(shotDir, { recursive: true });
await mkdir(path.join(outDir, 'parts'), { recursive: true });

function stats(samples) {
	if (!samples.length) return { n: 0 };
	const ordered = [...samples].sort((a, b) => a - b);
	const at = (percent) => ordered[Math.floor(((ordered.length - 1) * percent) / 100)];
	return {
		n: ordered.length,
		min_ms: ordered[0],
		p50_ms: at(50),
		p95_ms: at(95),
		max_ms: ordered[ordered.length - 1],
		samples_ms: ordered
	};
}

async function loadLighthouse() {
	if (!lighthouseRoot) return { error: 'lighthouse package was not installed' };
	const candidates = [
		'lighthouse/core/index.js',
		'lighthouse/dist/index.js',
		'lighthouse/index.js'
	];
	const errors = [];
	for (const candidate of candidates) {
		const file = path.join(lighthouseRoot, 'node_modules', candidate);
		try {
			const loaded = await import(pathToFileURL(file).href);
			const runner = loaded.default ?? loaded;
			if (typeof runner === 'function') return { runner };
			errors.push(`${candidate}: export is not a function`);
		} catch (error) {
			errors.push(`${candidate}: ${error instanceof Error ? error.message : String(error)}`);
		}
	}
	return { error: errors.join('; ') };
}

function metricValue(audits, id) {
	const audit = audits?.[id];
	return typeof audit?.numericValue === 'number' ? audit.numericValue : null;
}

async function readVitals(page) {
	return page.evaluate(() => {
		const nav = performance.getEntriesByType('navigation')[0];
		const paints = performance.getEntriesByType('paint');
		const fcp = paints.find((entry) => entry.name === 'first-contentful-paint');
		return {
			ttfb_ms: nav ? nav.responseStart : null,
			dom_content_loaded_ms: nav ? nav.domContentLoadedEventEnd : null,
			load_event_ms: nav ? nav.loadEventEnd : null,
			fcp_ms: fcp ? fcp.startTime : window.__vitals?.fcp ?? null,
			lcp_ms: window.__vitals?.lcp ?? null,
			cls: window.__vitals?.cls ?? null
		};
	});
}

async function measurePage(page, label, objectId) {
	const route = `/workspace/${ids.workspace_id}/flow/${objectId}`;
	const result = {
		label,
		object_id: objectId,
		url: '',
		canvas_wall_ms: null,
		editor_wall_ms: null,
		vitals: null,
		screenshot: null,
		typing: { ok: false, error: 'not attempted' },
		page_errors: []
	};
	const onError = (error) => {
		if (result.page_errors.length < 8) result.page_errors.push(String(error));
	};
	page.on('pageerror', onError);
	const started = Date.now();
	try {
		await page.goto(route, { waitUntil: 'domcontentloaded', timeout: 180_000 });
		result.url = page.url();
		const canvas = page.locator('[data-testid="flow-canvas"]');
		await canvas.waitFor({ state: 'visible', timeout: 180_000 });
		result.canvas_wall_ms = Date.now() - started;
		const editor = page.locator('[data-testid="flow-canvas"] .ProseMirror');
		try {
			await editor.waitFor({ state: 'visible', timeout: 90_000 });
			result.editor_wall_ms = Date.now() - started;
		} catch (error) {
			result.typing = {
				ok: false,
				error: `editor did not become visible: ${error instanceof Error ? error.message : String(error)}`
			};
		}
		await page.waitForTimeout(2000);
		result.vitals = await readVitals(page);
		const shotName = `flow-page-${label}.png`;
		await page.screenshot({
			path: path.join(shotDir, shotName),
			fullPage: false,
			timeout: 30_000
		});
		result.screenshot = `screenshots/${shotName}`;
		if (result.editor_wall_ms != null) {
			result.typing = await measureTyping(page);
			if (result.typing.ok) {
				const typedName = `flow-page-${label}-typed.png`;
				await page.screenshot({
					path: path.join(shotDir, typedName),
					fullPage: false,
					timeout: 30_000
				});
				result.typed_screenshot = `screenshots/${typedName}`;
			}
		}
		result.ok = result.vitals != null && result.screenshot != null;
	} catch (error) {
		result.ok = false;
		result.error = error instanceof Error ? error.message : String(error);
		result.url = page.url();
		try {
			const shotName = `flow-page-${label}-failed.png`;
			await page.screenshot({ path: path.join(shotDir, shotName), fullPage: false, timeout: 15_000 });
			result.screenshot = `screenshots/${shotName}`;
		} catch {
			/* the failure screenshot is optional */
		}
	} finally {
		page.off('pageerror', onError);
	}
	return result;
}

async function measureTyping(page) {
	const chars = 'abcdefghijKLMNOPQRST';
	try {
		const editor = page.locator('[data-testid="flow-canvas"] .ProseMirror').first();
		await editor.click({ timeout: 15_000 });
		await page.evaluate(() => {
			const root = document.querySelector('[data-testid="flow-canvas"] .ProseMirror');
			window.__typeSamples = [];
			if (!root) return;
			let started = 0;
			root.addEventListener(
				'keydown',
				() => {
					started = performance.now();
				},
				true
			);
			const observer = new MutationObserver(() => {
				if (!started) return;
				window.__typeSamples.push(performance.now() - started);
				started = 0;
			});
			observer.observe(root, { subtree: true, characterData: true, childList: true });
		});
		for (const ch of chars) {
			const before = await page.evaluate(() => window.__typeSamples.length);
			await page.keyboard.type(ch);
			await page.waitForFunction((count) => window.__typeSamples.length > count, before, {
				timeout: 8_000
			});
		}
		const samples = await page.evaluate(() => window.__typeSamples);
		return { ok: true, chars: chars.length, latency: stats(samples), note: 'keydown 到编辑器 DOM 变化' };
	} catch (error) {
		return { ok: false, error: error instanceof Error ? error.message : String(error) };
	}
}

async function runLighthouse(runner, url) {
	const flags = {
		port: debugPort,
		output: 'json',
		logLevel: 'error',
		onlyCategories: ['performance'],
		disableStorageReset: true,
		formFactor: 'desktop',
		screenEmulation: { disabled: true }
	};
	const result = await runner(url, flags);
	const lhr = result?.lhr ?? {};
	const audits = lhr.audits ?? {};
	const metrics = {
		'largest-contentful-paint': metricValue(audits, 'largest-contentful-paint'),
		'cumulative-layout-shift': metricValue(audits, 'cumulative-layout-shift'),
		'first-contentful-paint': metricValue(audits, 'first-contentful-paint'),
		'total-blocking-time': metricValue(audits, 'total-blocking-time'),
		'speed-index': metricValue(audits, 'speed-index'),
		interactive: metricValue(audits, 'interactive'),
		'server-response-time': metricValue(audits, 'server-response-time')
	};
	return {
		ok: true,
		requested_url: lhr.requestedUrl ?? url,
		final_url: lhr.finalUrl ?? null,
		performance_score: lhr.categories?.performance?.score ?? null,
		metrics
	};
}

const session = {
	accessToken: ids.token,
	refreshToken: ids.refresh_token,
	authUser: ids.user
};

let context;
let debugArgsWorked = true;
let launchError = '';
try {
	context = await chromium.launchPersistentContext(path.join(stateDir, 'chrome-profile'), {
		executablePath: process.env.CHROMIUM_BIN || undefined,
		headless: true,
		viewport: { width: 1440, height: 900 },
		args: [`--remote-debugging-port=${debugPort}`]
	});
} catch (error) {
	debugArgsWorked = false;
	launchError = error instanceof Error ? error.message : String(error);
	context = await chromium.launchPersistentContext(path.join(stateDir, 'chrome-profile-plain'), {
		executablePath: process.env.CHROMIUM_BIN || undefined,
		headless: true,
		viewport: { width: 1440, height: 900 }
	});
}

await context.addInitScript((value) => {
	try {
		localStorage.setItem('auth_token', value.accessToken);
		localStorage.setItem('refresh_token', value.refreshToken);
		localStorage.setItem('auth_user', JSON.stringify(value.authUser));
		localStorage.setItem('locale', 'en');
	} catch {
		return;
	}
	window.__vitals = { lcp: null, cls: 0, fcp: null };
	try {
		new PerformanceObserver((list) => {
			for (const entry of list.getEntries()) {
				window.__vitals.lcp = entry.startTime;
			}
		}).observe({ type: 'largest-contentful-paint', buffered: true });
		new PerformanceObserver((list) => {
			for (const entry of list.getEntries()) {
				if (!entry.hadRecentInput) window.__vitals.cls += entry.value;
			}
		}).observe({ type: 'layout-shift', buffered: true });
		new PerformanceObserver((list) => {
			for (const entry of list.getEntries()) {
				if (entry.name === 'first-contentful-paint') window.__vitals.fcp = entry.startTime;
			}
		}).observe({ type: 'paint', buffered: true });
	} catch {
		/* observers are additive; navigation timing still lands */
	}
}, session);

const page = context.pages()[0] || (await context.newPage());
const pages = [];
pages.push(await measurePage(page, '1k', ids.pages.k1.id));
pages.push(await measurePage(page, '5k', ids.pages.k5.id));

let lighthouse = { ok: false, error: 'not attempted' };
if (!debugArgsWorked) {
	lighthouse = { ok: false, error: `browser did not expose a debugging port: ${launchError}` };
} else {
	const loaded = await loadLighthouse();
	if (!loaded.runner) {
		lighthouse = { ok: false, error: loaded.error };
	} else {
		try {
			const target = new URL(`/workspace/${ids.workspace_id}/flow/${ids.pages.k1.id}`, webOrigin).href;
			lighthouse = await runLighthouse(loaded.runner, target);
		} catch (error) {
			lighthouse = { ok: false, error: error instanceof Error ? error.message : String(error) };
		}
	}
}

const oneK = pages[0];
const payload = {
	ok: Boolean(oneK?.ok && oneK.screenshot),
	pages,
	lighthouse
};
await writeFile(path.join(outDir, 'parts', 'frontend.json'), JSON.stringify(payload, null, 2) + '\n');
try {
	await context.close();
} catch {
	/* Lighthouse may already have closed the browser it attached to. */
}
if (!payload.ok) {
	process.exitCode = 1;
}
