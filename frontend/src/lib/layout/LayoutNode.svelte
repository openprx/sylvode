<svelte:options runes />

<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import Metric from '#lib/ui/metric/Metric.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { renderMarkdown } from '#lib/utils/markdown.js';
	import { useRuntime } from './context';
	import { formatValue } from './format';
	import { displayValue, resolveText, resolveValue } from './refs';
	import { evaluateWhen } from './when';
	import LayoutNode from './LayoutNode.svelte';
	import LayoutForm from './LayoutForm.svelte';
	import type { LayoutNode as NodeSpec } from './types';

	let { node }: { node: NodeSpec } = $props();

	const runtime = useRuntime();
	const cfg = $derived(node.props ?? {});
	const loc = $derived($locale ?? 'en');
	const children = $derived(node.children ?? []);
	const text = (value: unknown) => resolveText(value, runtime.scope, loc);
	const value = (raw: unknown) => resolveValue(raw, runtime.scope);

	const GAP: Record<string, string> = {
		none: 'gap-0',
		xs: 'gap-1',
		sm: 'gap-2',
		md: 'gap-3',
		lg: 'gap-4'
	};
	const ALIGN: Record<string, string> = {
		start: 'items-start',
		center: 'items-center',
		end: 'items-end',
		stretch: 'items-stretch'
	};
	const COLUMNS: Record<number, string> = {
		1: 'grid-cols-1',
		2: 'grid-cols-1 sm:grid-cols-2',
		3: 'grid-cols-1 sm:grid-cols-2 lg:grid-cols-3',
		4: 'grid-cols-1 sm:grid-cols-2 lg:grid-cols-4'
	};
	/** Static class names so Tailwind sees them; the 2-column breakpoint clamps a wider span to 2. */
	const COL_SPAN: Record<number, string> = {
		2: 'sm:col-span-2',
		3: 'sm:col-span-2 lg:col-span-3',
		4: 'sm:col-span-2 lg:col-span-4'
	};
	const spanClass = (child: NodeSpec) => COL_SPAN[Number(child.col_span)] ?? '';
	const TEXT_STYLE: Record<string, string> = {
		body: 'text-sm text-fg',
		muted: 'text-sm text-fg-muted',
		heading: 'text-base font-semibold text-fg',
		caption: 'text-xs text-fg-muted'
	};
	const TONES = new Set(['success', 'warning', 'danger']);

	const gap = $derived(GAP[String(cfg.gap)] ?? GAP.md);
	const columns = $derived(COLUMNS[Number(cfg.columns)] ?? COLUMNS[2]);

	let collapsed = $state(false);
	let activeTab = $state(0);
	const tabs = $derived(
		Array.isArray(cfg.tabs) ? (cfg.tabs as { key?: string; label?: unknown }[]) : []
	);

	const badgeTone = $derived.by(() => {
		const shown = displayValue(value(cfg.value));
		const tone = cfg.tone;
		const picked =
			tone && typeof tone === 'object' && 'map' in tone
				? (tone as { map: Record<string, string> }).map[shown]
				: tone;
		return typeof picked === 'string' && TONES.has(picked)
			? (picked as 'success' | 'warning' | 'danger')
			: 'neutral';
	});

	const rows = $derived.by(() => {
		const resolved = value(cfg.rows);
		return Array.isArray(resolved) ? (resolved as Record<string, unknown>[]) : [];
	});
	const columnsSpec = $derived(
		Array.isArray(cfg.columns)
			? (cfg.columns as { field: string; label?: unknown; format?: unknown }[]).slice(0, 12)
			: []
	);

	function cell(row: Record<string, unknown>, field: string): unknown {
		const values = row.values;
		if (values && typeof values === 'object' && field in (values as object)) {
			return (values as Record<string, unknown>)[field];
		}
		return row[field];
	}

	const fieldRecord = $derived(value(cfg.record) as Record<string, unknown> | undefined);
	const fieldKey = $derived(typeof cfg.field === 'string' ? cfg.field : '');
	const fieldDef = $derived.by(() => {
		const list = fieldRecord?.fields;
		return Array.isArray(list)
			? (list as { key: string; label?: string }[]).find((item) => item.key === fieldKey)
			: undefined;
	});

	const unavailable = $derived(runtime.unavailable(node.on_click));
	const buttonVariant = $derived(
		cfg.variant === 'primary' || cfg.variant === 'danger'
			? (cfg.variant as 'primary' | 'danger')
			: cfg.variant === 'ghost'
				? 'text'
				: 'secondary'
	);
	const running = $derived(
		node.on_click ? runtime.actions[node.on_click]?.status === 'running' : false
	);
	const trend = $derived(cfg.trend === undefined ? '' : displayValue(value(cfg.trend)));
</script>

