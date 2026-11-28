<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { layoutApi } from '#lib/layout/api.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import Segmented from '#lib/ui/segmented/Segmented.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import type { SelectOption } from '#lib/ui/select/listbox.js';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import { sitesApi } from './api';
	import { errorText } from './errors';
	import {
		isRoute,
		layoutIssues,
		navFields,
		navProblem,
		navValue,
		pageErrorField,
		type PageField
	} from './view';
	import type { PageAudience, PageInput, SitePage } from './wire';

	/**
	 * Adds a page to a site, or changes one of its own pages. The layout is chosen among the
	 * project's layouts; making layouts is left to agents and packages. Absent fields stay on an
	 * edit, so only what changed is sent.
	 */
	let {
		siteId,
		projectId,
		page,
		currentLayoutKey,
		onclose,
		onsaved
	}: {
		siteId: string;
		projectId: string;
		/** The page to change; absent for a new page. */
		page?: SitePage;
		/** The layout key of the page's current layout, for a layout outside the project's list. */
		currentLayoutKey?: string;
		onclose: () => void;
		onsaved: () => void;
	} = $props();

	const initialNav = $derived(navFields(page?.nav ?? null));
	let route = $state('');
	let audience = $state<PageAudience>('visitor');
	let navEn = $state('');
	let navZh = $state('');
	let layoutId = $state<string | null>(null);
	let seeded = $state(false);
	let layouts = $state.raw<SelectOption[]>([]);
	let layoutsPhase = $state<'loading' | 'ready' | 'failed'>('loading');
	let busy = $state(false);
	let errors = $state<Partial<Record<PageField | 'form', string>>>({});
	let issues = $state<string[]>([]);

	$effect(() => {
		if (seeded) return;
		seeded = true;
		route = page?.route ?? '';
		audience = page?.audience ?? 'visitor';
		navEn = initialNav.en;
		navZh = initialNav.zh;
		layoutId = page?.layout_id ?? null;
	});

	async function loadLayouts(): Promise<void> {
		layoutsPhase = 'loading';
		const result = await layoutApi.listLayouts('project', projectId);
		if (result.code !== 0 || !result.data) {
			layoutsPhase = 'failed';
			return;
		}
		const options: SelectOption[] = result.data.items
			.filter((layout) => layout.status === 'active' && layout.installation_id === null)
			.map((layout) => ({ value: layout.id, label: layout.layout_key }));
		if (page && currentLayoutKey && !options.some((option) => option.value === page.layout_id))
			options.unshift({ value: page.layout_id, label: currentLayoutKey });
		layouts = options;
		layoutsPhase = 'ready';
	}

	onMount(() => {
		void loadLayouts();
	});

	function clear(field: PageField): void {
		errors = { ...errors, [field]: undefined, form: undefined };
	}

	async function submit(): Promise<void> {
		const next: Partial<Record<PageField | 'form', string>> = {};
		issues = [];
		if (!isRoute(route)) next.route = $t('sites.errors.route_invalid');
		if (navProblem(navEn, navZh)) next.nav = $t('sites.errors.nav_invalid');
		if (!layoutId) next.layout = $t('sites.pages.layoutNeeded');
		errors = next;
		if (Object.keys(next).length > 0 || !layoutId) return;

		const body: PageInput = {};
		if (!page || route !== page.route) body.route = route;
		if (!page || audience !== page.audience) body.audience = audience;
		const nav = navValue(navEn, navZh);
		if (!page || JSON.stringify(nav) !== JSON.stringify(navValue(initialNav.en, initialNav.zh)))
			body.nav = nav;
		if (!page || layoutId !== page.layout_id) body.layout_id = layoutId;
		if (page && Object.keys(body).length === 0) {
			onclose();
			return;
		}
		busy = true;
		const result = page
			? await sitesApi.updatePage(page.id, body)
			: await sitesApi.createPage(siteId, body);
		busy = false;
		if (result.code === 0) {
			onsaved();
			return;
		}
		const message = errorText(result, $t);
		const field = pageErrorField(result.error_code);
		if (result.error_code === 'layout_invalid') issues = layoutIssues(result.details);
		if (result.error_code === 'not_found') errors = { layout: $t('sites.pages.layoutNotUsable') };
		else if (field) errors = { [field]: message };
		else errors = { form: message };
	}
</script>

<Dialog
	open
	title={page ? $t('sites.pages.editTitle') : $t('sites.pages.addTitle')}
	{onclose}
	testId="page-dialog"
>
	<form
		class="form"
		onsubmit={(event) => {
			event.preventDefault();
			void submit();
		}}
	>
		<TextField
			label={$t('sites.pages.route')}
			value={route}
			oninput={(event) => {
				route = event.currentTarget.value;
				clear('route');
			}}
			helper={$t('sites.pages.routeHelper')}
			error={errors.route}
			maxlength={200}
			autocomplete="off"
			data-testid="page-route"
		/>
		<Segmented
			label={$t('sites.pages.audience')}
			width="fill"
			value={audience}
			options={[
				{ value: 'visitor', label: $t('sites.pages.audienceVisitor') },
				{ value: 'operator', label: $t('sites.pages.audienceOperator') }
			]}
			onchange={(value) => {
				audience = value;
				clear('audience');
			}}
		/>
		{#if errors.audience}<TipBar tone="danger">{errors.audience}</TipBar>{/if}
		<TextField
			label={$t('sites.pages.navEn')}
			value={navEn}
			oninput={(event) => {
				navEn = event.currentTarget.value;
				clear('nav');
			}}
			helper={$t('sites.pages.navHelper')}
			error={errors.nav}
			maxlength={200}
			autocomplete="off"
		/>
		<TextField
			label={$t('sites.pages.navZh')}
			value={navZh}
			oninput={(event) => {
				navZh = event.currentTarget.value;
				clear('nav');
			}}
			maxlength={200}
			autocomplete="off"
		/>
		{#if layoutsPhase === 'failed'}
			<TipBar tone="warning" action={{ label: $t('sites.retry'), run: () => void loadLayouts() }}>
				{$t('sites.pages.layoutsFailed')}
			</TipBar>
		{:else}
			<Select
				label={$t('sites.pages.layout')}
				value={layoutId}
				options={layouts}
				placeholder={$t('sites.pages.layoutPlaceholder')}
				helper={layoutsPhase === 'ready' && layouts.length === 0
					? $t('sites.pages.noLayouts')
					: $t('sites.pages.layoutHelper')}
				error={errors.layout}
				disabled={layoutsPhase === 'loading'}
				onchange={(value) => {
					layoutId = value;
					clear('layout');
					issues = [];
				}}
				testId="page-layout"
			/>
			{#if issues.length > 0}
				<ul class="issues" data-testid="layout-issues">
					{#each issues as issue (issue)}<li>{issue}</li>{/each}
				</ul>
			{/if}
		{/if}
		{#if errors.form}<TipBar tone="danger">{errors.form}</TipBar>{/if}
	</form>
	{#snippet actions()}
		<Button variant="secondary" disabled={busy} onclick={onclose}>{$t('sites.cancel')}</Button>
		<Button variant="primary" loading={busy} onclick={() => void submit()} data-testid="save-page">
			{page ? $t('sites.pages.save') : $t('sites.pages.create')}
		</Button>
	{/snippet}
</Dialog>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	.issues {
		margin: 0;
		padding-inline-start: var(--rf-space-7);
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
</style>
