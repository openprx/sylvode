<script lang="ts">
	import { t } from 'svelte-i18n';
	import { untrack } from 'svelte';
	import type { SharedRead, SharedMember } from '#lib/flow/shared-blocks/contracts.js';
	import { sharedProjectionRows } from '#lib/flow/shared-blocks/contracts.js';
	import { sharedAssetProvider } from '#lib/flow/shared-blocks/providers.js';
	import FlowInlineText from './FlowInlineText.svelte';
	import FlowBodyAsset from './FlowBodyAsset.svelte';
	let {
		read,
		onEdit,
		authorized = () => true
	}: {
		read: SharedRead;
		onEdit?: (member: SharedMember) => void;
		authorized?: () => boolean;
	} = $props();
	let folds = $state<Record<string, boolean>>({});
	const authorityKey = $derived(`${read.projection.share_id}:${read.projection.revision}`);
	const provider = $derived.by(() => {
		void authorityKey;
		return untrack(() => sharedAssetProvider(read, authorized));
	});
	const canonicalRows = $derived.by(() => {
		void authorityKey;
		return untrack(() => sharedProjectionRows(read));
	});
	$effect(() => {
		const current = provider;
		return () => current.dispose();
	});
	const rows = $derived.by(() => {
		let hidden: number | null = null;
		return canonicalRows.filter((row) => {
			if (hidden !== null && row.depth <= hidden) hidden = null;
			if (hidden !== null) return false;
			if (
				row.member.content.kind === 'toggle' &&
				(folds[row.member.member_id] ?? row.member.content.collapsed)
			)
				hidden = row.depth;
			return true;
		});
	});
</script>

<section
	aria-label={$t('flow.shared.blocks')}
	data-testid="flow-shared-projection"
	data-revision={read.projection.revision}
	data-permission={read.effective_permission}
>
	{#each rows as row (row.member.member_id)}{@const content = row.member.content}
		<article
			class="group relative my-2 min-h-6"
			style={`margin-inline-start:${row.depth * 20}px`}
			data-member-id={row.member.member_id}
			data-member-kind={content.kind}
		>
			{#if content.kind === 'heading'}<svelte:element
					this={`h${content.level}`}
					class="font-semibold"
					><FlowInlineText text={content.text} spans={content.spans} /></svelte:element
				>
			{:else if content.kind === 'paragraph'}<p class="whitespace-pre-wrap">
					<FlowInlineText text={content.text} spans={content.spans} />
				</p>
			{:else if content.kind === 'quote'}<blockquote
					class="border-s-2 border-line-strong ps-3 whitespace-pre-wrap"
				>
					<FlowInlineText text={content.text} spans={content.spans} />
				</blockquote>
			{:else if content.kind === 'bullet' || content.kind === 'ordered'}<div class="flex gap-2">
					<span aria-hidden="true">{content.kind === 'ordered' ? `${row.listNumber}.` : '•'}</span>
					<p class="whitespace-pre-wrap">
						<FlowInlineText text={content.text} spans={content.spans} />
					</p>
				</div>
			{:else if content.kind === 'todo'}<label class="flex gap-2"
					><input type="checkbox" checked={content.checked} disabled /><span
						><FlowInlineText text={content.text} spans={content.spans} /></span
					></label
				>
			{:else if content.kind === 'toggle'}<div class="flex gap-2">
					<button
						type="button"
						aria-label={$t('flow.shared.toggle')}
						aria-expanded={!(folds[row.member.member_id] ?? content.collapsed)}
						onclick={() =>
							(folds[row.member.member_id] = !(folds[row.member.member_id] ?? content.collapsed))}
						>{(folds[row.member.member_id] ?? content.collapsed) ? '▸' : '▾'}</button
					><span><FlowInlineText text={content.text} spans={content.spans} /></span>
				</div>
			{:else if content.kind === 'code'}<pre class="overflow-x-auto rounded bg-surface p-3"><code
						>{content.text}</code
					></pre>
			{:else if content.kind === 'divider'}<hr />
			{:else if content.kind === 'image'}<FlowBodyAsset
					kind="image"
					assetId={content.asset_alias}
					{provider}
					alt={content.alt}
					caption={content.caption}
				/>
			{:else if content.kind === 'attachment'}<FlowBodyAsset
					kind="attachment"
					assetId={content.asset_alias}
					{provider}
					name={content.name}
					caption={content.caption}
				/>
			{:else}<div class="rounded border border-line px-3 py-2 text-sm text-fg-muted">
					{$t('flow.shared.childPlaceholder')}
				</div>{/if}
			{#if read.effective_permission === 'edit' && onEdit && 'text' in content}<button
					type="button"
					class="mt-1 rounded px-2 py-1 text-xs text-fg-muted hover:bg-surface"
					onclick={() => onEdit?.(row.member)}>{$t('flow.shared.editMember')}</button
				>{/if}
		</article>
	{/each}
</section>
