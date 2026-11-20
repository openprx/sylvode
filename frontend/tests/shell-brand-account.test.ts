/**
 * Brand, theme default and boot script, account menu.
 *
 * Run standalone: `bun tests/shell-brand-account.test.ts`
 */
import { readFileSync } from 'node:fs';
import { Suite, assert, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';

await installSvelteSsr();
const { BRAND, faviconDataUrl, faviconSvg } = await import('../src/lib/shell/brand/brand');
const { default: BrandMark } = await import('../src/lib/shell/brand/BrandMark.svelte');
const { DEFAULT_THEME, resolveTheme } = await import('../src/lib/theme');
const { LOCALE_OPTIONS, THEME_OPTIONS, normalizeAppLocale } =
	await import('../src/lib/shell/account/account-model');
const { default: AccountMenu } = await import('../src/lib/shell/account/AccountMenu.svelte');

const suite = new Suite('shell-brand-account');
const bootSource = readFileSync(new URL('../static/theme-boot.js', import.meta.url), 'utf8');

suite.check('the favicon is an SVG data URL with both colour schemes', () => {
	const url = faviconDataUrl();
	assert(url.startsWith('data:image/svg+xml,'), url.slice(0, 40));
	const svg = decodeURIComponent(url.slice('data:image/svg+xml,'.length));
	assertEqual(svg, faviconSvg(), 'round trip');
	assert(svg.includes('prefers-color-scheme:dark'), 'dark rule');
	assert(svg.includes(BRAND.favicon.light[0]) && svg.includes(BRAND.favicon.dark[1]), 'colours');
	assertEqual((svg.match(/<path /g) ?? []).length, 2, 'two shapes');
});

suite.check('brand data: name, symbol and favicon shapes', () => {
	assertEqual(BRAND.name, 'Sylvode', 'name');
	assertEqual(BRAND.symbol.viewBox, '0 0 64 64', 'viewBox');
	assert(BRAND.symbol.paths.length > 0, 'paths');
});

suite.check('the default theme follows the system', () => {
	assertEqual(DEFAULT_THEME, 'system', 'default');
	assertEqual(resolveTheme('system', true), 'dark', 'system dark');
	assertEqual(resolveTheme('system', false), 'light', 'system light');
	assertEqual(resolveTheme('light', true), 'light', 'explicit light');
	assertEqual(resolveTheme('dark', false), 'dark', 'explicit dark');
});

/** Runs the real boot script against a fake document and storage. */
function boot(options: { stored?: string | null; prefersDark: boolean; storageThrows?: boolean }) {
	const classes = new Set<string>();
	const fakeWindow = {
		matchMedia: (query: string) => ({ matches: query.includes('dark') && options.prefersDark })
	};
	const fakeDocument = { documentElement: { classList: { add: (c: string) => classes.add(c) } } };
	const fakeStorage = {
		getItem: () => {
			if (options.storageThrows) throw new Error('blocked');
			return options.stored ?? null;
		}
	};
	new Function('window', 'document', 'localStorage', bootSource)(
		fakeWindow,
		fakeDocument,
		fakeStorage
	);
	return classes.has('dark');
}

suite.check('theme-boot: nothing stored follows the system', () => {
	assertEqual(boot({ prefersDark: true }), true, 'dark system');
	assertEqual(boot({ prefersDark: false }), false, 'light system');
});
suite.check('theme-boot: explicit choices win over the system', () => {
	assertEqual(boot({ stored: 'light', prefersDark: true }), false, 'light');
	assertEqual(boot({ stored: 'dark', prefersDark: false }), true, 'dark');
	assertEqual(boot({ stored: 'system', prefersDark: true }), true, 'system');
});
suite.check('theme-boot: junk or blocked storage falls back to system, never throws', () => {
	assertEqual(boot({ stored: 'purple', prefersDark: true }), true, 'junk');
	assertEqual(
		boot({ prefersDark: true, storageThrows: true }),
		false,
		'blocked storage leaves light'
	);
});

suite.check('account model: options and locale normalisation', () => {
	assertEqual(THEME_OPTIONS.map((o) => o.value).join(','), 'light,dark,system', 'themes');
	assertEqual(LOCALE_OPTIONS.map((o) => o.label).join(','), '中文,English', 'languages');
	assertEqual(normalizeAppLocale('en'), 'en', 'en');
	assertEqual(normalizeAppLocale('fr'), 'zh', 'other falls back');
	assertEqual(normalizeAppLocale(null), 'zh', 'null');
});

await suite.checkAsync('BrandMark: named image, or hidden when decorative', async () => {
	const named = await renderToHtml(BrandMark, {});
	assert(named.includes('role="img"') && named.includes('aria-label="Sylvode"'), named);
	assert(named.includes('fill="currentColor"') && named.includes('width="28"'), 'mono, 28');
	const decorative = await renderToHtml(BrandMark, { decorative: true, size: 24 });
	assert(
		decorative.includes('aria-hidden="true"') && !decorative.includes('role="img"'),
		decorative
	);
});

await suite.checkAsync(
	'AccountMenu: header, radio groups, settings link and sign out',
	async () => {
		const html = await renderToHtml(AccountMenu, {
			open: true,
			anchor: null,
			onclose: () => {},
			user: { name: 'Ada Lovelace', email: 'ada@example.test' },
			theme: 'dark',
			locale: 'en',
			ontheme: () => {},
			onlocale: () => {},
			onsignout: () => {}
		});
		assert(html.includes('data-shell="account-menu"'), 'marker');
		assert(html.includes('Ada Lovelace') && html.includes('ada@example.test'), 'header');
		const dark = /<[^>]*data-theme-option="dark"[^>]*>/.exec(html)?.[0] ?? '';
		const light = /<[^>]*data-theme-option="light"[^>]*>/.exec(html)?.[0] ?? '';
		assert(dark.includes('role="menuitemradio"') && dark.includes('aria-checked="true"'), dark);
		assert(light.includes('aria-checked="false"'), light);
		const en = /<[^>]*data-locale-option="en"[^>]*>/.exec(html)?.[0] ?? '';
		assert(en.includes('aria-checked="true"'), en);
		assertEqual((html.match(/data-theme-option=/g) ?? []).length, 3, 'three themes');
		assertEqual((html.match(/data-locale-option=/g) ?? []).length, 2, 'two languages');
		const settings = /<a[^>]*data-shell="account-settings"[^>]*>/.exec(html)?.[0] ?? '';
		assert(settings.includes('href="/settings"'), settings);
		assert(html.includes('data-shell="account-signout"'), 'sign out');
	}
);

finish(suite.result());
