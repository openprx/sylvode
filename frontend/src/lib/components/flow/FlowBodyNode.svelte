<script lang="ts">
	import type { Readable } from 'svelte/store';
	import FlowBodyAsset from './FlowBodyAsset.svelte';
	import FlowBodyChild from './FlowBodyChild.svelte';
	import type { PageBodyJobs, BodyJobsSnapshot } from '#lib/flow/body/jobs.js';
	let {
		source,
		provider,
		jobs,
		editable,
		onReplace,
		onDetails,
		onContinue
	}: {
		source: Readable<{
			kind: string;
			objectId: string;
			attrs: Record<string, unknown>;
			readOnly: boolean;
		}>;
		provider: import('#lib/flow/body/providers.js').BodyAssetProvider;
		jobs: PageBodyJobs | null;
		editable: Readable<boolean>;
		onReplace?: () => void;
		onDetails?: () => void;
		onContinue?: () => void;
	} = $props();
	const empty: BodyJobsSnapshot = { draft: null, uploads: [], children: [], pending: 0 };
	let jobsState = $state(empty);
	$effect(() => jobs?.subscribe((value) => (jobsState = value)));
	const pendingTitle = $derived.by(() => {
		void jobsState;
		return jobs?.pendingTitle(String($source.attrs.child_page_id)) ?? null;
	});
</script>

{#if $source.kind === 'childPage'}<FlowBodyChild
		childId={String($source.attrs.child_page_id)}
		{pendingTitle}
	/>
{:else}<FlowBodyAsset
		kind={$source.kind as 'image' | 'attachment'}
		{provider}
		assetId={String($source.attrs.asset_id)}
		alt={String($source.attrs.alt ?? '')}
		caption={String($source.attrs.caption ?? '')}
		name={String($source.attrs.name ?? '')}
		onReplace={!$editable ? undefined : onReplace}
		onDetails={!$editable ? undefined : onDetails}
		onContinue={!$editable ? undefined : onContinue}
	/>{/if}
