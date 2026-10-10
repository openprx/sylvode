// Navigator / command palette Flow search (`contracts/ui-surface-v1.md` "后续版本 UI 派生" v0.5:
// "200ms debounce、AbortController 取消旧请求、键盘遍历；结果显示 `stale`/lag badge，打开时再 GET
// 当前对象"; `rest-api-v1.md` `GET /workspaces/{workspace_id}/flow/search`; `ADR-0009`).
//
// Rules this module owns:
// - typing only schedules a request; the request leaves after `debounceMs` of quiet, so a burst
//   of keystrokes inside the window sends exactly one;
// - starting a request aborts the previous one, and a response whose signal was aborted is
//   dropped: an older, slower response can never overwrite the newer result;
// - a hit is marked stale when the server says so or its projection lag is non-zero; the list
//   only identifies an object -- opening it goes to the object page, which reads the current
//   object (a snippet is never treated as current content);
// - `freshness=require_current` is opt-in; when the server fails it closed (`stale_frontier`) the
//   error is shown as such and the request is NOT silently repeated with `allow_stale`;
// - errors are classified by `error_code` only, never `message`.

import type { ApiResult } from '$lib/api/client';
import type { FlowSearchHit, FlowSearchQuery, FlowSearchResponse } from '$lib/api/flow';
import { flowErrorFromEnvelope, flowErrorI18nKey } from './errors';

export const SEARCH_DEBOUNCE_MS = 200;
export const SEARCH_RESULT_LIMIT = 20;
/** `flow/search.rs::validate`: `q` must contain 1..=256 characters. */
export const SEARCH_QUERY_MAX_CHARS = 256;

export type SearchFreshness = 'allow_stale' | 'require_current';

export const SEARCH_STALE_FRONTIER_KEY = 'flow.search.error.staleFrontier';
export const SEARCH_TOO_BROAD_KEY = 'flow.search.error.tooBroad';
export const SEARCH_TOO_LONG_KEY = 'flow.search.error.tooLong';
export const SEARCH_GENERIC_ERROR_KEY = 'flow.search.error.generic';

export interface SnippetSegment {
	readonly text: string;
	readonly match: boolean;
}

export interface SearchRow {
	readonly id: string;
	readonly title: string;
	readonly objectType: string;
	readonly stale: boolean;
	readonly lag: number;
	readonly snippet: readonly SnippetSegment[];
}

export type SearchStatus = 'idle' | 'pending' | 'loading' | 'ready' | 'error';

export interface SearchSnapshot {
	readonly query: string;
	readonly freshness: SearchFreshness;
	readonly status: SearchStatus;
	readonly rows: readonly SearchRow[];
	/** Index into `rows`, or -1 when nothing is active. */
	readonly activeIndex: number;
	readonly failureKey: string | null;
	readonly failureCode: string | null;
	/** The response frontier (`index_frontier.stale`): results may be missing newer content. */
	readonly frontierStale: boolean;
	readonly frontierLag: number;
}

export interface SearchReads {
	search(
		workspaceId: string,
		query: FlowSearchQuery,
		signal?: AbortSignal
	): Promise<ApiResult<FlowSearchResponse>>;
}

export interface NavigatorSearchDeps {
	readonly reads: SearchReads;
	readonly debounceMs: number;
	readonly setTimer: (callback: () => void, ms: number) => unknown;
	readonly clearTimer: (handle: unknown) => void;
}

/** A hit is stale when the server flags it or its index lags the document head. */
export function isStaleHit(hit: Pick<FlowSearchHit, 'stale' | 'projection_lag'>): boolean {
	return hit.stale === true || (Number.isFinite(hit.projection_lag) && hit.projection_lag > 0);
}

function unescapeEntities(text: string): string {
	return text.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');
}

/**
 * Splits a server snippet into plain-text segments. The server escapes `&`, `<`, `>` in the
 * indexed text and then wraps matches in `<mark>`/`</mark>` (`flow/search.rs`), so the only real
 * markup is those two tags; everything else is text. Rendering the segments as text (never as
 * HTML) keeps literal `<mark>` typed by a user from becoming markup.
 */
