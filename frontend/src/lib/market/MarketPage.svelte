<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import Modal from '#lib/components/Modal.svelte';
	import Button from '#lib/ui/button/Button.svelte';
	import Segmented from '#lib/ui/segmented/Segmented.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { authStore } from '#lib/stores/auth.js';
	import { workspacesApi } from '#lib/api/workspaces.js';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import { marketApi, marketDetailApi, type PackageDetail, type PackageSummary } from './api';
	import CatalogueBrowser from './CatalogueBrowser.svelte';
	import InstallationsPage from './InstallationsPage.svelte';
	import type { Role } from './index-logic';
	import { KINDS, pickText, type Kind } from './logic';
	import PackageDetailView from './PackageDetail.svelte';
	import PublicTab from './PublicTab.svelte';

	/**
	 * The workspace Market: Discover (built-in and this workspace's private packages, filtered by
	 * kind and searched on the server), Public (plugins of the public index sources) and Installed
	 * (the installations of the workspace). Nothing
	 * here decides who may install; the server does, and the buttons only follow its answer.
	 */
	let { workspaceId }: { workspaceId: string } = $props();

	type Tab = 'discover' | 'public' | 'installed' | 'developers';
	let tab = $state<Tab>('discover');
	let kind = $state<Kind | 'all'>('all');
	let query = $state('');
	let loading = $state(true);
	let packages = $state.raw<PackageSummary[]>([]);
	let failed = $state(false);

	/** Installed package keys; the installations list is answered to administrators only. */
	let admin = $state(false);
	let role = $state<Role>('member');
	let installedKeys = $state.raw<Set<string>>(new Set());

	let detail = $state.raw<PackageDetail | null>(null);
	let detailOpen = $state(false);
	let installDigest = $state<string | null>(null);

	let searchToken = 0;
	async function search() {
		const token = ++searchToken;
		loading = true;
		const response = await marketApi.packages(workspaceId, {
			...(kind !== 'all' ? { kind } : {}),
			q: query.trim()
		});
		if (token !== searchToken) return;
		loading = false;
		failed = response.code !== 0;
		packages = response.data?.items ?? [];
		if (failed) toast.error(describeResult(response, get(t)('market.loadFailed')));
	}

	async function probeInstalled() {
		const list = await marketApi.list(workspaceId);
		admin = list.code === 0;
		installedKeys = new Set(
			(list.data?.items ?? []).filter((i) => i.status !== 'revoked').map((i) => i.package_key)
		);
	}

	async function probeRole() {
		const userId = get(authStore).user?.id;
		const members = await workspacesApi.getMembers(workspaceId);
		role = members.data?.items.find((member) => member.user_id === userId)?.role ?? 'member';
	}

	onMount(() => {
		void probeInstalled();
		void probeRole();
	});

	// Search runs on kind change at once and on typing after a pause.
	$effect(() => {
		void kind;
		const text = query;
		const timer = setTimeout(() => void search(), text === '' ? 0 : 300);
		return () => clearTimeout(timer);
	});

	async function open(pkg: PackageSummary) {
		const response = await marketDetailApi.package(workspaceId, pkg.digest);
		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response, get(t)('market.detailLoadFailed')));
			return;
		}
		detail = response.data;
		detailOpen = true;
	}

	function install() {
		if (!detail) return;
		installDigest = detail.digest;
		detailOpen = false;
		tab = 'installed';
	}

	/** A package imported from the public index: its install flow opens on the Installed tab. */
	function imported(digest: string) {
		installDigest = digest;
		tab = 'installed';
	}

	function kindName(value: string): string {
		const key = `market.kindOne.${value}`;
		const text = get(t)(key);
		return text === key ? value : text;
	}

	const kindOptions = $derived([
		{ value: 'all' as const, label: $t('market.kindAll') },
		...KINDS.map((value) => ({ value, label: $t(`market.kind.${value}`) }))
	]);
	const tabOptions = $derived([
		{ value: 'discover' as const, label: $t('market.tabDiscover') },
		{ value: 'public' as const, label: $t('market.tabPublic') },
		{ value: 'installed' as const, label: $t('market.tabInstalled') },
		{ value: 'developers' as const, label: $t('market.tabDevelopers') }
	]);
	const sorted = $derived([...packages].sort((a, b) => a.package_key.localeCompare(b.package_key)));
</script>

<div class="space-y-6 p-6" data-market-page>
	<div>
		<h1 class="text-2xl font-semibold text-fg">{$t('market.marketTitle')}</h1>
		<p class="mt-1 max-w-2xl text-sm text-fg-muted">{$t('market.marketLead')}</p>
	</div>

	<Segmented
		label={$t('market.marketTitle')}
		options={tabOptions}
		value={tab}
		onchange={(value) => {
			tab = value;
			if (value === 'discover') installDigest = null;
		}}
		width={520}
	/>

	{#if tab === 'discover'}
		<div class="flex flex-wrap items-end gap-3">
			<div class="min-w-64 flex-1">
				<TextField label={$t('market.searchLabel')} bind:value={query} spellcheck={false} />
			</div>
			<Segmented
				label={$t('market.kindLabel')}
				options={kindOptions}
				value={kind}
				onchange={(value) => (kind = value)}
				width={520}
			/>
		</div>

		{#if loading}
			<div class="rounded-card border border-line bg-raised p-6 text-fg-muted">
				{$t('market.loading')}
			</div>
		{:else if sorted.length === 0}
			<div class="rounded-card border border-line bg-raised p-10 text-center text-fg-muted">
				{failed ? $t('market.loadFailed') : $t('market.searchEmpty')}
			</div>
		{:else}
			<ul class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
				{#each sorted as pkg (pkg.digest)}
					<li
						class="flex flex-col gap-2 rounded-card border border-line bg-raised p-4"
						data-market-package={pkg.package_key}
					>
						<div class="flex items-start justify-between gap-2">
							<div class="min-w-0">
								<div class="truncate text-sm font-semibold text-fg">{pkg.package_key}</div>
								<div class="text-xs text-fg-muted">
									{$t('market.version', { values: { version: pkg.version } })}
								</div>
							</div>
							{#if installedKeys.has(pkg.package_key)}
								<StatusChip tone="success">{$t('market.installedBadge')}</StatusChip>
							{/if}
						</div>
						<p class="line-clamp-2 min-h-8 text-sm text-fg-muted">
							{pickText(pkg.summary, $locale ?? 'en')}
						</p>
						<div class="flex flex-wrap items-center gap-2 text-xs">
							<StatusChip>{kindName(pkg.kind)}</StatusChip>
							<StatusChip>{$t(`market.tier.${pkg.tier}`)}</StatusChip>
							{#if pkg.custom_code}
								<StatusChip tone="warning">{$t('market.customCode')}</StatusChip>
							{/if}
						</div>
						<div class="mt-auto flex justify-end">
							<Button variant="secondary" size="sm" onclick={() => void open(pkg)}
								>{$t('market.details')}</Button
							>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	{:else if tab === 'public'}
		<PublicTab {workspaceId} {role} onimported={imported} />
	{:else if tab === 'developers'}
		<CatalogueBrowser />
	{:else}
		{#key installDigest}
			<InstallationsPage {workspaceId} embedded {installDigest} />
		{/key}
	{/if}
</div>

<Modal
	bind:open={detailOpen}
	title={detail?.package_key ?? ''}
	maxWidthClass="max-w-2xl"
	onclose={() => (detail = null)}
>
	{#if detail}
		<PackageDetailView {detail} canInstall={admin} oninstall={install} />
	{/if}
</Modal>
