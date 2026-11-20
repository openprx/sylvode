<script lang="ts" module>
	export type PanelStatus = 'loading' | 'ready' | 'error' | 'forbidden' | 'missing';
	export type PanelMode = 'docked' | 'overlay' | 'sheet';
	export const PANEL_DEFAULT_WIDTH = 480;
</script>

<script lang="ts">
	/**
	 * Read-only record details beside the record list.
	 *
	 * Docked next to the list when the content column is wide, floating over the inline-end part of
	 * the list on medium widths, and a full-screen modal sheet on narrow screens. Fields follow the
	 * form's detail layout and render through `FieldValue`, so nothing here shows raw JSON, data
	 * URIs or HTML source. Plugin panels live in a collapsed "Apps" section and are mounted only
	 * once the user expands it, so browsing records never creates plugin frames.
	 */
	import { onDestroy, tick } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import {
		AlertTriangle,
		Check,
		ChevronDown,
		ChevronLeft,
		ChevronRight,
		ChevronUp,
		Copy,
		ExternalLink,
		Lock,
		MoreHorizontal,
		Pencil,
		RefreshCw,
		Trash2,
		X
	} from '@lucide/svelte';
	import type { FormChildRecord, FormField, FormRecord, UniversalForm } from '#lib/api/forms.js';
	import type { RecordPluginActionSurface } from '#lib/plugins/placements.js';
	import type { DetailHeaderFields, DetailSection } from '#lib/forms/detail-layout.js';
	import { sectionFieldsForRecord } from '#lib/forms/detail-layout.js';
	import {
		copyText,
		fieldDisplay,
		intlLocale,
		type FieldDisplayContext,
		type ImageItem
	} from '#lib/forms/field-display.js';
	import { toast } from '#lib/stores/toast.js';
	import CreatorPluginLink from '#lib/components/plugins/CreatorPluginLink.svelte';
	import RecordPluginActions from '#lib/components/plugins/RecordPluginActions.svelte';
	import RecordPluginPanels from '#lib/components/plugins/RecordPluginPanels.svelte';
	import FieldValue from './FieldValue.svelte';
	import Popover from './Popover.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';

	interface Props {
		record: FormRecord | null;
		status: PanelStatus;
		errorMessage?: string;
		inView: boolean;
		form: UniversalForm;
		sections: DetailSection[];
		header: DetailHeaderFields;
		ctx: FieldDisplayContext;
		/** Rows and columns of a child table field, from the record's loaded children. */
		childTable: (field: FormField) => { rows: FormChildRecord[]; columns: FormField[] };
		/** Child records and attachments still loading in the background. */
		detailsLoading?: boolean;
		position: { index: number; total: number } | null;
		fullPageHref: string;
		recordHref: (recordId: string) => string;
		recordUrl: string;
		memberName: (userId: string | null | undefined) => string;
		canEdit: boolean;
		canDelete: boolean;
		canRestore: boolean;
		showConfigBadges: boolean;
		configBadges?: (field: FormField) => string[];
		isHiddenByCondition: (field: FormField, record: FormRecord) => boolean;
		plugins: {
			workspaceId: string;
			projectId: string;
			surface: RecordPluginActionSurface;
			enabled: boolean;
		};
		changedElsewhere?: boolean;
		width: number;
		appsOpen?: boolean;
		inert?: boolean;
		onclose: () => void;
		onprev: () => void;
		onnext: () => void;
		onedit: () => void;
		ondelete: () => void;
		onrestore: () => void;
		onretry: () => void;
		onrefresh?: () => void;
		onshowall: () => void;
		onresize: (px: number) => void;
		onopenrelation: (recordId: string, sameForm: boolean, event: MouseEvent) => void;
		onpluginapplied: () => void;
		onfocuslist: () => void;
		onannounce: (message: string) => void;
		onmodechange?: (mode: PanelMode) => void;
		/** Opens the full in-page record details (link and child record editing). */
		onmoredetails?: () => void;
		/**
		 * Hosted in another application's frame: body only. The toolbar, the resize handle and the
		 * floating and sheet modes are not rendered (the host owns header, close and navigation), and
		 * links that would move the window ask the host through `onmoredetails` instead.
		 */
		hosted?: boolean;
	}

	let {
		record,
		status,
		errorMessage = '',
		inView,
		form,
		sections,
		header,
		ctx,
		childTable,
		detailsLoading = false,
		position,
		fullPageHref,
		recordHref,
		recordUrl,
		memberName,
		canEdit,
		canDelete,
		canRestore,
		showConfigBadges,
		configBadges = () => [],
		isHiddenByCondition,
		plugins,
		changedElsewhere = false,
		width,
		appsOpen = $bindable(false),
		inert = false,
		onclose,
		onprev,
		onnext,
		onedit,
		ondelete,
		onrestore,
		onretry,
		onrefresh,
		onshowall,
		onresize,
		onopenrelation,
		onpluginapplied,
		onfocuslist,
		onannounce,
		onmodechange,
		onmoredetails,
		hosted = false
	}: Props = $props();

	const MIN_WIDTH = 360;
	const DOCK_MIN_CONTAINER = 1000;
	const SHEET_MAX_VIEWPORT = 768;

	let root = $state<HTMLElement | null>(null);
	let heading = $state<HTMLElement | null>(null);
	let containerWidth = $state(1440);
	let viewportWidth = $state(1440);
	let online = $state(true);
	let moreOpen = $state(false);
	let moreButton = $state<HTMLButtonElement | null>(null);
	let copiedKey = $state<string | null>(null);
	let preview = $state<{ items: ImageItem[]; index: number; trigger: HTMLElement | null } | null>(
		null
	);

	const mode = $derived<PanelMode>(
		hosted
			? 'docked'
			: viewportWidth < SHEET_MAX_VIEWPORT
				? 'sheet'
				: containerWidth >= DOCK_MIN_CONTAINER
					? 'docked'
					: 'overlay'
	);
	const maxWidth = $derived(Math.max(MIN_WIDTH, Math.min(720, Math.round(containerWidth * 0.6))));
	const effectiveWidth = $derived(
		hosted
			? width
			: mode === 'sheet'
				? viewportWidth
				: Math.min(Math.max(width, MIN_WIDTH), maxWidth)
	);
	const wide = $derived(effectiveWidth >= 560);
	const narrowToolbar = $derived(effectiveWidth < 400);
	const rootClass = $derived(
		hosted
			? 'hosted min-h-0 min-w-0 flex-1'
			: mode === 'sheet'
				? 'sheet fixed inset-0 z-50'
				: mode === 'overlay'
					? 'overlay fixed end-4 top-4 bottom-4 z-40 rounded-float border shadow-overlay'
					: 'docked sticky top-4 max-h-[calc(100dvh-2rem)] shrink-0 self-start rounded-card border'
	);
	const intl = $derived(intlLocale($locale ?? 'en'));
	const panelId = 'record-panel';
	const headingId = 'record-panel-title';

	$effect(() => {
		onmodechange?.(mode);
	});

	$effect(() => {
		const parent = root?.parentElement;
		if (!parent || typeof ResizeObserver === 'undefined') return;
		const observer = new ResizeObserver(() => {
			containerWidth = parent.clientWidth;
			viewportWidth = window.innerWidth;
		});
		observer.observe(parent);
		containerWidth = parent.clientWidth;
		viewportWidth = window.innerWidth;
		const onWindowResize = () => (viewportWidth = window.innerWidth);
		const onOnline = () => (online = navigator.onLine);
		window.addEventListener('resize', onWindowResize);
		window.addEventListener('online', onOnline);
		window.addEventListener('offline', onOnline);
		online = navigator.onLine;
		return () => {
			observer.disconnect();
			window.removeEventListener('resize', onWindowResize);
			window.removeEventListener('online', onOnline);
			window.removeEventListener('offline', onOnline);
		};
	});

	// Sheet mode is modal: everything outside the sheet is inert while it is open.
	let inerted: HTMLElement[] = [];
	function releaseInert() {
		for (const element of inerted) element.inert = false;
		inerted = [];
	}
	$effect(() => {
		if (mode !== 'sheet' || !root) return;
		let node: HTMLElement | null = root;
		while (node && node !== document.body) {
			const parent: HTMLElement | null = node.parentElement;
			for (const sibling of Array.from(parent?.children ?? [])) {
				if (sibling !== node && sibling instanceof HTMLElement && !sibling.inert) {
					sibling.inert = true;
					inerted.push(sibling);
				}
			}
			node = parent;
		}
		void tick().then(() => heading?.focus());
		return releaseInert;
	});
	onDestroy(releaseInert);

	export function focusHeading() {
		heading?.focus();
	}

	export function contains(node: Node | null): boolean {
		return Boolean(node && root?.contains(node));
	}

	function onKeydown(event: KeyboardEvent) {
		if (preview) return;
		if (event.key === 'Escape' && !event.defaultPrevented) {
			event.preventDefault();
			onclose();
			return;
		}
		if (event.altKey && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) {
			event.preventDefault();
			// The record's content is re-rendered; keep focus in the panel, on the new heading.
			const fromContent = !!(document.activeElement as HTMLElement | null)?.closest(
				'.record-content'
			);
			if (event.key === 'ArrowDown') onnext();
			else onprev();
			if (fromContent) void tick().then(() => heading?.focus());
			return;
		}
		if (event.key === 'F6') {
			event.preventDefault();
			onfocuslist();
			return;
		}
		if (event.key === 'Tab' && mode === 'sheet' && root) {
			const focusable = Array.from(
				root.querySelectorAll<HTMLElement>(
					'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), h2[tabindex]'
				)
			).filter((element) => element.offsetParent !== null || element === heading);
			if (focusable.length === 0) return;
			const first = focusable[0];
			const last = focusable[focusable.length - 1];
			if (event.shiftKey && document.activeElement === first) {
				event.preventDefault();
				last.focus();
			} else if (!event.shiftKey && document.activeElement === last) {
				event.preventDefault();
				first.focus();
			}
		}
	}

	// Resize (APG window splitter). The handle sits at the panel's inline start, so in a
	// left-to-right page dragging left widens the panel; in right-to-left the reverse.
	function isRtl(): boolean {
		return root ? getComputedStyle(root).direction === 'rtl' : false;
	}

	function setWidth(px: number) {
		onresize(Math.round(Math.min(Math.max(px, MIN_WIDTH), maxWidth)));
	}

	function onSeparatorKey(event: KeyboardEvent) {
		const step = event.shiftKey ? 64 : 16;
		const widen = isRtl() ? 'ArrowRight' : 'ArrowLeft';
		const narrow = isRtl() ? 'ArrowLeft' : 'ArrowRight';
		if (event.key === widen) setWidth(effectiveWidth + step);
		else if (event.key === narrow) setWidth(effectiveWidth - step);
		else if (event.key === 'Home') setWidth(MIN_WIDTH);
		else if (event.key === 'End') setWidth(maxWidth);
		else if (event.key === 'Enter') setWidth(PANEL_DEFAULT_WIDTH);
		else return;
		event.preventDefault();
	}

	function onSeparatorPointerDown(event: PointerEvent) {
		if (event.button !== 0) return;
		event.preventDefault();
		const handle = event.currentTarget as HTMLElement;
		handle.setPointerCapture(event.pointerId);
		const startX = event.clientX;
		const startWidth = effectiveWidth;
		const direction = isRtl() ? 1 : -1;
		const move = (moveEvent: PointerEvent) =>
			setWidth(startWidth + (moveEvent.clientX - startX) * direction);
		const up = () => {
			handle.removeEventListener('pointermove', move);
			handle.removeEventListener('pointerup', up);
			handle.removeEventListener('pointercancel', up);
		};
		handle.addEventListener('pointermove', move);
		handle.addEventListener('pointerup', up);
		handle.addEventListener('pointercancel', up);
	}

	// Header.
	const autonumberField = $derived(
		(() => {
			const fields = sections.flatMap((section) => section.fields);
			return fields.find((field) => field.type === 'autonumber') ?? null;
		})()
	);
	const identifier = $derived(
		record
			? (() => {
					const value = autonumberField ? record.values?.[autonumberField.key] : null;
					return typeof value === 'string' && value.trim()
						? { text: value.trim(), copy: value.trim() }
						: { text: `#${record.id.slice(0, 8)}`, copy: record.id };
				})()
			: null
	);

	function formatInstant(value: string | null | undefined): string {
		if (!value) return '';
		const date = new Date(value);
		return Number.isNaN(date.getTime())
			? value
			: new Intl.DateTimeFormat(intl, { dateStyle: 'medium', timeStyle: 'short' }).format(date);
	}

	function hasValue(field: FormField): boolean {
		return record ? fieldDisplay(field, record.values?.[field.key], ctx).kind !== 'empty' : false;
	}

	const visibleSections = $derived(
		record
			? sections
					.map((section) => ({
						section,
						fields: sectionFieldsForRecord(
							section,
							record,
							isHiddenByCondition,
							(field, value) => fieldDisplay(field, value, ctx).kind === 'empty'
						)
					}))
					.filter((entry) => entry.fields.length > 0)
			: []
	);
	const noValues = $derived(
		visibleSections.every((entry) => entry.fields.every((field) => !hasValue(field)))
	);

	async function copyValue(key: string, text: string | null) {
		if (!text) return;
		try {
			await navigator.clipboard.writeText(text);
			copiedKey = key;
			onannounce($t('forms.recordPanel.copied'));
			window.setTimeout(() => {
				if (copiedKey === key) copiedKey = null;
			}, 1500);
		} catch {
			toast.error($t('forms.recordPanel.copyFailed'));
		}
	}

	function fieldLabel(field: FormField): string {
		return field.label?.trim() || field.key.replaceAll('_', ' ');
	}

	// Image preview dialog.
	function openPreview(items: ImageItem[], index: number, trigger: HTMLElement) {
		preview = { items, index, trigger };
	}

	function closePreview() {
		const trigger = preview?.trigger;
		preview = null;
		trigger?.focus();
	}

	function onPreviewKey(event: KeyboardEvent) {
		if (!preview) return;
		if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
			event.preventDefault();
			const forward = (event.key === 'ArrowRight') !== isRtl();
			const total = preview.items.length;
			preview = { ...preview, index: (preview.index + (forward ? 1 : total - 1)) % total };
		}
	}

	// "More" menu (APG menu button).
	function menuItems(): HTMLElement[] {
		return Array.from(
			document.querySelectorAll<HTMLElement>('[data-record-panel-menu] [role="menuitem"]')
		);
	}

	function onMenuKey(event: KeyboardEvent) {
		const items = menuItems();
		const index = items.indexOf(document.activeElement as HTMLElement);
		if (event.key === 'ArrowDown') items[(index + 1) % items.length]?.focus();
		else if (event.key === 'ArrowUp') items[(index - 1 + items.length) % items.length]?.focus();
		else if (event.key === 'Home') items[0]?.focus();
		else if (event.key === 'End') items[items.length - 1]?.focus();
		else return;
		event.preventDefault();
	}

	function runMenu(action: () => void) {
		moreOpen = false;
		action();
	}

	$effect(() => {
		if (moreOpen) void tick().then(() => menuItems()[0]?.focus());
	});

	function expandApps() {
		appsOpen = true;
		void tick().then(() =>
			root?.querySelector<HTMLElement>('[data-record-panel-apps] summary')?.focus()
		);
	}

	const iconButton =
		'inline-flex h-8 w-8 items-center justify-center rounded-[var(--radius-sm)] text-fg-muted hover:bg-surface hover:text-fg disabled:cursor-not-allowed disabled:text-fg-muted';
	const menuItem =
		'flex min-h-8 w-full items-center gap-2 rounded-row px-2 py-1.5 text-start text-sm text-fg hover:bg-surface focus:bg-surface';
