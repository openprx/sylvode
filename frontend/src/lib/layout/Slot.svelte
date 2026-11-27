<svelte:options runes />

<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import { authStore } from '#lib/stores/auth.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import { layoutApi } from './api';
	import { resolveText } from './refs';
	import { conditionScope } from './scope';
	import { evaluateWhen } from './when';
	import CreateHere from './editor/CreateHere.svelte';
	import { offeredScopes } from './editor/logic';
	import LayoutView from './LayoutView.svelte';
	import type { PlacementEntry, SlotContextParams, SlotDescriptor, SlotPlacements } from './types';

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
	let reloadTick = $state(0);
	let descriptor = $state.raw<SlotDescriptor | null>(null);
	let creating = $state(false);
	let removeFailed = $state(false);

	const loc = $derived($locale ?? 'en');
	const contextKey = $derived(
		`${id}|${context.project_id}|${context.form_id ?? ''}|${context.record_id ?? ''}`
	);
	const entries = $derived(
		(loaded?.entries ?? []).filter(
			(entry) =>
				!hidden.some((item) => item.placement_id === entry.placement_id) &&
				evaluateWhen(entry.condition, conditionScope(entry.spec, context))
		)
	);

	const role = $derived($authStore.user?.role);
	// "Create here" is offered where this viewer can write a placement at some scope of the slot;
	// the server still decides every write.
	const canEdit = $derived(
		chrome &&
			descriptor !== null &&
			offeredScopes(descriptor, context, workspaceId, role).length > 0
	);

	$effect(() => {
		if (!chrome || !role) return;
		let cancelled = false;
		void layoutApi
			.slots()
			.then((result) => {
				if (cancelled || result.code !== 0 || !result.data) return;
				descriptor = result.data.slots.find((item) => item.id === id) ?? null;
			})
			.catch(() => {});
		return () => {
			cancelled = true;
		};
	});

	$effect(() => {
		void contextKey;
		void reloadTick;
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

	async function remove(entry: PlacementEntry) {
		const confirmed = await confirmAction({
			verb: 'remove',
			object: titleOf(entry),
			title: $t('layout.editor.removeTitle', { values: { title: titleOf(entry) } }),
			body: $t('layout.editor.removeBody')
		});
		if (!confirmed) return;
		const result = await layoutApi.deletePlacement(entry.placement_id);
		removeFailed = result.code !== 0;
		if (result.code === 0) reloadTick += 1;
	}

	function saved() {
		creating = false;
		reloadTick += 1;
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

{#if entries.length > 0 || hidden.length > 0 || canEdit}
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
								{#if canEdit && entry.installation_id === null}
									<button
										type="button"
										class="hover:text-danger"
										data-layout-remove={entry.placement_id}
										onclick={() => void remove(entry)}
									>
										{$t('layout.editor.remove')}
									</button>
								{/if}
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
		{#if removeFailed}
			<p class="text-xs text-danger" role="alert">{$t('layout.editor.removeFailed')}</p>
		{/if}
		{#if canEdit && descriptor}
			{#if creating}
				<CreateHere
					slotId={id}
					{context}
					{workspaceId}
					{descriptor}
					onsaved={saved}
					onclose={() => (creating = false)}
				/>
			{:else}
				<p class="text-xs text-fg-muted">
					<button
						type="button"
						class="underline hover:text-fg"
						data-layout-create={id}
						onclick={() => (creating = true)}
					>
						{$t('layout.editor.open')}
					</button>
				</p>
			{/if}
		{/if}
	</div>
{/if}
