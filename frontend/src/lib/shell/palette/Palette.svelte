<script lang="ts">
	import { t, locale } from 'svelte-i18n';
	import Dialog from '../../ui/dialog/Dialog.svelte';
	import { uid } from '../../ui/foundations/ids';
	import { nextRovingIndex } from '../../ui/foundations/roving';
	import '../../ui/foundations/visually-hidden.css';
	import { glyphFor } from '../rail/glyphs';
	import {
		flowEntries,
		issueEntries,
		buildPaletteView,
		type PaletteActionRef,
		type PaletteContext,
		type PaletteEntry
	} from './palette-sources';
	import type { PaletteSources } from './palette-api';
	import { createServerSearch, type ServerSearch } from './palette-search';

	interface Props {
		open: boolean;
		onclose: () => void;
		context: PaletteContext;
		sources: PaletteSources;
		/** Open an in-app path: in this tab, or (Cmd/Ctrl+Enter, modified click) in a new browser tab. */
		onnavigate: (href: string, options: { newTab: boolean }) => void;
		/** Run an action entry. The palette closes first. */
		onaction: (action: PaletteActionRef) => void;
		/** Platform form of the shortcut shown in the hint ("Ctrl K"). */
		shortcutHint?: string;
	}

	let { open, onclose, context, sources, onnavigate, onaction, shortcutHint }: Props = $props();

	const listId = uid('palette-list');
	const optionId = (entry: PaletteEntry) => `${listId}-${entry.id.replace(/[^\w-]/g, '_')}`;

	let query = $state('');
	let active = $state(0);
	let flowHits = $state.raw<PaletteEntry[] | null>(null);
	let issueHits = $state.raw<PaletteEntry[] | null>(null);
	let loading = $state(0);
	let listEl = $state<HTMLElement | null>(null);

	const collator = $derived(new Intl.Collator($locale ?? 'en'));
	const view = $derived(
		buildPaletteView({
			query,
			t: $t,
			collator,
			goto: context.goto,
			recent: context.recent,
			projects: context.projects,
			forms: context.forms,
			actions: context.actions,
			flow: flowHits,
			issues: issueHits,
			flowEnabled: context.flowWorkspace !== null
		})
	);
	const activeEntry = $derived(view.entries[Math.min(active, view.entries.length - 1)] ?? null);

	// Server searches: one for issues and comments, one for Flow pages of the current workspace.
	let issueSearch: ServerSearch | null = null;
	let flowSearch: ServerSearch | null = null;

	function makeSearches() {
		const count = (on: boolean) => (loading = Math.max(0, loading + (on ? 1 : -1)));
		let issuePending = false;
		let flowPending = false;
		issueSearch = createServerSearch({
			fetch: (q, signal) => sources.searchAll(q, signal),
			onresult: (_q, result) => {
				issueHits = result ? issueEntries(result, context.fallbackWorkspaceId, $t) : null;
			},
			onloading: (on) => {
				if (on !== issuePending) count(on);
				issuePending = on;
			}
		});
		flowSearch = createServerSearch({
			fetch: (q, signal) =>
				context.flowWorkspace
					? sources.searchFlow(context.flowWorkspace.id, q, 5, signal)
					: Promise.resolve(null),
			onresult: (_q, result) => {
				flowHits =
					result && context.flowWorkspace ? flowEntries(result, context.flowWorkspace, $t) : null;
			},
			onloading: (on) => {
				if (on !== flowPending) count(on);
				flowPending = on;
			}
		});
	}

	function stopSearches() {
		issueSearch?.cancel();
		flowSearch?.cancel();
		issueSearch = null;
		flowSearch = null;
		loading = 0;
	}

	$effect(() => {
		if (!open) return;
		query = '';
		active = 0;
		flowHits = null;
		issueHits = null;
		makeSearches();
		return stopSearches;
	});

	function oninput() {
		active = 0;
		issueSearch?.request(query);
		if (context.flowWorkspace) flowSearch?.request(query);
	}

	function run(entry: PaletteEntry, newTab: boolean) {
		if (entry.href) {
			onclose();
			onnavigate(entry.href, { newTab });
		} else if (entry.action && !newTab) {
			onclose();
			onaction(entry.action);
		}
	}

	function onkeydown(event: KeyboardEvent) {
		if (event.isComposing) return;
		if (event.key === 'Enter') {
			if (!activeEntry) return;
			event.preventDefault();
			run(activeEntry, (event.metaKey || event.ctrlKey) && Boolean(activeEntry.href));
			return;
		}
		if (event.altKey || event.ctrlKey || event.metaKey) return;
		const next = nextRovingIndex(
			event.key,
			active,
			view.entries.map(() => true),
			{ orientation: 'vertical' }
		);
		if (next === null) return;
		event.preventDefault();
		active = next;
	}

	$effect(() => {
		if (!listEl || !activeEntry) return;
		listEl
			.querySelector<HTMLElement>(`[id="${optionId(activeEntry)}"]`)
			?.scrollIntoView?.({ block: 'nearest' });
	});

	function click(entry: PaletteEntry, event: MouseEvent) {
		run(entry, (event.metaKey || event.ctrlKey) && Boolean(entry.href));
	}
