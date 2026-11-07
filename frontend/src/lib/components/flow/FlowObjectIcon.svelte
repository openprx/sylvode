<script lang="ts">
	import { appearanceBlobUrl, type FlowAppearance } from '#lib/api/flow.js';

	interface Props {
		appearance?: FlowAppearance | null;
		objectId: string;
		size?: 'sm' | 'lg';
	}

	let { appearance = null, objectId, size = 'sm' }: Props = $props();

	let imageUrl = $state<string | null>(null);
	const assetId = $derived(
		appearance?.icon?.kind === 'image' ? appearance.icon.asset_id : null
	);
	const emoji = $derived(appearance?.icon?.kind === 'emoji' ? appearance.icon.emoji : null);
	const box = $derived(size === 'lg' ? 'h-16 w-16 text-4xl' : 'h-5 w-5 text-sm');

	$effect(() => {
		const id = assetId;
		imageUrl = null;
		if (!id) return;
		let active = true;
		void appearanceBlobUrl(objectId, id).then((url) => {
			if (active) imageUrl = url;
		});
		return () => {
			active = false;
		};
	});
</script>

{#if emoji}
	<span class={`inline-flex shrink-0 items-center justify-center ${box}`} aria-hidden="true">{emoji}</span>
{:else if imageUrl}
	<img src={imageUrl} alt="" class={`${box} shrink-0 rounded object-cover`} />
{/if}
