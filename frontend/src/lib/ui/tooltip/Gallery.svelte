<script lang="ts">
	import Info from '@lucide/svelte/icons/info';
	import IconButton from '../icon-button/IconButton.svelte';
	import { tooltip } from './tooltip';
</script>

<section data-gallery-section="tooltip">
	<h2>tooltip</h2>
	<div class="row">
		<span data-gallery="tooltip:icon-button">
			<IconButton icon={Info} label="Info" shortcut="Ctrl K" />
		</span>
		<button
			type="button"
			data-gallery="tooltip:describes"
			use:tooltip={{ text: 'full', describes: true }}
		>
			describes
		</button>
	</div>
</section>

<style>
	.row {
		display: flex;
		gap: var(--rf-space-4);
	}
</style>
