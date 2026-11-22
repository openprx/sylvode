<script lang="ts">
	import { onMount } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { apiClient } from '#lib/api/client.js';
	import {
		pluginPackagesApi,
		type InstalledPluginPackage,
		type PluginBindingEvent,
		type PluginCapability,
		type PluginGrant,
		type PluginInvocationEvent,
		type PluginPlacement
	} from '#lib/api/plugin-packages.js';
	import Button from '#lib/components/Button.svelte';

	let {
		projectId,
		packages,
		history
	}: {
		projectId: string;
		packages: InstalledPluginPackage[];
		history?: PluginBindingEvent[];
	} = $props();

	let receipts = $state<PluginInvocationEvent[]>([]);
	let loading = $state(false);
	let loadError = $state('');
	let invalidated = false;

	onMount(() => {
		const signal = apiClient.authSessionSignal();
		const invalidate = () => {
			invalidated = true;
			receipts = [];
		};
		signal.addEventListener('abort', invalidate, { once: true });
		void refresh();
		return () => {
			invalidate();
			signal.removeEventListener('abort', invalidate);
		};
	});

	async function refresh() {
		if (invalidated || loading) return;
		loading = true;
		loadError = '';
		const result = await pluginPackagesApi.invocations(projectId);
		if (invalidated) return;
		loading = false;
		if (result.code !== 0 || !Array.isArray(result.data)) {
			receipts = [];
			loadError = result.code === 403 ? 'plugins.manageDenied' : 'plugins.historyError';
			return;
		}
		receipts = result.data;
	}

	function formatDate(value: string, language: string | null | undefined) {
		return new Intl.DateTimeFormat(language || 'en', {
			dateStyle: 'medium',
			timeStyle: 'short'
		}).format(new Date(value));
	}

	function grantLabel(grant: PluginGrant) {
		if (grant === 'amount.preview' || grant === 'amount.apply')
			return $t(`plugins.capabilityShort.${grant}`);
		if (grant === 'ui.display') return $t('plugins.layoutV3.grant.uiDisplay');
		if (grant.startsWith('read:'))
			return $t('plugins.layoutV3.grant.readId', { values: { id: grant.slice(5) } });
		return grant;
	}

	function describeGrants(grants: PluginGrant[]) {
		return grants.length ? grants.map(grantLabel).join(', ') : $t('plugins.historyNoGrants');
	}

	function describePlacements(layout: PluginPlacement[] | undefined) {
		const enabled = layout?.filter((placement) => placement.enabled);
		return enabled?.length
			? enabled.map((placement) => `${placement.id} (${placement.position})`).join(', ')
			: $t('plugins.layoutNone');
	}

	function reasonLabel(reason: string) {
		return reason === 'configuration_reset'
			? $t('plugins.layoutV3.reason.configuration_reset')
			: reason;
	}

	function packageVersion(version: string | null) {
		return version ? `v${version}` : $t('plugins.historyDisabled');
	}

	function recordVersion(row: PluginInvocationEvent) {
		const item = packages.find((entry) => entry.id === row.package_id);
		const key = row.key ?? item?.manifest.key ?? row.package_id;
		const version = row.version ?? item?.manifest.version;
		return version ? `${key} · v${version}` : key;
	}
</script>

