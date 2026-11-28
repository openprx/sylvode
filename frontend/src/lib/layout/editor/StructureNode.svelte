<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import ArrowLeftRight from '@lucide/svelte/icons/arrow-left-right';
	import MoveHorizontal from '@lucide/svelte/icons/move-horizontal';
	import GripVertical from '@lucide/svelte/icons/grip-vertical';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import StructureNode from './StructureNode.svelte';
	import type { LayoutNode } from '../types';
	import {
		GRID_MAX_COLUMNS,
		arrowTarget,
		colSpanOf,
		columnsOf,
		snapColumns,
		spanFromDrag,
		takesChildren,
		type StructureCtrl
	} from './ops';

	let {
		node,
		path,
		parent = null,
		index = 0,
		siblings = 1,
		parentColumns = null,
		ctrl
	}: {
		node: LayoutNode;
		path: string;
		parent?: LayoutNode | null;
		index?: number;
		siblings?: number;
		parentColumns?: number | null;
		ctrl: StructureCtrl;
	} = $props();

	const children = $derived(node.children ?? []);
	const isGrid = $derived(node.type === 'grid');
	const columns = $derived(columnsOf(node));
	const errors = $derived(ctrl.errorsAt(path));
	const palette = $derived(ctrl.palette(path));
	const parentPath = $derived(path.slice(0, path.lastIndexOf('/children/')));
	const movable = $derived(parent !== null && takesChildren(parent));
	const drag = $derived(ctrl.dragging());
	const isDragged = $derived(!!drag && drag.parent === parentPath && drag.from === index);
	const isTarget = $derived(
		!!drag && drag.parent === parentPath && drag.to === index && drag.from !== index
	);

	const inGrid = $derived(parent?.type === 'grid' && parentColumns !== null);
	const span = $derived(Math.min(colSpanOf(node), parentColumns ?? 1));
	let previewSpan = $state<number | null>(null);
	let spanDrag: { x: number; start: number; columnWidth: number } | null = null;
	const shownSpan = $derived(previewSpan ?? span);

	let rulerEl: HTMLElement | undefined = $state();
	let previewColumns = $state<number | null>(null);
	const shownColumns = $derived(previewColumns ?? columns);

	function textOf(value: unknown): string {
		if (typeof value === 'string') return value;
		if (value && typeof value === 'object' && 'en' in value && typeof value.en === 'string') {
			return value.en;
		}
		return '';
	}

	const summary = $derived.by(() => {
		const props = node.props ?? {};
		return textOf(props.title) || textOf(props.label) || textOf(props.value) || '';
	});

	function gripKey(event: KeyboardEvent) {
		if (!movable) return;
		if (event.key === 'Delete' || event.key === 'Backspace') {
			event.preventDefault();
			ctrl.remove(path);
			return;
		}
		if (event.metaKey || event.ctrlKey || event.altKey) return;
		if (event.shiftKey && inGrid && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
			event.preventDefault();
			ctrl.setColSpan(path, span + (event.key === 'ArrowRight' ? 1 : -1));
			return;
		}
		const target = arrowTarget(event.key, index, siblings, parentColumns);
		if (event.key.startsWith('Arrow')) event.preventDefault();
		if (target !== null) ctrl.move(parentPath, index, target);
	}

	function gripDown(event: PointerEvent) {
		if (!movable || event.button !== 0) return;
		const list = (event.currentTarget as HTMLElement).closest('[data-snode-list]');
		if (list instanceof HTMLElement) ctrl.beginDrag(parentPath, index, list, event);
	}

	function spanDown(event: PointerEvent) {
		if (event.button !== 0 || !inGrid || !parentColumns) return;
		const list = (event.currentTarget as HTMLElement).closest('[data-snode-list]');
		if (!(list instanceof HTMLElement)) return;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		spanDrag = { x: event.clientX, start: span, columnWidth: list.clientWidth / parentColumns };
		previewSpan = span;
	}

	function spanAt(event: PointerEvent): number | null {
		if (!spanDrag || !parentColumns) return null;
		return spanFromDrag(
			spanDrag.start,
			event.clientX - spanDrag.x,
			spanDrag.columnWidth,
			parentColumns
		);
	}

	function spanMove(event: PointerEvent) {
		if (previewSpan === null) return;
		previewSpan = spanAt(event) ?? previewSpan;
	}

	function spanUp(event: PointerEvent) {
		if (previewSpan === null) return;
		const next = spanAt(event) ?? previewSpan;
		previewSpan = null;
		spanDrag = null;
		ctrl.setColSpan(path, next);
	}

	function spanKey(event: KeyboardEvent) {
		if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
		event.preventDefault();
		ctrl.setColSpan(path, span + (event.key === 'ArrowRight' ? 1 : -1));
	}

	function rulerAt(event: PointerEvent): number | null {
		if (!rulerEl) return null;
		const box = rulerEl.getBoundingClientRect();
		return snapColumns(event.clientX - box.left, box.width);
	}

	function rulerDown(event: PointerEvent) {
		if (event.button !== 0) return;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		previewColumns = rulerAt(event);
	}

	function rulerMove(event: PointerEvent) {
		if (previewColumns === null) return;
		previewColumns = rulerAt(event);
	}

	function rulerUp(event: PointerEvent) {
		if (previewColumns === null) return;
		const next = rulerAt(event) ?? previewColumns;
		previewColumns = null;
		ctrl.setColumns(path, next);
	}

	function rulerKey(event: KeyboardEvent) {
		const step = event.key === 'ArrowRight' || event.key === 'ArrowUp' ? 1 : -1;
		if (!event.key.startsWith('Arrow')) return;
		event.preventDefault();
		ctrl.setColumns(path, columns + step);
	}
