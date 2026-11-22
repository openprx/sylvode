<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { t } from 'svelte-i18n';
	import { sharedBlockApi } from '#lib/flow/shared-blocks/client.js';
	import type { SharedDiscovery, SharedTargetContext } from '#lib/flow/shared-blocks/contracts.js';
	let { item, context }: { item: SharedDiscovery; context: SharedTargetContext } = $props();
	let excerpt = $state(''),
		available = $state(true),
		loaded = $state(false),
		permission = $state<'view' | 'edit' | null>(null);
	const abort = new AbortController();
	onMount(() => {
		void sharedBlockApi.read(item.share_id, abort.signal, context).then((result) => {
			if (abort.signal.aborted) return;
			loaded = true;
			available = result.code === 0 && !!result.data;
			if (!result.data) return;
			permission = result.data.effective_permission;
			const content = result.data.projection.members[0]?.content;
			excerpt =
				content && 'text' in content
					? content.text.slice(0, 160)
					: content?.kind === 'image'
						? [content.alt, content.caption].filter(Boolean).join(' · ')
						: content?.kind === 'attachment'
							? [content.name, content.caption].filter(Boolean).join(' · ')
							: '';
		});
	});
	onDestroy(() => abort.abort());
</script>

<p class="font-medium">{$t('flow.shared.blocks')}</p>
{#if !loaded}<p class="text-xs text-fg-muted">{$t('common.loading')}</p>{:else if !available}<p
		class="text-xs text-fg-muted"
	>
		{$t('flow.shared.unavailable')}
	</p>{:else if excerpt}<p class="mt-1 line-clamp-2 whitespace-pre-wrap text-sm text-fg-muted">
		{excerpt}
	</p>{/if}
<p class="mt-1 text-xs text-fg-muted">
	{(permission ?? item.effective_permission) === 'edit'
		? $t('flow.shared.canEditLimited')
		: $t('flow.shared.viewOnly')}
</p>
