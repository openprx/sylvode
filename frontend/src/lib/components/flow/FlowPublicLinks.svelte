<script lang="ts">
	import { t } from 'svelte-i18n';
	import { flowApi, type FlowShareLink } from '#lib/api/flow.js';
	import { flowErrorFromResult } from '#lib/flow/command-service.js';
	import { flowErrorDetailKey } from '#lib/flow/errors.js';

	interface Props {
		objectId: string;
	}

	let { objectId }: Props = $props();

	let links = $state<FlowShareLink[]>([]);
	let expiry = $state('');
	let freshToken = $state<string | null>(null);
	let loading = $state(true);
	let busy = $state(false);
	let failureKey = $state<string | null>(null);
	let copied = $state(false);

	async function load(): Promise<void> {
		loading = true;
		const result = await flowApi.listShareLinks(objectId);
		if (result.code === 0 && result.data) {
			links = result.data.items;
			failureKey = null;
		} else if (result.error_code === 'forbidden' || result.code === 403) {
			links = [];
			failureKey = null;
		} else {
			failureKey = flowErrorDetailKey(flowErrorFromResult(result));
		}
		loading = false;
	}

	$effect(() => {
		const id = objectId;
		freshToken = null;
		void loadFor(id);
	});

	async function loadFor(id: string): Promise<void> {
		loading = true;
		const result = await flowApi.listShareLinks(id);
		if (id !== objectId) return;
		if (result.code === 0 && result.data) links = result.data.items;
		else if (result.error_code !== 'forbidden' && result.code !== 403) {
			failureKey = flowErrorDetailKey(flowErrorFromResult(result));
		}
		loading = false;
	}

	async function create(): Promise<void> {
		if (busy) return;
		busy = true;
		copied = false;
		let expires_at: string | null = null;
		if (expiry) {
			const parsed = new Date(expiry);
			if (!Number.isNaN(parsed.getTime())) expires_at = parsed.toISOString();
		}
		const result = await flowApi.createShareLink(objectId, { expires_at });
		busy = false;
		if (result.code !== 0 || !result.data) {
			failureKey = flowErrorDetailKey(flowErrorFromResult(result));
			return;
		}
		freshToken = result.data.token;
		expiry = '';
		await load();
	}

	async function revoke(linkId: string): Promise<void> {
		if (busy) return;
		busy = true;
		const result = await flowApi.revokeShareLink(objectId, linkId);
		busy = false;
		if (result.code !== 0) {
			failureKey = flowErrorDetailKey(flowErrorFromResult(result));
			return;
		}
		if (freshToken) freshToken = null;
		await load();
	}

	async function copyToken(): Promise<void> {
		if (!freshToken || typeof navigator === 'undefined' || !navigator.clipboard) return;
		await navigator.clipboard.writeText(`${location.origin}/s/${freshToken}`);
		copied = true;
	}

	const active = $derived(links.filter((link) => !link.revoked_at));
</script>

<section class="space-y-2 border-t border-line pt-4" data-testid="flow-public-links">
	<h3 class="text-xs font-semibold uppercase text-fg-muted">
		{$t('flow.share.public.title')}
	</h3>
	<p class="text-xs text-fg-muted">{$t('flow.share.public.help')}</p>
	{#if loading}
		<p class="text-sm text-fg-muted">{$t('flow.share.public.loading')}</p>
	{:else if active.length === 0}
		<p class="text-sm text-fg-muted">{$t('flow.share.public.empty')}</p>
	{:else}
		<ul class="space-y-2">
			{#each active as link (link.id)}
				<li class="flex items-center justify-between gap-2 text-sm">
					<span class="font-mono text-xs text-fg-muted">{link.token_prefix}…</span>
					<button type="button" class="text-xs text-danger" onclick={() => revoke(link.id)}>
						{$t('flow.share.public.revoke')}
					</button>
				</li>
			{/each}
		</ul>
	{/if}
	<label class="block text-xs text-fg-muted">
		{$t('flow.share.public.expiry')}
		<input
			type="datetime-local"
			class="mt-1 min-h-11 w-full rounded-control border border-input px-2 bg-field"
			bind:value={expiry}
		/>
	</label>
	<button
		type="button"
		class="min-h-11 w-full rounded-control bg-primary rf-filled px-3 text-sm font-medium text-on-primary disabled:bg-surface disabled:text-fg-muted"
		disabled={busy}
		onclick={create}
	>
		{busy ? $t('flow.share.public.creating') : $t('flow.share.public.create')}
	</button>
	{#if freshToken}
		<p class="text-xs text-fg-muted">{$t('flow.share.public.tokenOnce')}</p>
		<input
			readonly
			class="w-full font-mono text-xs"
			value={`${typeof location === 'undefined' ? '' : location.origin}/s/${freshToken}`}
		/>
		<button type="button" class="text-xs text-fg underline" onclick={copyToken}>
			{copied ? $t('flow.share.public.copied') : $t('flow.share.public.copy')}
		</button>
	{/if}
	{#if failureKey}
		<p class="text-danger" role="alert">{$t(failureKey)}</p>
	{/if}
</section>
