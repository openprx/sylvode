<svelte:options runes />

<script lang="ts">
	/* eslint-disable svelte/no-navigation-without-resolve -- console paths are built in full here */
	import { getContext, onMount } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import type { ApiResult } from '#lib/api/client.js';
	import { projectsApi } from '#lib/api/projects.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';
	import { authStore } from '#lib/stores/auth.js';
	import { toast } from '#lib/stores/toast.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import PageHeader from '#lib/ui/page-header/PageHeader.svelte';
	import type { SurfaceAction } from '#lib/ui/page-header/page-header.js';
	import Segmented from '#lib/ui/segmented/Segmented.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import { sitesApi } from './api';
	import { errorText } from './errors';
	import PublishDialog from './PublishDialog.svelte';
	import SettingsTab from './SettingsTab.svelte';
	import { SITE_TABS, siteTitle, statusTone, type SiteTab } from './view';
	import type { Site, SiteRole } from './wire';

	/** One site: header with its status actions, then the tab the address names. */
	let {
		workspaceId,
		projectId,
		siteId,
		tab,
		ontab
	}: {
		workspaceId: string;
		projectId: string;
		siteId: string;
		tab: SiteTab;
		ontab: (tab: SiteTab) => void;
	} = $props();

	const adminGate = getContext<(() => boolean) | undefined>(PLUGIN_ADMIN_CONTEXT);
	const isAdmin = $derived(adminGate?.() ?? false);

	let phase = $state<'loading' | 'ready' | 'failed' | 'missing'>('loading');
	let problem = $state('');
	let site = $state.raw<Site | null>(null);
	let projectName = $state('');
	let roles = $state.raw<SiteRole[]>([]);
	let busy = $state(false);
	let publishing = $state(false);

	const myId = $derived($authStore.user?.id ?? '');
	const myRole = $derived(roles.find((role) => role.user_id === myId)?.role ?? null);
	const canTitle = $derived(isAdmin || myRole === 'site_editor');
	const name = $derived(site ? siteTitle(site.title, $locale) : '');
	const listHref = $derived(
		`/workspace/${encodeURIComponent(workspaceId)}/projects/${encodeURIComponent(projectId)}/sites`
	);

	async function load(): Promise<void> {
		phase = 'loading';
		const result = await sitesApi.get(siteId);
		if (result.code === 0 && result.data) {
			site = result.data;
			phase = 'ready';
			void loadRoles();
		} else {
			problem = describeResult(result);
			phase = result.code === 404 || result.error_code === 'not_found' ? 'missing' : 'failed';
		}
	}

	async function loadRoles(): Promise<void> {
		const result = await sitesApi.roles(siteId);
		if (result.code === 0 && result.data) roles = result.data.items;
	}

	onMount(() => {
		void load();
		void projectsApi.get(projectId).then((project) => (projectName = project.data?.name ?? ''));
	});

	/** Runs one status call and shows its result. */
	async function change(
		call: (id: string) => Promise<ApiResult<Site>>,
		message: string
	): Promise<boolean> {
		if (!site) return false;
		busy = true;
		const result = await call(site.id);
		busy = false;
		if (result.code === 0 && result.data) {
			site = result.data;
			toast.success(message);
			return true;
		}
		toast.error(errorText(result, $t));
		return false;
	}

	async function publish(): Promise<void> {
		if (await change(sitesApi.publish, $t('sites.publish.done', { values: { title: name } })))
			publishing = false;
	}

	async function unpublish(): Promise<void> {
		const yes = await confirmAction({
			tone: 'neutral',
			title: $t('sites.unpublish.title', { values: { title: name } }),
			body: $t('sites.unpublish.body'),
			confirmLabel: $t('sites.unpublish.confirm')
		});
		if (yes) await change(sitesApi.unpublish, $t('sites.unpublish.done'));
	}

	async function disable(): Promise<void> {
		const yes = await confirmAction({
			tone: 'destructive',
			title: $t('sites.disable.title', { values: { title: name } }),
			body: $t('sites.disable.body'),
			confirmLabel: $t('sites.disable.confirm')
		});
		if (yes) await change(sitesApi.disable, $t('sites.disable.done'));
	}

	async function remove(): Promise<void> {
		if (!site) return;
		const yes = await confirmAction({
			tone: 'destructive',
			title: $t('sites.delete.title', { values: { title: name } }),
			body: $t('sites.delete.body'),
			confirmLabel: $t('sites.delete.confirm')
		});
		if (!yes) return;
		busy = true;
		const result = await sitesApi.remove(site.id);
		busy = false;
		if (result.code === 0) {
			toast.success($t('sites.delete.done'));
			void goto(listHref);
		} else {
			toast.error(errorText(result, $t));
			if (result.error_code === 'site_published') void load();
		}
	}

	async function grantMe(): Promise<void> {
		if (!site || !myId) return;
		const result = await sitesApi.putRole(site.id, myId, 'site_editor');
		if (result.code === 0) {
			toast.success($t('sites.draftTip.granted'));
			await loadRoles();
		} else toast.error(errorText(result, $t));
	}

	function openSite(): void {
		if (site?.url) window.open(site.url, '_blank', 'noopener,noreferrer');
	}

	const primary = $derived.by<SurfaceAction | undefined>(() => {
		if (!site || !isAdmin || site.status === 'published') return undefined;
		return {
			id: 'publish',
			label: $t('sites.actions.publish'),
			kind: 'primary',
			run: () => {
				publishing = true;
			}
		};
	});

	const secondary = $derived.by<SurfaceAction[]>(() => {
		if (!site) return [];
		const actions: SurfaceAction[] = [
			{
				id: 'open',
				label: $t('sites.actions.open'),
				kind: 'secondary',
				run: openSite,
				disabledReason: site.url ? undefined : $t('sites.list.notServed')
			}
		];
		if (!isAdmin) return actions;
		if (site.status === 'published')
			actions.push({
				id: 'unpublish',
				label: $t('sites.actions.unpublish'),
				kind: 'secondary',
				run: unpublish
			});
		if (site.status !== 'disabled')
			actions.push({
				id: 'disable',
				label: $t('sites.actions.disable'),
				kind: 'danger-quiet',
				run: disable
			});
		actions.push({
			id: 'delete',
			label: $t('sites.actions.delete'),
			kind: 'danger-quiet',
			run: remove,
			disabledReason: site.status === 'published' ? $t('sites.delete.unpublishFirst') : undefined
		});
		return actions;
	});