export function parseSnippet(raw: string): SnippetSegment[] {
	const segments: SnippetSegment[] = [];
	const pattern = /<mark>([\s\S]*?)<\/mark>/g;
	let last = 0;
	for (const match of raw.matchAll(pattern)) {
		const start = match.index ?? 0;
		if (start > last)
			segments.push({ text: unescapeEntities(raw.slice(last, start)), match: false });
		if (match[1]) segments.push({ text: unescapeEntities(match[1]), match: true });
		last = start + match[0].length;
	}
	if (last < raw.length) segments.push({ text: unescapeEntities(raw.slice(last)), match: false });
	return segments.filter((segment) => segment.text.length > 0);
}

export function searchRowFromHit(hit: FlowSearchHit): SearchRow {
	const raw = hit.snippets?.plain_text ?? hit.snippets?.title ?? '';
	return {
		id: hit.object.id,
		title: hit.object.title,
		objectType: hit.object.object_type,
		stale: isStaleHit(hit),
		lag: Number.isFinite(hit.projection_lag) ? Math.max(0, hit.projection_lag) : 0,
		snippet: parseSnippet(raw)
	};
}

/** Next active index for a key; `null` when the key is not a list navigation key. Clamped at both
 * ends (no wrap): ArrowUp on the first row stays there, ArrowDown on the last row stays there. */
export function nextActiveIndex(current: number, count: number, key: string): number | null {
	if (count <= 0)
		return key === 'ArrowDown' || key === 'ArrowUp' || key === 'Home' || key === 'End' ? -1 : null;
	switch (key) {
		case 'ArrowDown':
			return Math.min(Math.max(current, -1) + 1, count - 1);
		case 'ArrowUp':
			return current <= 0 ? 0 : Math.min(current - 1, count - 1);
		case 'Home':
			return 0;
		case 'End':
			return count - 1;
		default:
			return null;
	}
}

/** The i18n key for a rejected search, from `error_code` only. */
export function searchFailureKey(
	result: Pick<ApiResult<unknown>, 'code' | 'error_code' | 'details'>
): {
	key: string;
	code: string | null;
} {
	const error = flowErrorFromEnvelope(result);
	if (!error) return { key: SEARCH_GENERIC_ERROR_KEY, code: null };
	if (error.code === 'stale_frontier') return { key: SEARCH_STALE_FRONTIER_KEY, code: error.code };
	if (error.code === 'limit_exceeded') return { key: SEARCH_TOO_BROAD_KEY, code: error.code };
	return { key: flowErrorI18nKey(error), code: error.code };
}

export class FlowNavigatorSearch {
	private readonly deps: NavigatorSearchDeps;
	private query = '';
	private freshness: SearchFreshness = 'allow_stale';
	private status: SearchStatus = 'idle';
	private rows: SearchRow[] = [];
	private activeIndex = -1;
	private failureKey: string | null = null;
	private failureCode: string | null = null;
	private frontierStale = false;
	private frontierLag = 0;
	private timer: unknown = null;
	private inflight: AbortController | null = null;

	constructor(
		/** A getter keeps a component from capturing its prop's first value. */
		private readonly workspace: string | (() => string),
		private readonly onChange: (snapshot: SearchSnapshot) => void = () => {},
		deps: Partial<NavigatorSearchDeps> & { reads: SearchReads }
	) {
		this.deps = {
			reads: deps.reads,
			debounceMs: deps.debounceMs ?? SEARCH_DEBOUNCE_MS,
			setTimer: deps.setTimer ?? ((callback, ms) => setTimeout(callback, ms)),
			clearTimer:
				deps.clearTimer ?? ((handle) => clearTimeout(handle as ReturnType<typeof setTimeout>))
		};
	}

	private workspaceId(): string {
		return typeof this.workspace === 'function' ? this.workspace() : this.workspace;
	}

