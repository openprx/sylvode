<script lang="ts">
	import { t } from 'svelte-i18n';
	import ArrowDown from '@lucide/svelte/icons/arrow-down';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import Lock from '@lucide/svelte/icons/lock';
	import Plus from '@lucide/svelte/icons/plus';
	import X from '@lucide/svelte/icons/x';
	import Button from '../../ui/button/Button.svelte';
	import IconButton from '../../ui/icon-button/IconButton.svelte';
	import Menu from '../../ui/menu/Menu.svelte';
	import MenuItem from '../../ui/menu/MenuItem.svelte';
	import Popover from '../../ui/popover/Popover.svelte';
	import '../../ui/foundations/visually-hidden.css';
	import {
		governanceShortcut,
		shortcutId,
		type RailShortcuts,
		type Shortcut
	} from '../shortcuts.svelte';
	import { glyphFor } from './glyphs';
	import type { RailItemModel } from './rail-model';

	interface Props {
		open: boolean;
		/** "Edit navigation" button (or the top bar's "More"): gets focus back on close. */
		anchor: HTMLElement | null;
		onclose: () => void;
		shortcuts: RailShortcuts;
		/** The four fixed destinations, listed first and locked. */
		destinations: readonly Pick<RailItemModel, 'id' | 'label' | 'glyph'>[];
		/** The page the "Current page" entry would add, or null when the page cannot be pinned. */
		current: Shortcut | null;
		/**
		 * Neutral confirmation before "Restore default navigation" (the app passes
		 * `confirmAction({ tone: 'neutral', ... })`); resolves true to go on.
		 */
		confirmRestore: () => Promise<boolean>;
	}

	let { open, anchor, onclose, shortcuts, destinations, current, confirmRestore }: Props = $props();

	let addMenuOpen = $state(false);
	let addButton = $state<HTMLElement | null>(null);
	/** Inline reason (never a toast) and the last change, for screen readers. */
	let problem = $state('');
	let announcement = $state('');

	const items = $derived(shortcuts.items);
	const canAddCurrent = $derived(current !== null && !shortcuts.has(current.id));
	const canAddGovernance = $derived(!shortcuts.has(shortcutId('governance', 'governance')));
	const canAdd = $derived(canAddCurrent || canAddGovernance);

	$effect(() => {
		if (!open) {
			problem = '';
			announcement = '';
			addMenuOpen = false;
		}
	});

	function add(shortcut: Shortcut) {
		const result = shortcuts.add(shortcut);
		if (!result.ok) {
			problem = $t(result.reason);
			return;
		}
		problem = '';
		if (result.added)
			announcement = $t('shell.rail.editor.added', { values: { label: shortcut.label } });
	}

	function move(item: Shortcut, delta: -1 | 1) {
		if (shortcuts.move(item.id, delta))
			announcement = $t(delta < 0 ? 'shell.rail.editor.movedUp' : 'shell.rail.editor.movedDown', {
				values: { label: item.label }
			});
	}

	function remove(item: Shortcut) {
		shortcuts.remove(item.id);
		problem = '';
		announcement = $t('shell.rail.editor.removed', { values: { label: item.label } });
	}

	async function restore() {
		if (items.length === 0) return;
		if (!(await confirmRestore())) return;
		shortcuts.restoreDefault();
		problem = '';
		announcement = $t('shell.rail.editor.restored');
	}
</script>

<Popover
	{open}
	{anchor}
	{onclose}
	label={$t('shell.rail.editor.title')}
	width={300}
	placement="bottom-start"
	testId="shortcut-editor"
	data-shell="shortcut-editor"