<div class="space-y-3">
	<details class="rounded-card border border-line bg-raised p-4">
		<summary class="cursor-pointer text-base font-semibold">{$t('plugins.bindingHistory')}</summary>
		{#if history === undefined}
			<p class="mt-3 text-sm">{$t('plugins.historyUnavailable')}</p>
		{:else if !history.length}
			<p class="mt-3 text-sm">{$t('plugins.historyNoChanges')}</p>
		{:else}
			<ol class="mt-3 divide-y divide-line">
				{#each history as row (row.request_id + row.key)}
					<li class="space-y-2 py-3 text-sm">
						<div class="flex flex-wrap items-baseline justify-between gap-2">
							<h3 class="font-semibold">
								{row.key} · {packageVersion(row.previous_version)} → {packageVersion(row.version)}
							</h3>
							<time datetime={row.created_at} class="text-xs text-fg-muted"
								>{formatDate(row.created_at, $locale)}</time
							>
						</div>
						<p>
							{$t('plugins.historyPermissionChange', {
								values: {
									from: describeGrants(row.previous_grants),
									to: describeGrants(row.grants)
								}
							})}
						</p>
						{#if row.reason}
							<p>
								{$t('plugins.layoutV3.historyReason', {
									values: { reason: reasonLabel(row.reason) }
								})}
							</p>
						{/if}
						{#if row.placements || row.previous_placements}
							<p>
								{$t('plugins.historyLayoutChange', {
									values: {
										from: describePlacements(row.previous_placements),
										to: describePlacements(row.placements)
									}
								})}
							</p>
						{/if}
						<dl class="grid gap-2 sm:grid-cols-2">
							<div>
								<dt>{$t('plugins.historyActor')}</dt>
								<dd class="break-all font-mono text-xs">{row.actor_id}</dd>
							</div>
							<div>
								<dt>{$t('plugins.historyRequest')}</dt>
								<dd class="break-all font-mono text-xs">{row.request_id}</dd>
							</div>
						</dl>
						<p class="text-xs text-fg-muted">
							{$t('plugins.historyGeneration', { values: { generation: row.generation } })}
						</p>
					</li>
				{/each}
			</ol>
		{/if}
	</details>

	<details class="rounded-card border border-line bg-raised p-4">
		<summary class="cursor-pointer text-base font-semibold"
			>{$t('plugins.invocationHistory')}</summary
		>
		<div class="mt-3 flex flex-wrap items-center gap-2">
			<Button variant="secondary" size="sm" {loading} onclick={refresh}
				>{$t('plugins.historyRefresh')}</Button
			>
			{#if loading}<span role="status" class="text-sm">{$t('common.loading')}</span>{/if}
		</div>
		{#if loadError}
			<p role="alert" class="mt-3 text-sm text-danger">{$t(loadError)}</p>
		{:else if !receipts.length && !loading}
			<p class="mt-3 text-sm">{$t('plugins.historyNoInvocations')}</p>
		{:else}
			<ol class="mt-3 divide-y divide-line">
				{#each receipts as row (row.request_id + row.record_id + row.actor_id + row.capability + row.generation + row.package_id)}
					<li class="space-y-2 py-3 text-sm">
						<div class="flex flex-wrap items-baseline justify-between gap-2">
							<h3 class="font-semibold">{recordVersion(row)} · {grantLabel(row.capability)}</h3>
							<time datetime={row.created_at} class="text-xs text-fg-muted"
								>{formatDate(row.created_at, $locale)}</time
							>
						</div>
						<p>
							{$t(
								row.error_kind
									? 'plugins.historyInvocationFailed'
									: 'plugins.historyInvocationSucceeded'
							)}{#if row.error_kind}
								· {row.error_kind}{/if}
						</p>
						<dl class="grid gap-2 sm:grid-cols-2">
							<div>
								<dt>{$t('plugins.historyRecord')}</dt>
								<dd class="break-all font-mono text-xs">{row.record_id}</dd>
							</div>
							<div>
								<dt>{$t('plugins.historyRequest')}</dt>
								<dd class="break-all font-mono text-xs">{row.request_id}</dd>
							</div>
							<div>
								<dt>{$t('plugins.historyActor')}</dt>
								<dd class="break-all font-mono text-xs">{row.actor_id}</dd>
							</div>
							{#if row.preview_request_id}<div>
									<dt>{$t('plugins.historyPreview')}</dt>
									<dd class="break-all font-mono text-xs">{row.preview_request_id}</dd>
								</div>{/if}
						</dl>
						<p class="text-xs text-fg-muted">
							{$t('plugins.historyGeneration', { values: { generation: row.generation } })}
						</p>
					</li>
				{/each}
			</ol>
		{/if}
	</details>
</div>