</script>

<Dialog
	{open}
	title={$t('shell.palette.title')}
	{onclose}
	width={560}
	headless
	initialFocus="[data-palette-input]"
	testId="palette-dialog"
>
	<div class="palette" data-shell="palette">
		<input
			class="field"
			type="text"
			role="combobox"
			autocomplete="off"
			autocapitalize="off"
			spellcheck="false"
			aria-expanded="true"
			aria-controls={listId}
			aria-activedescendant={activeEntry ? optionId(activeEntry) : undefined}
			aria-label={$t('shell.palette.placeholder')}
			placeholder={$t('shell.palette.placeholder')}
			data-palette-input
			bind:value={query}
			{oninput}
			{onkeydown}
		/>

		<div
			class="results"
			id={listId}
			role="listbox"
			aria-label={$t('shell.palette.results')}
			bind:this={listEl}
		>
			{#each view.groups as group (group.id)}
				<div class="group" role="group" aria-label={group.label} data-palette-group={group.id}>
					<div class="caption" aria-hidden="true">{group.label}</div>
					{#each group.entries as entry (entry.id)}
						{@const Glyph = glyphFor(entry.glyph)}
						<!-- svelte-ignore a11y_click_events_have_key_events -->
						<div
							class="option"
							id={optionId(entry)}
							role="option"
							aria-selected={entry === activeEntry}
							data-palette-kind={entry.kind}
							tabindex="-1"
							onclick={(event) => click(entry, event)}
							onpointermove={() => (active = view.entries.indexOf(entry))}
						>
							<Glyph size={16} aria-hidden="true" />
							<span class="text">
								<span class="title">{entry.title}</span>
								{#if entry.context}<span class="context">{entry.context}</span>{/if}
							</span>
							<span class="verb">{entry.verb}</span>
						</div>
					{/each}
				</div>
			{/each}
			{#if view.entries.length === 0}
				<p class="none" role="presentation">
					{$t(loading > 0 ? 'shell.palette.searching' : 'shell.palette.none')}
				</p>
			{/if}
		</div>

		<p class="hint">{$t('shell.palette.hint')}{shortcutHint ? ` · ${shortcutHint}` : ''}</p>
		<p class="ui-sr-only" role="status" aria-live="polite">
			{loading > 0
				? $t('shell.palette.searching')
				: $t('shell.palette.count', { values: { count: view.entries.length } })}
		</p>
	</div>
</Dialog>

<style>
	.palette {
		display: flex;
		flex-direction: column;
		font-family: var(--rf-font-sans-stack);
		color: var(--rf-text);
	}
	.field {
		box-sizing: border-box;
		height: var(--rf-field-height);
		margin: var(--rf-space-5);
		width: calc(100% - 2 * var(--rf-space-5));
		padding: 0 var(--rf-space-5);
		border: var(--rf-border-width) solid var(--rf-input-border);
		border-radius: var(--rf-radius-input);
		background: var(--rf-input);
		color: var(--rf-text);
		font: inherit;
		font-size: var(--rf-size-body);
	}
	.field:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 1px;
	}
	.results {
		max-height: 360px;
		overflow-y: auto;
		padding: 0 var(--rf-space-4) var(--rf-space-4);
	}
	.caption {
		padding: var(--rf-space-4) var(--rf-space-4) var(--rf-space-2);
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		font-weight: var(--rf-weight-medium);
	}
	.option {
		display: flex;
		align-items: center;
		gap: var(--rf-space-5);
		min-height: 38px;
		padding: var(--rf-space-2) var(--rf-space-4);
		border-radius: var(--rf-radius-sm);
		cursor: pointer;
	}
	.option[aria-selected='true'] {
		background: var(--rf-selected);
	}
	.text {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-width: 0;
	}
	.title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--rf-text);
		font-size: var(--rf-size-button);
	}
	.context,
	.verb {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.verb {
		flex: none;
	}
	.none {
		margin: 0;
		padding: var(--rf-space-6) var(--rf-space-4);
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.hint {
		margin: 0;
		padding: var(--rf-space-4) var(--rf-space-6);
		border-block-start: var(--rf-border-width) solid var(--rf-line);
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
</style>