	get snapshot(): SearchSnapshot {
		return {
			query: this.query,
			freshness: this.freshness,
			status: this.status,
			rows: [...this.rows],
			activeIndex: this.activeIndex,
			failureKey: this.failureKey,
			failureCode: this.failureCode,
			frontierStale: this.frontierStale,
			frontierLag: this.frontierLag
		};
	}

	get activeRow(): SearchRow | null {
		return this.rows[this.activeIndex] ?? null;
	}

	/** A keystroke. Schedules one request after the debounce window; an empty query clears. */
	setQuery(raw: string): void {
		this.query = raw;
		this.cancelTimer();
		const q = raw.trim();
		if (q.length === 0) {
			this.abortInflight();
			this.reset('idle');
			return;
		}
		if ([...q].length > SEARCH_QUERY_MAX_CHARS) {
			this.abortInflight();
			this.reset('error');
			this.failureKey = SEARCH_TOO_LONG_KEY;
			this.emit();
			return;
		}
		this.status = 'pending';
		this.emit();
		this.timer = this.deps.setTimer(() => {
			this.timer = null;
			void this.run();
		}, this.deps.debounceMs);
	}

	/** Toggles `require_current`. Re-runs the current query at once (no debounce: not typing). */
	setFreshness(freshness: SearchFreshness): void {
		if (freshness === this.freshness) return;
		this.freshness = freshness;
		this.cancelTimer();
		if (this.query.trim().length === 0) {
			this.emit();
			return;
		}
		void this.run();
	}

	/** Arrow/Home/End handling; returns whether the key was consumed. */
	handleKey(key: string): boolean {
		const next = nextActiveIndex(this.activeIndex, this.rows.length, key);
		if (next === null) return false;
		this.activeIndex = next;
		this.emit();
		return true;
	}

	setActive(index: number): void {
		if (index < -1 || index >= this.rows.length) return;
		this.activeIndex = index;
		this.emit();
	}

	clear(): void {
		this.query = '';
		this.cancelTimer();
		this.abortInflight();
		this.reset('idle');
	}

	dispose(): void {
		this.cancelTimer();
		this.abortInflight();
	}

	private async run(): Promise<void> {
		const q = this.query.trim();
		if (q.length === 0) return;
		this.abortInflight();
		const controller = new AbortController();
		this.inflight = controller;
		this.status = 'loading';
		this.emit();
		const result = await this.deps.reads.search(
			this.workspaceId(),
			{ q, all_visible: true, freshness: this.freshness, limit: SEARCH_RESULT_LIMIT },
			controller.signal
		);
		// A newer request (or a clear) aborted this one: its answer belongs to an old query.
		if (controller.signal.aborted) return;
		this.inflight = null;
		if (result.code !== 0 || !result.data) {
			const failure = searchFailureKey(result);
			this.rows = [];
			this.activeIndex = -1;
			this.frontierStale = false;
			this.frontierLag = 0;
			this.status = 'error';
			this.failureKey = failure.key;
			this.failureCode = failure.code;
			this.emit();
			return;
		}
		this.rows = (result.data.items ?? []).map(searchRowFromHit);
		this.activeIndex = this.rows.length > 0 ? 0 : -1;
		this.frontierStale = result.data.index_frontier?.stale === true;
		this.frontierLag = Math.max(0, result.data.index_frontier?.lag ?? 0);
		this.failureKey = null;
		this.failureCode = null;
		this.status = 'ready';
		this.emit();
	}

	private reset(status: SearchStatus): void {
		this.status = status;
		this.rows = [];
		this.activeIndex = -1;
		this.failureKey = null;
		this.failureCode = null;
		this.frontierStale = false;
		this.frontierLag = 0;
		this.emit();
	}

	private cancelTimer(): void {
		if (this.timer !== null) {
			this.deps.clearTimer(this.timer);
			this.timer = null;
		}
	}

	private abortInflight(): void {
		this.inflight?.abort();
		this.inflight = null;
	}

	private emit(): void {
		this.onChange(this.snapshot);
	}
}
