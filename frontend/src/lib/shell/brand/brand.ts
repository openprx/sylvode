/**
 * The product's logo, in one replaceable file. Provisional: concept 01 "Interlock" (the owner has
 * not confirmed a logo). Swapping the logo means replacing the contents of this file only: the
 * rail, the favicon link and every other caller read `BRAND` and `faviconDataUrl()`.
 *
 * This is the only file allowed to hold literal hex colours (the favicon is an image the page's
 * tokens cannot reach); it is listed in `scripts/design-lint-allow.json` as "brand favicon colours".
 */

export interface BrandSymbol {
	viewBox: string;
	/** Path data; drawn in one colour (`currentColor`). */
	paths: string[];
}

export interface BrandFavicon {
	viewBox: string;
	/** Two shapes: the first is drawn in the ink colour, the second in the accent colour. */
	paths: [string, string];
	/** `[ink, accent]` on a light browser tab strip. */
	light: [string, string];
	/** `[ink, accent]` on a dark browser tab strip. */
	dark: [string, string];
}

export interface Brand {
	name: string;
	symbol: BrandSymbol;
	favicon: BrandFavicon;
}

export const BRAND: Brand = {
	name: 'Sylvode',
	symbol: {
		viewBox: '0 0 64 64',
		paths: [
			'M13 4H51A9 9 0 0 1 60 13A1.5 1.5 0 0 1 58.5 14.5H25A10.5 10.5 0 0 0 25 35.5H39A3.5 3.5 0 0 1 39 42.5H5.5A1.5 1.5 0 0 1 4 41V13A9 9 0 0 1 13 4Z',
			'M51 60H13A9 9 0 0 1 4 51A1.5 1.5 0 0 1 5.5 49.5H39A10.5 10.5 0 0 0 39 28.5H25A3.5 3.5 0 0 1 25 21.5H58.5A1.5 1.5 0 0 1 60 23V51A9 9 0 0 1 51 60Z'
		]
	},
	favicon: {
		viewBox: '0 0 16 16',
		paths: [
			'M3 0H13A3 3 0 0 1 16 3H6A3 3 0 0 0 6 9H10A1 1 0 0 1 10 11H0V3A3 3 0 0 1 3 0Z',
			'M13 16H3A3 3 0 0 1 0 13H10A3 3 0 0 0 10 7H6A1 1 0 0 1 6 5H16V13A3 3 0 0 1 13 16Z'
		],
		light: ['#444444', '#1F8A7A'],
		dark: ['#F2F2F2', '#63CDB8']
	}
};

/** The favicon as SVG text; it switches to the dark colours with the browser's colour scheme. */
export function faviconSvg(brand: Brand = BRAND): string {
	const { favicon } = brand;
	const [inkLight, accentLight] = favicon.light;
	const [inkDark, accentDark] = favicon.dark;
	return (
		`<svg xmlns="http://www.w3.org/2000/svg" viewBox="${favicon.viewBox}">` +
		`<style>.a{fill:${inkLight}}.b{fill:${accentLight}}` +
		`@media (prefers-color-scheme:dark){.a{fill:${inkDark}}.b{fill:${accentDark}}}</style>` +
		`<path class="a" d="${favicon.paths[0]}"/><path class="b" d="${favicon.paths[1]}"/></svg>`
	);
}

/** `data:` URL for `<link rel="icon">`. */
export function faviconDataUrl(brand: Brand = BRAND): string {
	return `data:image/svg+xml,${encodeURIComponent(faviconSvg(brand))}`;
}
