<svelte:options runes />

<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import { layoutApi } from './api';
	import { resolveText } from './refs';
	import { evaluateWhen } from './when';
	import LayoutView from './LayoutView.svelte';
	import type { PlacementEntry, SlotContextParams, SlotPlacements } from './types';

	/**
	 * A typed region of a host page. It lists the layouts placed in slot `id` for `context`
	 * (`GET /slots/{id}/placements`), renders each one on its own, and keeps the viewer's collapse
	 * and hide choices. A failing placement never reaches the host page: every one renders inside
	 * its own error boundary, and a failed fetch renders nothing.
	 */
	let {
		id,
		context,
		workspaceId,
		chrome = true,
		class: className = ''
	}: {
		id: string;
		context: SlotContextParams;
		workspaceId: string;
		/** `false` renders the layouts bare (an action bar); no title, collapse or hide controls. */
		chrome?: boolean;
		/** Spacing around the slot; only applied when something is rendered. */
		class?: string;
	} = $props();

	let loaded = $state.raw<SlotPlacements | null>(null);
	let hidden = $state.raw<PlacementEntry[]>([]);
	let collapsedOverride = $state.raw<Record<string, boolean>>({});
	let requestToken = 0;

	const loc = $derived($locale ?? 'en');
	const contextKey = $derived(
		`${id}|${context.project_id}|${context.form_id ?? ''}|${context.record_id ?? ''}`
	);
	const entries = $derived(
		(loaded?.entries ?? []).filter(
			(entry) =>
				!hidden.some((item) => item.placement_id === entry.placement_id) &&
				evaluateWhen(entry.condition)
		)
	);

	$effect(() => {
		void contextKey;
		const token = ++requestToken;
		hidden = [];
		collapsedOverride = {};
		layoutApi
			.slotPlacements(id, context)
			.then((result) => {
				if (token !== requestToken) return;
				loaded = result.code === 0 && result.data ? result.data : null;
			})
			.catch(() => {
				if (token === requestToken) loaded = null;
			});
	});

	function isCollapsed(entry: PlacementEntry): boolean {
		return collapsedOverride[entry.placement_id] ?? entry.collapsed;
	}

	async function toggleCollapsed(entry: PlacementEntry) {
		const next = !isCollapsed(entry);
		collapsedOverride = { ...collapsedOverride, [entry.placement_id]: next };
		const result = await layoutApi.updatePrefs(entry.placement_id, { collapsed: next });
		if (result.code !== 0) {
			collapsedOverride = { ...collapsedOverride, [entry.placement_id]: !next };
		}
	}

	async function hide(entry: PlacementEntry) {
		hidden = [...hidden, entry];
		const result = await layoutApi.updatePrefs(entry.placement_id, { hidden: true });
		if (result.code !== 0) hidden = hidden.filter((item) => item !== entry);
	}

	async function restore(entry: PlacementEntry) {
		hidden = hidden.filter((item) => item !== entry);
		const result = await layoutApi.updatePrefs(entry.placement_id, { hidden: false });
		if (result.code !== 0) hidden = [...hidden, entry];
	}

	function titleOf(entry: PlacementEntry): string {
		return (
			resolveText(
				entry.spec.title,
				{ params: {}, data: {}, slot: {}, viewer: {}, actions: {} },
				loc
			) || entry.layout_key
		);
	}
</script>

{#if entries.length > 0 || hidden.length > 0}
	<div class="flex min-w-0 flex-col gap-3 {className}" data-layout-slot={id}>
		{#key contextKey}
			{#each entries as entry (entry.placement_id)}
				<section class="min-w-0" data-layout-placement={entry.placement_id}>
					{#if chrome}
						<div class="mb-1 flex items-center justify-between gap-2">
							<h2 class="truncate text-xs font-semibold uppercase text-fg-muted">
								{titleOf(entry)}
							</h2>
							<span class="flex shrink-0 gap-3 text-xs text-fg-muted">
								<button
									type="button"
									class="hover:text-fg"
									aria-expanded={!isCollapsed(entry)}
									onclick={() => void toggleCollapsed(entry)}
								>
									{isCollapsed(entry) ? $t('layout.expand') : $t('layout.collapse')}
								</button>
								<button type="button" class="hover:text-fg" onclick={() => void hide(entry)}>
									{$t('layout.hide')}
								</button>
							</span>
						</div>
					{/if}
					{#if !chrome || !isCollapsed(entry)}
						<svelte:boundary>
							<LayoutView
								spec={entry.spec}
								{context}
								{workspaceId}
								installations={loaded?.installations ?? {}}
							/>
							{#snippet failed()}
								<p class="text-xs text-fg-muted" data-layout-failed>{$t('layout.failed')}</p>
							{/snippet}
						</svelte:boundary>
					{/if}
				</section>
			{/each}
		{/key}
		{#each hidden as entry (entry.placement_id)}
			<p class="text-xs text-fg-muted" data-layout-hidden={entry.placement_id}>
				{$t('layout.hiddenNote', { values: { title: titleOf(entry) } })}
				<button type="button" class="underline hover:text-fg" onclick={() => void restore(entry)}>
					{$t('layout.undo')}
				</button>
			</p>
		{/each}
	</div>
{/if}