{#if evaluateWhen(node.when, runtime.scope)}
	{#if node.type === 'stack'}
		<div
			class="flex min-w-0 {cfg.direction === 'horizontal'
				? 'flex-row flex-wrap'
				: 'flex-col'} {gap} {ALIGN[String(cfg.align)] ?? ''}"
			data-layout-node="stack"
		>
			{#each children as child, index (index)}<LayoutNode node={child} />{/each}
		</div>
	{:else if node.type === 'grid'}
		<div class="grid min-w-0 {columns} {gap}" data-layout-node="grid">
			{#each children as child, index (index)}
				{#if spanClass(child)}
					<div class="min-w-0 empty:hidden {spanClass(child)}" data-layout-span={child.col_span}>
						<LayoutNode node={child} />
					</div>
				{:else}
					<LayoutNode node={child} />
				{/if}
			{/each}
		</div>
	{:else if node.type === 'card'}
		<section class="rounded-card border border-line bg-raised p-4" data-layout-node="card">
			{#if cfg.title || cfg.subtitle}
				<header class="mb-3 flex items-start justify-between gap-2">
					<div class="min-w-0">
						{#if cfg.title}
							<h3 class="truncate text-sm font-semibold text-fg">{text(cfg.title)}</h3>
						{/if}
						{#if cfg.subtitle}
							<p class="text-xs text-fg-muted">{text(cfg.subtitle)}</p>
						{/if}
					</div>
					{#if cfg.collapsible === true}
						<button
							type="button"
							class="text-xs text-fg-muted hover:text-fg"
							aria-expanded={!collapsed}
							onclick={() => (collapsed = !collapsed)}
						>
							{collapsed ? $t('layout.expand') : $t('layout.collapse')}
						</button>
					{/if}
				</header>
			{/if}
			{#if !collapsed}
				<div class="flex min-w-0 flex-col gap-3">
					{#each children as child, index (index)}<LayoutNode node={child} />{/each}
				</div>
			{/if}
		</section>
	{:else if node.type === 'tabs'}
		<div class="min-w-0" data-layout-node="tabs">
			<div class="mb-3 flex gap-1 border-b border-line" role="tablist">
				{#each tabs as tab, index (index)}
					<button
						type="button"
						role="tab"
						aria-selected={activeTab === index}
						class="px-3 py-2 text-sm {activeTab === index
							? 'border-b-2 border-primary font-medium text-fg'
							: 'text-fg-muted hover:text-fg'}"
						onclick={() => (activeTab = index)}
					>
						{text(tab.label) || tab.key}
					</button>
				{/each}
			</div>
			{#if children[activeTab]}
				<div role="tabpanel"><LayoutNode node={children[activeTab]} /></div>
			{/if}
		</div>
	{:else if node.type === 'text'}
		<p
			class="min-w-0 break-words {TEXT_STYLE[String(cfg.style)] ?? TEXT_STYLE.body}"
			data-layout-node="text"
		>
			{text(cfg.value)}
		</p>
	{:else if node.type === 'markdown'}
		<div class="min-w-0 break-words text-sm text-fg" data-layout-node="markdown">
			<!-- eslint-disable-next-line svelte/no-at-html-tags -- renderMarkdown escapes all HTML and only emits http(s) and relative links -->
			{@html renderMarkdown(text(cfg.value))}
		</div>
	{:else if node.type === 'stat'}
		<div data-layout-node="stat">
			<Metric
				label={text(cfg.label)}
				value={formatValue(value(cfg.value), cfg.format, loc)}
				note={trend}
			/>
		</div>
	{:else if node.type === 'badge'}
		<StatusChip tone={badgeTone} data-layout-node="badge">
			{displayValue(value(cfg.value))}
		</StatusChip>
	{:else if node.type === 'table'}
		<div class="min-w-0 overflow-x-auto" data-layout-node="table">
			{#if rows.length === 0}
				<p class="text-sm text-fg-muted">{cfg.empty ? text(cfg.empty) : $t('layout.noRows')}</p>
			{:else}
				<table class="w-full text-left text-sm">
					<thead>
						<tr class="border-b border-line text-xs text-fg-muted">
							{#each columnsSpec as column, index (index)}
								<th class="px-2 py-1 font-medium"
									>{column.label ? text(column.label) : column.field}</th
								>
							{/each}
						</tr>
					</thead>
					<tbody>
						{#each rows as row, rowIndex (rowIndex)}
							<tr
								class="border-b border-line last:border-0 {typeof cfg.row_click === 'string'
									? 'cursor-pointer hover:bg-surface'
									: ''}"
								onclick={() => typeof cfg.row_click === 'string' && void runtime.run(cfg.row_click)}
							>
								{#each columnsSpec as column, index (index)}
									<td class="px-2 py-1 text-fg">
										{formatValue(cell(row, column.field), column.format, loc)}
									</td>
								{/each}
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</div>
	{:else if node.type === 'field-display'}
		<div class="min-w-0 text-sm" data-layout-node="field-display">
			{#if cfg.label !== false}
				<span class="text-xs text-fg-muted">{fieldDef?.label ?? fieldKey}</span>
			{/if}
			<div class="break-words text-fg">
				{displayValue((fieldRecord?.values as Record<string, unknown> | undefined)?.[fieldKey])}
			</div>
		</div>
	{:else if node.type === 'form'}
		<LayoutForm props={cfg} />
	{:else if node.type === 'button'}
		<span title={unavailable ?? undefined} data-layout-node="button">
			<Button
				variant={buttonVariant}
				disabled={unavailable !== null}
				loading={running}
				onclick={() => node.on_click && void runtime.run(node.on_click)}
			>
				{text(cfg.label)}
			</Button>
		</span>
	{:else}
		<div
			class="rounded-control border border-dashed border-line px-3 py-2 text-xs text-fg-muted"
			data-layout-placeholder={node.type}
		>
			{$t('layout.unknownNode', { values: { type: node.type } })}
		</div>
	{/if}
{/if}
