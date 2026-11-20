/** Pure windowing, layout thresholds and wording for the pagination bar. */

export interface PageInput {
	page: number;
	pageSize: number;
	/** `null` when the total is not known (a "load more" style API). */
	total: number | null;
	/** With an unknown total: another page exists. */
	hasMore?: boolean;
	/** With an unknown total: how many rows the current page really holds. */
	shown?: number;
}

export interface PageModel {
	/** Nothing is rendered for an empty list or a single page. */
	render: boolean;
	page: number;
	pages: number | null;
	from: number;
	to: number;
	total: number | null;
	isFirst: boolean;
	isLast: boolean;
}

export function pageModel({ page, pageSize, total, hasMore = false, shown }: PageInput): PageModel {
	const size = Math.max(1, pageSize);
	const current = Math.max(1, page);
	const pages = total === null ? null : Math.max(1, Math.ceil(total / size));
	const from = total === 0 ? 0 : (current - 1) * size + 1;
	let to: number;
	if (total !== null) to = Math.min(current * size, total);
	else to = shown !== undefined ? from + Math.max(0, shown) - 1 : current * size;
	const isLast = pages !== null ? current >= pages : !hasMore;
	return {
		render: total !== null ? total > size : current > 1 || hasMore,
		page: current,
		pages,
		from,
		to,
		total,
		isFirst: current <= 1,
		isLast
	};
}

export interface BarLayout {
	/** The rows-per-page control is hidden below 480. */
	showSize: boolean;
	/** Below 400 the short wording is used. */
	short: boolean;
}

export function layoutFor(width: number): BarLayout {
	return { showSize: width >= 480, short: width < 400 };
}

/** The button that keeps focus after a page change: the pressed one, unless it is now disabled. */
export function focusAfterPage(pressed: 'prev' | 'next', model: PageModel): 'prev' | 'next' {
	if (pressed === 'next' && model.isLast) return 'prev';
	if (pressed === 'prev' && model.isFirst) return 'next';
	return pressed;
}

export type Translate = (key: string, values?: Record<string, string | number>) => string;

export function formatCount(n: number, locale: string): string {
	return new Intl.NumberFormat(locale).format(n);
}

/** "Showing 101–150 of 1,240", or the short "101–150 / 1,240", or "Loading…". */
export function rangeText(
	model: PageModel,
	options: { loading: boolean; short: boolean; locale: string },
	t: Translate
): string {
	if (options.loading) return t('ui.pagination.loading');
	const values = {
		from: formatCount(model.from, options.locale),
		to: formatCount(model.to, options.locale),
		total: model.total === null ? '' : formatCount(model.total, options.locale)
	};
	const known = model.total !== null;
	const key = options.short
		? known
			? 'ui.pagination.rangeShort'
			: 'ui.pagination.rangeShortNoTotal'
		: known
			? 'ui.pagination.range'
			: 'ui.pagination.rangeNoTotal';
	return t(key, values);
}

/** "Page 3 of 25", "3 / 25" when short, "Page 2" with an unknown total. */
export function positionText(
	model: PageModel,
	options: { short: boolean; locale: string },
	t: Translate
): string {
	const values = {
		page: formatCount(model.page, options.locale),
		pages: model.pages === null ? '' : formatCount(model.pages, options.locale)
	};
	const known = model.pages !== null;
	const key = options.short
		? known
			? 'ui.pagination.positionShort'
			: 'ui.pagination.positionShortNoTotal'
		: known
			? 'ui.pagination.position'
			: 'ui.pagination.positionNoTotal';
	return t(key, values);
}