</script>

<div class="site" data-testid="site-detail">
	{#if phase === 'loading'}
		<PageHeader title="" loading />
	{:else if phase === 'missing'}
		<StateBlock
			kind="notFound"
			object={$t('sites.object')}
			action={{ label: $t('sites.backToList'), run: () => void goto(listHref) }}
		/>
	{:else if phase === 'failed' || !site}
		<StateBlock
			kind="loadFailed"
			thing={$t('sites.object')}
			message={problem || undefined}
			action={{ run: () => void load() }}
		/>
	{:else}
		<PageHeader
			title={name}
			context={{
				segments: [{ id: 'project', label: projectName || '…', href: listHref }],
				current: site.slug
			}}
			{primary}
			{secondary}
		>
			<StatusChip tone={statusTone(site.status)} data-testid="site-status">
				{$t(`sites.status.${site.status}`)}
			</StatusChip>
		</PageHeader>

		{#if site.status === 'draft'}
			<TipBar
				tone="neutral"
				data-testid="draft-tip"
				action={isAdmin && !myRole && myId
					? { label: $t('sites.draftTip.grant'), run: () => void grantMe() }
					: undefined}
			>
				{$t('sites.draftTip.text')}
			</TipBar>
		{:else if site.status === 'disabled'}
			<TipBar tone="warning">{$t('sites.disabledTip')}</TipBar>
		{/if}

		{#if SITE_TABS.length > 1}
			<Segmented
				label={$t('sites.tabs.label')}
				width="fill"
				value={tab}
				options={SITE_TABS.map((value) => ({ value, label: $t(`sites.tabs.${value}`) }))}
				onchange={ontab}
			/>
		{/if}

		{#if tab === 'settings'}
			<SettingsTab
				{site}
				{isAdmin}
				{canTitle}
				onchanged={(next) => {
					site = next;
				}}
			/>
		{/if}
	{/if}
</div>

{#if publishing && site}
	<PublishDialog
		{site}
		title={name}
		{busy}
		onconfirm={() => void publish()}
		onclose={() => (publishing = false)}
	/>
{/if}

<style>
	.site {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
		padding: var(--rf-space-9);
		font-family: var(--rf-font-sans-stack);
		color: var(--rf-text);
	}
</style>
