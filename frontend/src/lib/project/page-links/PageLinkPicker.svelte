<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { flowApi } from '#lib/api/flow.js';
	import Button from '#lib/ui/button/Button.svelte';
	import { PICKER_LIMIT, pickerCandidates } from './rules.js';

	interface Props {
		/** Pages are searched in this workspace, among those the person can see. */
		workspaceId: string;
		linkedIds: ReadonlySet<string>;
		onchoose: (objectId: string) => void | Promise<void>;
		onclose: () => void;
	}

	let { workspaceId, linkedIds, onchoose, onclose }: Props = $props();

	let query = $state('');
	let hits = $state<Array<{ id: string; title: string }>>([]);
	let searching = $state(false);
	let failed = $state(false);
	let searched = $state(false);
	let choosingId = $state<string | null>(null);
	let timer: ReturnType<typeof setTimeout> | null = null;
	let controller: AbortController | null = null;

	function cancelPending() {
		if (timer) clearTimeout(timer);
		timer = null;
		controller?.abort();
		controller = null;
	}

	async function search(text: string) {
		cancelPending();
		const mine = new AbortController();
		controller = mine;
		searching = true;
		const result = await flowApi.searchObjects(
			workspaceId,
			{ q: text, all_visible: true, object_type: 'page', limit: PICKER_LIMIT },
			mine.signal
		);
		if (mine.signal.aborted) return;
		searching = false;
		searched = true;
		if (result.code === 0 && result.data) {
			failed = false;
			hits = pickerCandidates(result.data.items, linkedIds);
		} else {
			failed = true;
			hits = [];
		}
	}

	function oninput() {
		const text = query.trim();
		if (timer) clearTimeout(timer);
		if (!text) {
			cancelPending();
			hits = [];
			searching = false;
			searched = false;
			failed = false;
			return;
		}
		timer = setTimeout(() => void search(text), 250);
	}

	async function choose(id: string) {
		choosingId = id;
		await onchoose(id);
		choosingId = null;
	}

	$effect(() => cancelPending);
</script>

<div
	class="mb-4 space-y-2 rounded-control border border-line bg-surface p-3"
	data-testid="page-link-picker"
>
	<div class="flex items-center gap-2">
		<input
			type="search"
			class="min-h-11 w-full rounded-control border border-input bg-field px-2 text-sm text-fg"
			aria-label={$t('pageLinks.picker.label')}
			placeholder={$t('pageLinks.picker.placeholder')}
			bind:value={query}
			{oninput}
		/>
		<Button size="sm" onclick={onclose}>{$t('pageLinks.picker.close')}</Button>
	</div>
	<div aria-live="polite" class="text-sm">
		{#if searching}
			<p class="text-fg-muted">{$t('pageLinks.picker.searching')}</p>
		{:else if failed}
			<p class="text-fg-muted" role="alert">{$t('pageLinks.picker.failed')}</p>
		{:else if !searched}
			<p class="text-fg-muted">{$t('pageLinks.picker.hint')}</p>
		{:else if hits.length === 0}
			<p class="text-fg-muted">{$t('pageLinks.picker.none')}</p>
		{/if}
	</div>
	{#if hits.length > 0 && !searching}
		<ul class="space-y-1" data-testid="page-link-results">
			{#each hits as hit (hit.id)}
				<li>
					<button
						type="button"
						class="min-h-11 w-full truncate rounded-control px-2 text-left text-sm text-fg hover:bg-raised disabled:text-fg-muted"
						disabled={choosingId !== null}
						aria-label={$t('pageLinks.picker.choose', { values: { title: hit.title } })}
						onclick={() => choose(hit.id)}>{hit.title}</button
					>
				</li>
			{/each}
		</ul>
	{/if}
</div>
