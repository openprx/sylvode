<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import { catalogueApi, type CatalogueCapability } from './api';
	import { areaOf, riskTone, searchCatalogue } from './logic';

	/**
	 * The operations a plugin can reach, from `GET /api/v1/catalogue?exposure=plugin`, grouped by area.
	 * For plugin authors: which capability id opens which operation, with its method, path, scope and risk.
	 */
	let loading = $state(true);
	let failed = $state(false);
	let capabilities = $state.raw<CatalogueCapability[]>([]);
	let query = $state('');
	let area = $state('');

	onMount(async () => {
		const response = await catalogueApi.plugin();
		failed = response.code !== 0;
		capabilities = response.data?.capabilities ?? [];
		loading = false;
	});

	const areas = $derived([...new Set(capabilities.map((c) => areaOf(c.id)))].sort());
	const shown = $derived(
		searchCatalogue(
			capabilities.filter((c) => area === '' || areaOf(c.id) === area),
			query
		)
	);
	const groups = $derived.by(() => {
		const byArea = new Map<string, CatalogueCapability[]>();
		for (const capability of shown) {
			const key = areaOf(capability.id);
			byArea.set(key, [...(byArea.get(key) ?? []), capability]);
		}
		return [...byArea.entries()].sort(([a], [b]) => a.localeCompare(b));
	});
	const opTotal = $derived(shown.reduce((sum, c) => sum + c.operations.length, 0));

	function areaName(key: string): string {
		const path = `market.areas.${key}.name`;
		const text = get(t)(path);
		return text === path ? key : text;
	}

	function areaDescription(key: string): string {
		const path = `market.areas.${key}.desc`;
		const text = get(t)(path);
		return text === path ? '' : text;
	}
</script>

<div class="space-y-4" data-market-catalogue>
	<div>
		<h2 class="text-lg font-semibold text-fg">{$t('market.catTitle')}</h2>
		<p class="mt-1 max-w-2xl text-sm text-fg-muted">{$t('market.catLead')}</p>
	</div>

	<div class="flex flex-wrap items-end gap-3">
		<div class="min-w-64 flex-1">
			<TextField label={$t('market.catSearch')} bind:value={query} spellcheck={false} />
		</div>
		<label class="text-sm text-fg">
			<span class="mb-1 block">{$t('market.catArea')}</span>
			<select
				class="rounded-control border border-line bg-raised px-3 py-2 text-sm"
				bind:value={area}
			>
				<option value="">{$t('market.catAreaAll')}</option>
				{#each areas as key (key)}
					<option value={key}>{areaName(key)}</option>
				{/each}
			</select>
		</label>
	</div>

	{#if loading}
		<div class="rounded-card border border-line bg-raised p-6 text-fg-muted">
			{$t('market.catLoading')}
		</div>
	{:else if failed}
		<div class="rounded-card border border-line bg-raised p-6 text-fg-muted">
			{$t('market.catFailed')}
		</div>
	{:else if groups.length === 0}
		<div class="rounded-card border border-line bg-raised p-10 text-center text-fg-muted">
			{$t('market.catEmpty')}
		</div>
	{:else}
		<p class="text-xs text-fg-muted">
			{$t('market.catCounts', { values: { ops: opTotal, ids: shown.length } })}
		</p>
		<div class="space-y-3">
			{#each groups as [key, items] (key)}
				<details
					class="rounded-card border border-line bg-raised"
					open={query.trim() !== '' || area !== ''}
					data-catalogue-area={key}
				>
					<summary class="cursor-pointer px-4 py-3">
						<span class="text-sm font-medium text-fg">{areaName(key)}</span>
						<span class="ml-2 text-xs text-fg-muted">{key} · {items.length}</span>
						{#if areaDescription(key)}
							<span class="block text-xs text-fg-muted">{areaDescription(key)}</span>
						{/if}
					</summary>
					<ul class="divide-y divide-line border-t border-line">
						{#each items as capability (capability.id)}
							<li class="space-y-1 px-4 py-3">
								<div class="flex flex-wrap items-center gap-2">
									<code class="text-sm text-fg">{capability.id}</code>
									<StatusChip tone={riskTone(capability.risk)}
										>{$t(`market.risk.${capability.risk}`)}</StatusChip
									>
								</div>
								<ul class="space-y-0.5 text-xs text-fg-muted">
									{#each capability.operations as operation (operation.op)}
										<li>
											<code class="text-fg">{operation.op}</code>
											· {operation.method}
											<code>{operation.path}</code>
											· {$t('market.catScope', { values: { scope: operation.scope } })}
											{#if operation.stability !== 'stable'}
												<StatusChip tone="warning">{operation.stability}</StatusChip>
											{/if}
											{#if operation.comment}
												<span class="block">{operation.comment}</span>
											{/if}
										</li>
									{/each}
								</ul>
							</li>
						{/each}
					</ul>
				</details>
			{/each}
		</div>
	{/if}
</div>
