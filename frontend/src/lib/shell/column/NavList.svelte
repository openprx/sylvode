<script lang="ts">
	import { t } from 'svelte-i18n';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import { GLYPHS } from './glyphs';
	import { treeKeyAction, type NavRow } from './rows';

	interface Props {
		rows: NavRow[];
		/** Id of the selected row (`aria-current="page"`). */
		selectedId: string | null;
		ontoggle?: (id: string, open: boolean) => void;
		/** "P" on a pinnable row. */
		onpin?: (row: NavRow) => void;
		/** A row link was followed (the narrow window switches to the content view; a workspace row sets the scope). */
		onfollow?: (row: NavRow) => void;
	}

	let { rows, selectedId, ontoggle, onpin, onfollow }: Props = $props();

	function focusRow(list: HTMLElement, id: string) {
		const next = Array.from(list.querySelectorAll<HTMLElement>('[data-nav-row]')).find(
			(el) => el.dataset.rowId === id
		);
		next?.focus();
	}

	// Up, down, Home and End belong to the column's roving group; these are the tree keys.
	function onkeydown(event: KeyboardEvent, row: NavRow) {
		if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
		const list = (event.currentTarget as HTMLElement).closest<HTMLElement>('[data-shell-list]');
		if (!list) return;
		const firstChild = list.querySelector<HTMLElement>(`[data-parent-id="${CSS.escape(row.id)}"]`);
		const action = treeKeyAction(event.key, {
			id: row.id,
			parentId: row.parentId,
			expandable: row.expandable === true,
			expanded: row.expanded === true,
			firstChildId: firstChild?.dataset.rowId ?? null
		});
		if (!action) return;
		if (action.type === 'pin' && !row.pin) return;
		event.preventDefault();
		if (action.type === 'expand') ontoggle?.(row.id, true);
		else if (action.type === 'collapse') ontoggle?.(row.id, false);
		else if (action.type === 'focus') focusRow(list, action.id);
		else onpin?.(row);
	}
</script>

<ul class="rows">
	{#each rows as row (row.id)}
		{@const Glyph = GLYPHS[row.glyph]}
		<li>
			{#if row.caption}
				<p class="caption" style:--level={row.level} data-nav-caption={row.id}>{row.label}</p>
			{:else}
				<!-- eslint-disable svelte/no-navigation-without-resolve -- hrefs are built by the shell -->
				<a
					{...row.attrs}
					href={row.href}
					target={row.external ? '_blank' : undefined}
					rel={row.external ? 'noopener noreferrer' : undefined}
					class="row"
					class:selected={row.id === selectedId}
					style:--level={row.level}
					data-nav-row
					data-row-id={row.id}
					data-parent-id={row.parentId ?? undefined}
					aria-current={row.id === selectedId ? 'page' : undefined}
					aria-expanded={row.expandable ? (row.expanded ? 'true' : 'false') : undefined}
					onclick={() => onfollow?.(row)}
					onkeydown={(event) => onkeydown(event, row)}
				>
					<Glyph size={16} aria-hidden="true" />
					<span class="label">{row.label}</span>
					{#if row.count !== undefined && row.count > 0}
						<span
							class="count"
							aria-label={$t('shell.column.unread', { values: { count: row.count } })}
							>{row.count > 99 ? '99+' : row.count}</span
						>
					{/if}
					{#if row.external}
						<ExternalLink size={14} aria-hidden="true" />
					{/if}
					{#if row.expandable}
						<!-- Pointer-only: the keyboard uses the arrow keys. -->
						<span
							class="chevron"
							class:open={row.expanded}
							aria-hidden="true"
							tabindex="-1"
							onclick={(event) => {
								event.preventDefault();
								event.stopPropagation();
								ontoggle?.(row.id, !row.expanded);
							}}
						>
							<ChevronRight size={14} aria-hidden="true" />
						</span>
					{/if}
				</a>
				<!-- eslint-enable svelte/no-navigation-without-resolve -->
			{/if}
		</li>
	{/each}
</ul>

<style>
	.rows {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-1);
	}
	.caption {
		margin: 0;
		padding: var(--rf-space-4) var(--rf-space-4) var(--rf-space-2);
		padding-inline-start: calc(var(--rf-space-4) + var(--level, 0) * var(--rf-space-6));
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		font-weight: var(--rf-weight-medium);
	}
	/* Row height 32: the space-9 token (contract 2.4). */
	.row {
		position: relative;
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		min-height: var(--rf-space-9);
		padding-block: 0;
		padding-inline: calc(var(--rf-space-4) + var(--level, 0) * var(--rf-space-6)) var(--rf-space-4);
		border-radius: var(--rf-radius-sm);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		font-weight: var(--rf-weight-regular);
		text-decoration: none;
	}
	.row :global(svg) {
		flex: none;
		color: var(--rf-secondary);
	}
	.row:hover {
		background: var(--rf-hover-nav);
	}
	.row.selected {
		background: var(--rf-selected);
	}
	.row.selected :global(svg) {
		color: var(--rf-text);
	}
	/* 2px leading bar, 16 high. */
	.row.selected::before {
		content: '';
		position: absolute;
		inset-inline-start: 0;
		inset-block: calc(50% - var(--rf-space-4));
		width: var(--rf-border-width-focus);
		background: var(--rf-primary);
	}
	.row:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.label {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.count {
		flex: none;
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
		font-variant-numeric: tabular-nums;
	}
	.chevron {
		flex: none;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: var(--rf-space-7);
		height: var(--rf-space-7);
		border-radius: var(--rf-radius-xs);
		cursor: pointer;
	}
	.chevron:hover {
		background: var(--rf-hover);
	}
	.chevron :global(svg) {
		transition: transform var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.chevron.open :global(svg) {
		transform: rotate(90deg);
	}
	@media (pointer: coarse) {
		.row {
			min-height: var(--rf-touch-target);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.chevron :global(svg) {
			transition: none;
		}
	}
</style>