>
	<div class="editor">
		<h2 class="title">{$t('shell.rail.editor.title')}</h2>

		<ul class="rows locked" aria-label={$t('shell.rail.editor.fixedLabel')}>
			{#each destinations as destination (destination.id)}
				{@const Glyph = glyphFor(destination.glyph)}
				<li class="row" data-editor-fixed={destination.id}>
					<Glyph size={16} aria-hidden="true" />
					<span class="name">{destination.label}</span>
					<span class="note"
						><Lock size={14} aria-hidden="true" />{$t('shell.rail.editor.always')}</span
					>
				</li>
			{/each}
		</ul>

		<ul class="rows" aria-label={$t('shell.rail.editor.shortcutsLabel')}>
			{#each items as item, index (item.id)}
				<li class="row" data-editor-shortcut={item.id}>
					<span class="name">{item.label}</span>
					<span class="tools">
						<IconButton
							icon={ArrowUp}
							label={$t('shell.rail.editor.moveUp', { values: { label: item.label } })}
							disabled={index === 0}
							onclick={() => move(item, -1)}
						/>
						<IconButton
							icon={ArrowDown}
							label={$t('shell.rail.editor.moveDown', { values: { label: item.label } })}
							disabled={index === items.length - 1}
							onclick={() => move(item, 1)}
						/>
						<IconButton
							icon={X}
							label={$t('shell.rail.editor.remove', { values: { label: item.label } })}
							onclick={() => remove(item)}
						/>
					</span>
				</li>
			{:else}
				<li class="empty">{$t('shell.rail.editor.empty')}</li>
			{/each}
		</ul>

		{#if problem}<p class="problem" role="alert" data-shell="shortcut-problem">{problem}</p>{/if}
		<p class="ui-sr-only" role="status" aria-live="polite">{announcement}</p>

		<div class="footer">
			<span bind:this={addButton} class="add">
				<Button
					variant="secondary"
					size="sm"
					aria-haspopup="menu"
					aria-expanded={addMenuOpen}
					disabled={!canAdd}
					disabledReason={canAdd ? undefined : $t('shell.rail.editor.nothingToAdd')}
					onclick={() => (addMenuOpen = !addMenuOpen)}
				>
					<Plus size={14} aria-hidden="true" />{$t('shell.rail.editor.add')}
				</Button>
			</span>
			<Button variant="primary" size="sm" onclick={onclose}>{$t('shell.rail.editor.done')}</Button>
		</div>
		<div class="restore">
			<Button variant="text" size="sm" disabled={items.length === 0} onclick={restore}>
				{$t('shell.rail.editor.restore')}
			</Button>
		</div>
	</div>
</Popover>

<Menu
	open={addMenuOpen}
	anchor={addButton}
	onclose={() => (addMenuOpen = false)}
	label={$t('shell.rail.editor.add')}
	data-shell="shortcut-add-menu"
>
	{#if current && canAddCurrent}
		<MenuItem onselect={() => add(current)}>
			{$t('shell.rail.editor.addCurrent', { values: { label: current.label } })}
		</MenuItem>
	{/if}
	{#if canAddGovernance}
		<MenuItem onselect={() => add(governanceShortcut($t('shell.destinations.governance')))}>
			{$t('shell.destinations.governance')}
		</MenuItem>
	{/if}
</Menu>

<style>
	.editor {
		box-sizing: border-box;
		width: 100%;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		font-family: var(--rf-font-sans-stack);
		color: var(--rf-text);
	}
	.title {
		margin: 0;
		font-size: var(--rf-size-body);
		font-weight: var(--rf-weight-semibold);
	}
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-1);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.locked {
		padding-block-end: var(--rf-space-4);
		border-block-end: var(--rf-border-width) solid var(--rf-line);
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		min-height: var(--rf-control-height);
		font-size: var(--rf-size-secondary);
	}
	.name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.note {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-2);
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.tools {
		display: inline-flex;
	}
	.empty {
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.problem {
		margin: 0;
		color: var(--rf-danger);
		font-size: var(--rf-size-meta);
	}
	.footer {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--rf-space-4);
	}
	.editor > * {
		flex: none;
	}
	.restore {
		display: flex;
	}
	.add {
		margin-inline-end: auto;
		display: inline-flex;
	}
</style>
