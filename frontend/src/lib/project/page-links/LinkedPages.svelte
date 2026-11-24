<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import FileText from '@lucide/svelte/icons/file-text';
	import { pageLinksApi, type PageLink } from '#lib/api/page-links.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import PageLinkPicker from './PageLinkPicker.svelte';
	import { MAX_PAGE_LINKS, canLinkMore, pageLinkView } from './rules.js';

	interface Props {
		issueId: string;
		workspaceId: string;
		/** Called after a link or unlink went through (the page reloads its activity). */
		onchange?: () => void;
	}

	let { issueId, workspaceId, onchange }: Props = $props();

	let links = $state<PageLink[]>([]);
	let loaded = $state(false);
	let failed = $state(false);
	let picking = $state(false);
	let busyId = $state<string | null>(null);
	let requestId = 0;

	const views = $derived(links.map(pageLinkView));
	const linkedIds = $derived(new Set(links.map((link) => link.object_id)));
	const limitReached = $derived(!canLinkMore(links.length));

	async function load(id: string) {
		const mine = ++requestId;
		const result = await pageLinksApi.list(id);
		if (mine !== requestId) return;
		if (result.code === 0) {
			links = result.data ?? [];
			failed = false;
		} else {
			failed = true;
		}
		loaded = true;
	}

	let shownFor = '';
	$effect(() => {
		if (shownFor !== issueId) {
			// A different work item must not keep showing the previous one's links.
			shownFor = issueId;
			links = [];
			loaded = false;
			failed = false;
			picking = false;
		}
		void load(issueId);
	});

	async function linkPage(objectId: string) {
		const result = await pageLinksApi.link(issueId, objectId);
		if (result.code !== 0) {
			toast.error(describeResult(result));
			return;
		}
		picking = false;
		toast.success($t('pageLinks.linked'));
		await load(issueId);
		onchange?.();
	}

	async function unlinkPage(objectId: string) {
		busyId = objectId;
		const result = await pageLinksApi.unlink(issueId, objectId);
		busyId = null;
		if (result.code !== 0) {
			toast.error(describeResult(result));
			return;
		}
		toast.success($t('pageLinks.unlinked'));
		await load(issueId);
		onchange?.();
	}

	function titleOf(view: ReturnType<typeof pageLinkView>): string {
		return view.title ?? $t(view.stateKey ?? 'pageLinks.untitled');
	}
</script>

<section
	class="rounded-card border border-line bg-raised p-5"
	data-testid="linked-pages"
	aria-labelledby="linked-pages-title"
>
	<div class="mb-4 flex flex-wrap items-center justify-between gap-2">
		<h2 id="linked-pages-title" class="text-lg font-semibold text-fg">
			{$t('pageLinks.title')}
			{#if loaded && !failed}
				<span class="text-sm font-normal text-fg-muted">
					({$t('pageLinks.count', { values: { count: links.length, max: MAX_PAGE_LINKS } })})
				</span>
			{/if}
		</h2>
		{#if loaded && !failed}
			<Button
				size="sm"
				disabled={limitReached}
				disabledReason={limitReached
					? $t('pageLinks.limitReached', { values: { max: MAX_PAGE_LINKS } })
					: undefined}
				onclick={() => (picking = !picking)}
			>
				{$t('pageLinks.link')}
			</Button>
		{/if}
	</div>

	{#if picking && !limitReached}
		<PageLinkPicker
			{workspaceId}
			{linkedIds}
			onchoose={linkPage}
			onclose={() => (picking = false)}
		/>
	{/if}

	{#if failed}
		<StateBlock
			kind="loadFailed"
			thing={$t('pageLinks.thing')}
			action={{ run: () => load(issueId) }}
		/>
	{:else if !loaded}
		<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
	{:else if views.length === 0}
		<p class="text-sm text-fg-muted" data-testid="linked-pages-empty">{$t('pageLinks.empty')}</p>
	{:else}
		<ul class="space-y-2" data-testid="linked-pages-list">
			{#each views as view (view.objectId)}
				<li
					class="flex items-center justify-between gap-3 rounded-control border border-line bg-surface px-3 py-2"
					data-testid="linked-page"
					data-state={view.restricted ? 'restricted' : 'visible'}
				>
					<span class="flex min-w-0 items-center gap-2 text-sm">
						<FileText class="size-4 shrink-0 text-fg-muted" aria-hidden="true" />
						{#if view.restricted}
							<span class="truncate text-fg-muted">{titleOf(view)}</span>
						{:else}
							<a
								class="truncate font-medium text-fg underline"
								href={resolve('/(app)/flow/[objectId]', { objectId: view.objectId })}
								aria-label={$t('pageLinks.open') + ': ' + titleOf(view)}>{titleOf(view)}</a
							>
						{/if}
						{#if view.inTrash}
							<span
								class="shrink-0 rounded-control border border-line px-1.5 text-xs text-fg-muted"
								data-testid="linked-page-trash">{$t('pageLinks.inTrash')}</span
							>
						{/if}
					</span>
					<Button
						size="sm"
						loading={busyId === view.objectId}
						aria-label={$t('pageLinks.unlinkLabel', { values: { title: titleOf(view) } })}
						onclick={() => unlinkPage(view.objectId)}
					>
						{$t('pageLinks.unlink')}
					</Button>
				</li>
			{/each}
		</ul>
	{/if}
</section>
