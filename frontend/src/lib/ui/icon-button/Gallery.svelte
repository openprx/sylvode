<script lang="ts">
	import Ellipsis from '@lucide/svelte/icons/ellipsis';
	import IconButton from './IconButton.svelte';
</script>

<section data-gallery-section="icon-button">
	<h2>icon-button</h2>
	<div class="row">
		<span data-gallery="icon-button:default">
			<IconButton icon={Ellipsis} label="More actions" shortcut="Ctrl K" />
		</span>
		<span data-gallery="icon-button:pressed">
			<IconButton icon={Ellipsis} label="Pressed" pressed={true} />
		</span>
		<span data-gallery="icon-button:disabled">
			<IconButton icon={Ellipsis} label="Disabled" disabled />
		</span>
	</div>
</section>

<style>
	.row {
		display: flex;
		gap: var(--rf-space-4);
	}
</style>