</script>

<div
	class="min-w-0 rounded-control border bg-surface-white p-2 {errors.length > 0
		? 'border-danger'
		: 'border-line'} {isDragged ? 'opacity-50' : ''} {isTarget ? 'outline-dashed outline-2' : ''}"
	style={inGrid ? `grid-column: span ${shownSpan} / span ${shownSpan}` : undefined}
	data-snode={path}
	data-snode-type={node.type}
>
	<div class="flex min-w-0 items-center gap-2">
		{#if movable}
			<button
				type="button"
				class="shrink-0 cursor-grab touch-none rounded-control p-1 text-fg-muted hover:text-fg"
				data-grip={path}
				aria-label={$t('layout.editor.structure.grip', { values: { type: node.type } })}
				aria-describedby="structure-help"
				onpointerdown={gripDown}
				onkeydown={gripKey}
			>
				<GripVertical size={16} aria-hidden="true" />
			</button>
		{/if}
		<span class="shrink-0 font-mono text-xs text-fg">{node.type}</span>
		{#if summary}
			<span class="min-w-0 truncate text-xs text-fg-muted">{summary}</span>
		{/if}
		{#if inGrid && parentColumns && parentColumns > 1}
			<div
				class="ml-auto flex shrink-0 items-center gap-1 text-xs text-fg-muted"
				data-span-control={path}
			>
				<span class="font-mono text-fg">{shownSpan}/{parentColumns}</span>
				<div
					class="cursor-ew-resize touch-none rounded-control border border-line px-1 hover:bg-selected"
					role="slider"
					tabindex="0"
					aria-label={$t('layout.editor.structure.spanLabel', { values: { type: node.type } })}
					aria-valuemin={1}
					aria-valuemax={parentColumns}
					aria-valuenow={shownSpan}
					data-span-handle={path}
					onpointerdown={spanDown}
					onpointermove={spanMove}
					onpointerup={spanUp}
					onkeydown={spanKey}
				>
					<MoveHorizontal size={14} aria-hidden="true" />
				</div>
			</div>
		{/if}
		{#if movable}
			<button
				type="button"
				class="{inGrid && parentColumns && parentColumns > 1
					? ''
					: 'ml-auto'} shrink-0 rounded-control p-1 text-fg-muted hover:text-danger"
				aria-label={$t('layout.editor.structure.remove', { values: { type: node.type } })}
				onclick={() => ctrl.remove(path)}
			>
				<Trash2 size={16} aria-hidden="true" />
			</button>
		{/if}
	</div>

	{#if errors.length > 0}
		<ul class="mt-1 text-xs text-danger" role="alert" data-snode-errors={path}>
			{#each errors as error, i (i)}
				<li class="break-all font-mono">
					{error.path.slice(path.length) || '/'}
					{error.code}{error.message ? `  ${error.message}` : ''}
				</li>
			{/each}
		</ul>
	{/if}

	{#if isGrid}
		<div class="mt-2 flex items-center gap-2 text-xs text-fg-muted">
			<ArrowLeftRight size={14} aria-hidden="true" />
			<span>{$t('layout.editor.structure.columns')}</span>
			<div
				bind:this={rulerEl}
				class="relative grid h-5 w-40 touch-none grid-cols-4 rounded-control border border-line"
				role="slider"
				tabindex="0"
				aria-label={$t('layout.editor.structure.columnsLabel')}
				aria-valuemin={1}
				aria-valuemax={GRID_MAX_COLUMNS}
				aria-valuenow={shownColumns}
				data-ruler={path}
				onpointerdown={rulerDown}
				onpointermove={rulerMove}
				onpointerup={rulerUp}
				onkeydown={rulerKey}
			>
				{#each [1, 2, 3, 4] as cell (cell)}
					<span
						class="border-r border-line last:border-r-0 {cell <= shownColumns ? 'bg-selected' : ''}"
					></span>
				{/each}
			</div>
			<span class="font-mono text-fg">{shownColumns}</span>
		</div>
	{/if}

	{#if takesChildren(node) || children.length > 0}
		<div
			class="mt-2 min-w-0 gap-2 {isGrid ? 'grid' : 'flex flex-col'}"
			style={isGrid ? `grid-template-columns: repeat(${shownColumns}, minmax(0, 1fr))` : undefined}
			data-snode-list={path}
		>
			{#each children as child, i (i)}
				<StructureNode
					node={child}
					path={`${path}/children/${i}`}
					parent={node}
					index={i}
					siblings={children.length}
					parentColumns={isGrid ? shownColumns : null}
					{ctrl}
				/>
			{/each}
		</div>
	{/if}

	{#if palette.length > 0}
		<details class="mt-2 text-xs" data-palette={path}>
			<summary class="cursor-pointer text-fg-muted hover:text-fg">
				{$t('layout.editor.structure.add')}
			</summary>
			<div class="mt-1 flex flex-wrap gap-1">
				{#each palette as type (type)}
					<button
						type="button"
						class="rounded-control border border-line px-2 py-0.5 font-mono hover:bg-selected"
						data-add={type}
						onclick={() => ctrl.add(path, type)}
					>
						{type}
					</button>
				{/each}
			</div>
		</details>
	{/if}
</div>