</script>

<!-- eslint-disable svelte/no-navigation-without-resolve -- links are full record page paths built by the caller -->

<svelte:element
	this={mode === 'sheet' ? 'div' : 'aside'}
	bind:this={root}
	id={panelId}
	role={mode === 'sheet' ? 'dialog' : undefined}
	aria-modal={mode === 'sheet' ? 'true' : undefined}
	aria-labelledby={headingId}
	aria-busy={status === 'loading' ? 'true' : undefined}
	{inert}
	tabindex="-1"
	class="record-panel flex flex-col border-line bg-raised text-fg {rootClass}"
	style={mode === 'sheet' || hosted ? undefined : `width:${effectiveWidth}px;`}
	data-record-panel={record?.id ?? ''}
	data-record-panel-mode={hosted ? 'hosted' : mode}
	data-record-panel-status={status}
	onkeydown={onKeydown}
>
	{#if mode !== 'sheet' && !hosted}
		<!-- A focusable separator is the APG window splitter widget, operated with the arrow keys. -->
		<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
		<div
			role="separator"
			aria-orientation="vertical"
			aria-controls={panelId}
			aria-label={$t('forms.recordPanel.resize')}
			aria-valuenow={effectiveWidth}
			aria-valuemin={MIN_WIDTH}
			aria-valuemax={maxWidth}
			tabindex="0"
			class="group absolute inset-y-0 -start-1 z-10 flex w-2 cursor-col-resize justify-center"
			data-record-panel-resize
			onkeydown={onSeparatorKey}
			onpointerdown={onSeparatorPointerDown}
			ondblclick={() => setWidth(PANEL_DEFAULT_WIDTH)}
		>
			<span
				class="h-full w-px bg-transparent group-hover:bg-line-strong group-focus-visible:w-0.5 group-focus-visible:bg-fg"
			></span>
		</div>
	{/if}

	{#if !hosted}
		<div
			class="flex min-h-12 flex-wrap items-center gap-1 border-b border-line px-3 py-2"
			data-record-panel-toolbar
		>
			<button
				type="button"
				class={iconButton}
				aria-label={$t('forms.recordPanel.previous')}
				title={$t('forms.recordPanel.previous')}
				disabled={!inView || !position || position.index <= 1}
				data-record-panel-prev
				onclick={onprev}><ChevronUp class="h-4 w-4" aria-hidden="true" /></button
			>
			<button
				type="button"
				class={iconButton}
				aria-label={$t('forms.recordPanel.next')}
				title={$t('forms.recordPanel.next')}
				disabled={!inView || !position || position.index >= position.total}
				data-record-panel-next
				onclick={onnext}><ChevronDown class="h-4 w-4" aria-hidden="true" /></button
			>
			{#if position && inView}
				<span class="px-1 text-xs tabular-nums text-fg-muted" data-record-panel-position
					>{$t('forms.recordPanel.position', {
						values: { index: position.index, total: position.total }
					})}</span
				>
			{/if}
			<span class="flex-1"></span>
			{#if canEdit && record && status === 'ready'}
				<button
					type="button"
					class="inline-flex h-8 items-center gap-1.5 rounded-control border border-input px-2.5 text-sm font-medium text-fg hover:bg-surface"
					data-record-panel-edit
					onclick={onedit}
				>
					<Pencil class="h-3.5 w-3.5" aria-hidden="true" />
					{$t('forms.recordPanel.edit')}
				</button>
			{/if}
			{#if !narrowToolbar}
				<a
					href={fullPageHref}
					class={iconButton}
					aria-label={$t('forms.recordPanel.openFullPage')}
					title={$t('forms.recordPanel.openFullPage')}
					data-record-panel-full-page
					><ExternalLink class="h-4 w-4 rtl:-scale-x-100" aria-hidden="true" /></a
				>
			{/if}
			<button
				type="button"
				bind:this={moreButton}
				class={iconButton}
				aria-label={$t('forms.recordPanel.more')}
				title={$t('forms.recordPanel.more')}
				aria-haspopup="menu"
				aria-expanded={moreOpen}
				aria-controls="record-panel-menu"
				data-record-panel-more
				onclick={() => (moreOpen = !moreOpen)}
				><MoreHorizontal class="h-4 w-4" aria-hidden="true" /></button
			>
			<button
				type="button"
				class={iconButton}
				aria-label={$t('forms.recordPanel.close')}
				title={$t('forms.recordPanel.close')}
				data-record-panel-close
				onclick={onclose}><X class="h-4 w-4" aria-hidden="true" /></button
			>
		</div>
	{/if}

	<Popover
		open={moreOpen}
		anchor={moreButton}
		onclose={() => (moreOpen = false)}
		role="menu"
		label={$t('forms.recordPanel.more')}
		align="end"
		width={220}
		autofocus={false}
		id="record-panel-menu"
		testId="record-panel-menu"
	>
		<div data-record-panel-menu onkeydown={onMenuKey} role="none">
			{#if narrowToolbar}
				<a
					role="menuitem"
					tabindex="-1"
					class={menuItem}
					href={fullPageHref}
					data-record-panel-full-page
					><ExternalLink class="h-4 w-4" aria-hidden="true" />{$t(
						'forms.recordPanel.openFullPage'
					)}</a
				>
			{/if}
			<button
				type="button"
				role="menuitem"
				tabindex="-1"
				class={menuItem}
				onclick={() => runMenu(() => void copyValue('link', recordUrl))}
				><Copy class="h-4 w-4" aria-hidden="true" />{$t('forms.recordPanel.copyLink')}</button
			>
			{#if record}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class={menuItem}
					onclick={() => runMenu(() => void copyValue('id', record?.id ?? null))}
					><Copy class="h-4 w-4" aria-hidden="true" />{$t('forms.recordPanel.copyId')}</button
				>
			{/if}
			{#if onmoredetails && record && status === 'ready'}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class={menuItem}
					data-record-panel-more-details
					onclick={() => runMenu(() => onmoredetails?.())}
					><span class="h-4 w-4" aria-hidden="true"></span>{$t(
						'forms.recordPanel.moreDetails'
					)}</button
				>
			{/if}
			{#if plugins.enabled && !appsOpen}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class={menuItem}
					onclick={() => runMenu(expandApps)}
					><span class="h-4 w-4" aria-hidden="true"></span>{$t(
						'forms.recordPanel.appsAction'
					)}</button
				>
			{/if}
			{#if canDelete && record && status === 'ready'}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class="{menuItem} text-danger"
					data-record-panel-delete
					onclick={() => runMenu(ondelete)}
					><Trash2 class="h-4 w-4" aria-hidden="true" />{$t('forms.recordPanel.delete')}</button
				>
			{/if}
		</div>
	</Popover>

	<div class="min-h-0 flex-1 overflow-y-auto overscroll-contain">
		{#if !online}
			<p class="border-b border-line px-4 py-1.5 text-xs text-fg-muted">
				{$t('forms.recordPanel.offline')}
			</p>
		{/if}
		{#if record && !inView && status === 'ready'}
			<div
				class="flex flex-wrap items-center gap-2 border-b border-line bg-surface px-4 py-2 text-sm text-fg"
				data-record-panel-not-in-view
			>
				<span>{$t('forms.recordPanel.notInView')}</span>
				<button
					type="button"
					class="min-h-6 font-medium underline underline-offset-2"
					onclick={onshowall}>{$t('forms.recordPanel.showInAll')}</button
				>
			</div>
		{/if}
		{#if changedElsewhere && status === 'ready'}
			<div
				class="flex items-center gap-2 border-b border-line bg-warning-soft px-4 py-2 text-sm text-warning"
			>
				<span>{$t('forms.recordPanel.updatedElsewhere')}</span>
				<button
					type="button"
					class="inline-flex min-h-6 items-center gap-1 font-medium underline underline-offset-2"
					onclick={() => onrefresh?.()}
					><RefreshCw class="h-3.5 w-3.5" aria-hidden="true" />{$t(
						'forms.recordPanel.showChanges'
					)}</button
				>
			</div>
		{/if}

		{#if status === 'loading' && !record}
			<div class="skeleton space-y-3 p-4" aria-hidden="true">
				<div class="h-5 w-2/3 rounded-control bg-surface"></div>
				<div class="h-3 w-1/3 rounded-control bg-surface"></div>
				{#each [0, 1, 2, 3, 4, 5] as row (row)}
					<div class="space-y-1.5 pt-2">
						<div class="h-2.5 w-20 rounded-control bg-surface"></div>
						<div class="h-3.5 w-1/2 rounded-control bg-surface"></div>
					</div>
				{/each}
			</div>
			<h2 id={headingId} tabindex="-1" bind:this={heading} class="sr-only">
				{$t('forms.recordPanel.label')}
			</h2>
		{:else if status === 'error'}
			<div class="space-y-3 p-4" role="alert">
				<h2
					id={headingId}
					tabindex="-1"
					bind:this={heading}
					class="flex items-center gap-2 text-base font-semibold"
				>
					<AlertTriangle class="h-4 w-4 text-warning" aria-hidden="true" />
					{$t('forms.recordPanel.loadError')}
				</h2>
				{#if errorMessage}
					<p class="text-sm text-fg-muted">{errorMessage}</p>
				{/if}
				<button
					type="button"
					class="inline-flex h-8 items-center rounded-control border border-input px-3 text-sm font-medium hover:bg-surface"
					onclick={onretry}>{$t('forms.recordPanel.retry')}</button
				>
			</div>
		{:else if status === 'forbidden'}
			<div class="space-y-2 p-4">
				<h2
					id={headingId}
					tabindex="-1"
					bind:this={heading}
					class="flex items-center gap-2 text-base font-semibold"
				>
					<Lock class="h-4 w-4 text-fg-muted" aria-hidden="true" />
					{$t('forms.recordPanel.forbidden')}
				</h2>
			</div>
		{:else if status === 'missing'}
			<div class="space-y-3 p-4">
				<h2 id={headingId} tabindex="-1" bind:this={heading} class="text-base font-semibold">
					{$t('forms.recordPanel.deleted')}
				</h2>
				{#if canRestore}
					<button
						type="button"
						class="inline-flex h-8 items-center rounded-control border border-input px-3 text-sm font-medium hover:bg-surface"
						onclick={onrestore}>{$t('forms.recordPanel.restore')}</button
					>
				{/if}
			</div>
		{:else if record}
			{#key record.id}
				<div class="record-content">
					<header class="space-y-1.5 px-4 pt-4 pb-3">
						<h2
							id={headingId}
							tabindex="-1"
							bind:this={heading}
							class="text-base font-semibold leading-snug break-words"
							style="display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;overflow:hidden;"
							title={record.title}
						>
							{record.title}
						</h2>
						<div class="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
							{#if identifier}
								<span
									class="font-mono [unicode-bidi:isolate]"
									dir="ltr"
									data-record-panel-identifier>{identifier.text}</span
								>
								<button
									type="button"
									class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-surface"
									aria-label={$t('forms.recordPanel.copyIdentifier')}
									title={$t('forms.recordPanel.copyIdentifier')}
									onclick={() => void copyValue('identifier', identifier.copy)}
								>
									{#if copiedKey === 'identifier'}<Check
											class="h-3.5 w-3.5"
											aria-hidden="true"
										/>{:else}<Copy class="h-3.5 w-3.5" aria-hidden="true" />{/if}
								</button>
							{/if}
							{#if header.badge}
								<FieldValue
									field={header.badge}
									value={record.values?.[header.badge.key]}
									variant="cell"
									{ctx}
								/>
							{/if}
						</div>
						<p class="text-xs text-fg-muted" data-record-panel-subtitle>
							{#if header.subtitle && hasValue(header.subtitle)}
								<FieldValue
									field={header.subtitle}
									value={record.values?.[header.subtitle.key]}
									variant="cell"
									{ctx}
								/>
							{:else if record.updated_by}
								{$t('forms.recordPanel.updatedBy', {
									values: {
										date: formatInstant(record.updated_at),
										member: memberName(record.updated_by)
									}
								})}
							{:else}
								{$t('forms.recordPanel.updatedOn', {
									values: { date: formatInstant(record.updated_at) }
								})}
							{/if}
						</p>
					</header>

					{#if header.highlights.length > 0}
						<dl
							class="grid gap-x-4 gap-y-3 border-y border-line bg-surface px-4 py-3 {wide
								? 'grid-cols-2'
								: 'grid-cols-1'}"
							data-record-panel-highlights
						>
							{#each header.highlights as field (field.key)}
								<div class="min-w-0">
									<dt class="text-xs font-medium uppercase text-fg-muted">
										{fieldLabel(field)}
									</dt>
									<dd class="mt-0.5 text-base font-semibold">
										<FieldValue
											{field}
											value={record.values?.[field.key]}
											variant="panel"
											{ctx}
											{recordHref}
											{onopenrelation}
											onpreviewimage={openPreview}
										/>
									</dd>
								</div>
							{/each}
						</dl>
					{/if}

					{#if noValues}
						<div class="space-y-2 px-4 py-4 text-sm text-fg-muted">
							<p>{$t('forms.recordPanel.noValues')}</p>
							{#if canEdit}
								<button
									type="button"
									class="inline-flex h-8 items-center rounded-control rf-filled bg-primary px-3 text-sm font-medium text-on-primary"
									onclick={onedit}>{$t('forms.recordPanel.edit')}</button
								>
							{/if}
						</div>
					{:else}
						{#each visibleSections as entry (entry.section.key)}
							<details
								class="group/section border-b border-line"
								open={!entry.section.collapsible || !entry.section.defaultCollapsed}
								data-record-panel-section={`${record.id}:${entry.section.key}`}
								ontoggle={(event) => {
									const details = event.currentTarget as HTMLDetailsElement;
									if (!entry.section.collapsible && !details.open) details.open = true;
								}}
							>
								<summary
									class="flex min-h-10 list-none items-center gap-1.5 px-4 py-2 text-xs font-semibold tracking-wide text-fg-muted uppercase marker:hidden focus-visible:-outline-offset-2 {entry
										.section.collapsible
										? 'cursor-pointer'
										: 'cursor-default'}"
								>
									{#if entry.section.collapsible}
										<ChevronRight
											class="h-3.5 w-3.5 transition-transform group-open/section:rotate-90 rtl:-scale-x-100"
											aria-hidden="true"
										/>
									{/if}
									{entry.section.title}
								</summary>
								<dl class="space-y-1 px-4 pb-3">
									{#each entry.fields as field (field.key)}
										{@const display = fieldDisplay(field, record.values?.[field.key], ctx)}
										{@const copy = copyText(display)}
										{@const badges = showConfigBadges ? configBadges(field) : []}
										<div
											class="group/row relative grid min-h-10 items-start gap-x-4 gap-y-0.5 py-1.5 {wide
												? 'grid-cols-[160px_minmax(0,1fr)_auto]'
												: 'grid-cols-[minmax(0,1fr)_auto]'}"
											data-record-panel-field={`${record.id}:${field.key}`}
											data-field-type={field.type ?? 'text'}
										>
											<dt
												class="pt-0.5 text-xs font-medium text-fg-muted uppercase break-words {wide
													? ''
													: 'col-start-1 row-start-1'}"
											>
												{fieldLabel(field)}
												{#if field.type === 'ai_summary'}
													<span
														class="ms-1 rounded-control bg-surface px-1 text-[10px] font-semibold text-fg normal-case"
														aria-label={$t('forms.fieldValue.aiGenerated')}
														title={$t('forms.fieldValue.aiGenerated')}
														>{$t('forms.fieldValue.aiLabel')}</span
													>
												{:else if field.type === 'formula'}
													<span
														class="ms-1 font-serif text-fg-muted normal-case italic"
														aria-label={$t('forms.fieldValue.calculated')}
														title={$t('forms.fieldValue.calculated')}>ƒ</span
													>
												{/if}
												{#each badges as badge (badge)}
													<span
														class="ms-1 rounded-control bg-surface px-1 text-[10px] text-fg-muted normal-case"
														>{badge}</span
													>
												{/each}
											</dt>
											<dd class="min-w-0 text-sm text-fg {wide ? '' : 'col-span-2'}">
												{#if field.type === 'child_table'}
													{@const table = childTable(field)}
													{#if detailsLoading && table.rows.length === 0}
														<div
															class="skeleton h-4 w-32 rounded-control bg-surface"
															aria-hidden="true"
														></div>
													{:else if table.rows.length === 0}
														<FieldValue
															{field}
															value={record.values?.[field.key]}
															variant="panel"
															{ctx}
														/>
													{:else}
														<div class="overflow-x-auto rounded-card border border-line">
															<table
																class="w-full text-xs"
																data-record-panel-child-table={field.key}
															>
																<thead class="bg-nav text-fg-muted">
																	<tr>
																		<th class="px-2 py-1.5 text-start font-medium"
																			>{$t('forms.views.titleField')}</th
																		>
																		{#each table.columns.slice(0, 3) as column (column.key)}
																			<th class="px-2 py-1.5 text-start font-medium"
																				>{fieldLabel(column)}</th
																			>
																		{/each}
																	</tr>
																</thead>
																<tbody class="divide-y divide-line">
																	{#each table.rows.slice(0, 5) as child (child.record.id)}
																		<tr>
																			<td class="max-w-40 truncate px-2 py-1.5"
																				>{child.record.title}</td
																			>
																			{#each table.columns.slice(0, 3) as column (column.key)}
																				<td class="max-w-32 px-2 py-1.5">
																					<FieldValue
																						field={column}
																						value={child.record.values?.[column.key]}
																						variant="cell"
																						{ctx}
																					/>
																				</td>
																			{/each}
																		</tr>
																	{/each}
																</tbody>
															</table>
														</div>
														{#if table.rows.length > 5 && hosted}
															{#if onmoredetails}
																<button
																	type="button"
																	class="mt-1 inline-block text-xs text-fg hover:underline"
																	onclick={onmoredetails}
																	>{$t('forms.fieldValue.viewAllChildren', {
																		values: { count: table.rows.length }
																	})}</button
																>
															{/if}
														{:else if table.rows.length > 5}
															<a
																href={fullPageHref}
																class="mt-1 inline-block text-xs text-fg hover:underline"
																>{$t('forms.fieldValue.viewAllChildren', {
																	values: { count: table.rows.length }
																})}</a
															>
														{/if}
													{/if}
												{:else}
													<FieldValue
														{field}
														value={record.values?.[field.key]}
														variant="panel"
														{ctx}
														{recordHref}
														{onopenrelation}
														onpreviewimage={openPreview}
													/>
												{/if}
											</dd>
											{#if copy}
												<button
													type="button"
													class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted opacity-0 group-hover/row:opacity-100 group-focus-within/row:opacity-100 hover:bg-surface focus:opacity-100 {wide
														? ''
														: 'col-start-2 row-start-1'}"
													aria-label={$t('forms.recordPanel.copyField', {
														values: { field: fieldLabel(field) }
													})}
													title={$t('forms.recordPanel.copyField', {
														values: { field: fieldLabel(field) }
													})}
													onclick={() => void copyValue(field.key, copy)}
												>
													{#if copiedKey === field.key}<Check
															class="h-3.5 w-3.5"
															aria-hidden="true"
														/>{:else}<Copy class="h-3.5 w-3.5" aria-hidden="true" />{/if}
												</button>
											{/if}
										</div>
									{/each}
								</dl>
							</details>
						{/each}
					{/if}

					{#if plugins.enabled}
						<details
							class="border-b border-line"
							open={appsOpen}
							data-record-panel-apps
							ontoggle={(event) => (appsOpen = (event.currentTarget as HTMLDetailsElement).open)}
						>
							<summary
								class="flex min-h-10 cursor-pointer list-none items-center gap-1.5 px-4 py-2 text-xs font-semibold tracking-wide text-fg-muted uppercase marker:hidden focus-visible:-outline-offset-2"
							>
								<ChevronRight
									class="h-3.5 w-3.5 transition-transform {appsOpen
										? 'rotate-90'
										: ''} rtl:-scale-x-100"
									aria-hidden="true"
								/>
								{$t('forms.recordPanel.apps')}
							</summary>
							{#if appsOpen}
								<div class="space-y-3 px-4 pb-4">
									{#if form.key === 'order_line'}
										<CreatorPluginLink
											workspaceId={plugins.workspaceId}
											projectId={plugins.projectId}
											formKey={form.key}
											slot="form.record.actions"
										/>
									{/if}
									<RecordPluginActions recordId={record.id} surface={plugins.surface} />
									{#if form.key === 'order_line'}
										<CreatorPluginLink
											workspaceId={plugins.workspaceId}
											projectId={plugins.projectId}
											formKey={form.key}
											slot="form.record.detail"
										/>
									{/if}
									<RecordPluginPanels
										recordId={record.id}
										surface={plugins.surface}
										onApplied={onpluginapplied}
									/>
								</div>
							{/if}
						</details>
					{/if}

					<details class="border-b border-line" data-record-panel-details>
						<summary
							class="flex min-h-10 cursor-pointer list-none items-center gap-1.5 px-4 py-2 text-xs font-semibold tracking-wide text-fg-muted uppercase marker:hidden focus-visible:-outline-offset-2"
						>
							<ChevronRight
								class="details-chevron h-3.5 w-3.5 transition-transform rtl:-scale-x-100"
								aria-hidden="true"
							/>
							{$t('forms.recordPanel.details')}
						</summary>
						<dl
							class="grid grid-cols-[minmax(0,auto)_minmax(0,1fr)] gap-x-4 gap-y-2 px-4 pb-4 text-sm"
						>
							<dt class="text-xs text-fg-muted uppercase">
								{$t('forms.recordPanel.created')}
							</dt>
							<dd>{formatInstant(record.created_at)}</dd>
							<dt class="text-xs text-fg-muted uppercase">
								{$t('forms.recordPanel.createdBy')}
							</dt>
							<dd>{record.created_by ? memberName(record.created_by) : '—'}</dd>
							<dt class="text-xs text-fg-muted uppercase">
								{$t('forms.recordPanel.updated')}
							</dt>
							<dd>{formatInstant(record.updated_at)}</dd>
							<dt class="text-xs text-fg-muted uppercase">
								{$t('forms.recordPanel.updatedByLabel')}
							</dt>
							<dd>{record.updated_by ? memberName(record.updated_by) : '—'}</dd>
							<dt class="text-xs text-fg-muted uppercase">
								{$t('forms.recordPanel.recordId')}
							</dt>
							<dd class="flex min-w-0 items-center gap-1">
								<span class="truncate font-mono text-xs" dir="ltr">{record.id}</span>
								<button
									type="button"
									class="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-control text-fg-muted hover:bg-surface"
									aria-label={$t('forms.recordPanel.copyId')}
									onclick={() => void copyValue('record-id', record?.id ?? null)}
								>
									{#if copiedKey === 'record-id'}<Check
											class="h-3.5 w-3.5"
											aria-hidden="true"
										/>{:else}<Copy class="h-3.5 w-3.5" aria-hidden="true" />{/if}
								</button>
							</dd>
						</dl>
					</details>
				</div>
			{/key}
		{/if}
	</div>
</svelte:element>

{#if preview}
	{@const item = preview.items[preview.index]}
	<Dialog
		open
		headless
		size="wide"
		title={$t('forms.recordPanel.previewTitle')}
		onclose={closePreview}
	>
		<!-- Escape, the focus trap and focus return belong to the dialog; the arrows page through the images. -->
		<div
			class="relative flex max-h-full max-w-full flex-col items-center gap-3 p-3"
			data-record-panel-preview
			role="presentation"
			onkeydown={onPreviewKey}
		>
			<img
				src={item.full ?? item.src}
				alt={item.alt}
				class="max-h-[80dvh] max-w-full rounded-control bg-raised object-contain"
			/>
			<div class="flex items-center gap-2 text-sm text-fg">
				{#if preview.items.length > 1}
					<button
						type="button"
						class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line bg-surface hover:bg-selected"
						aria-label={$t('forms.recordPanel.previewPrevious')}
						onclick={() =>
							preview &&
							(preview = {
								...preview,
								index: (preview.index - 1 + preview.items.length) % preview.items.length
							})}><ChevronLeft class="h-4 w-4 rtl:-scale-x-100" aria-hidden="true" /></button
					>
					<span class="tabular-nums"
						>{$t('forms.recordPanel.previewPosition', {
							values: { index: preview.index + 1, total: preview.items.length }
						})}</span
					>
					<button
						type="button"
						class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line bg-surface hover:bg-selected"
						aria-label={$t('forms.recordPanel.previewNext')}
						onclick={() =>
							preview &&
							(preview = { ...preview, index: (preview.index + 1) % preview.items.length })}
						><ChevronRight class="h-4 w-4 rtl:-scale-x-100" aria-hidden="true" /></button
					>
				{/if}
				<button
					type="button"
					class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line bg-surface hover:bg-selected"
					aria-label={$t('forms.recordPanel.close')}
					onclick={closePreview}><X class="h-4 w-4" aria-hidden="true" /></button
				>
			</div>
		</div>
	</Dialog>
{/if}

<style>
	.record-panel.docked,
	.record-panel.overlay {
		animation: panel-in 180ms cubic-bezier(0.2, 0, 0, 1);
	}

	.record-panel.sheet {
		animation: sheet-in 220ms cubic-bezier(0.2, 0, 0, 1);
	}

	.record-content {
		animation: content-in 100ms linear;
	}

	details[open] > summary :global(.details-chevron) {
		transform: rotate(90deg);
	}

	@keyframes panel-in {
		from {
			opacity: 0;
			transform: translateX(16px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	:global([dir='rtl']) .record-panel.docked,
	:global([dir='rtl']) .record-panel.overlay {
		animation-name: panel-in-rtl;
	}

	@keyframes panel-in-rtl {
		from {
			opacity: 0;
			transform: translateX(-16px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	@keyframes sheet-in {
		from {
			opacity: 0;
			transform: translateY(24px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	@keyframes content-in {
		from {
			opacity: 0.6;
		}
		to {
			opacity: 1;
		}
	}

	.skeleton {
		opacity: 0;
		animation: skeleton-show 0ms linear 200ms forwards;
	}

	@keyframes skeleton-show {
		to {
			opacity: 1;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.record-panel.docked,
		.record-panel.overlay,
		.record-panel.sheet,
		:global([dir='rtl']) .record-panel.docked,
		:global([dir='rtl']) .record-panel.overlay {
			animation: fade-in 80ms linear;
		}

		.record-content {
			animation: none;
		}

		@keyframes fade-in {
			from {
				opacity: 0;
			}
			to {
				opacity: 1;
			}
		}
	}
</style>
