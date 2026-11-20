<script lang="ts">
	import TableDemo from './TableDemo.svelte';
</script>

<section data-gallery-section="data-table">
	<h2>data-table</h2>
	<div class="stack">
		<div data-gallery="data-table:default" class="wide"><TableDemo /></div>
		<div data-gallery="data-table:compact" class="wide"><TableDemo density="compact" /></div>
		<div data-gallery="data-table:narrow" class="narrow"><TableDemo frame={340} /></div>
		<div data-gallery="data-table:loading" class="wide"><TableDemo state="loading" /></div>
		<div data-gallery="data-table:empty" class="wide"><TableDemo state="empty" /></div>
		<div data-gallery="data-table:no-results" class="wide"><TableDemo state="noResults" /></div>
		<div data-gallery="data-table:failed" class="wide"><TableDemo state="failed" /></div>
		<div data-gallery="data-table:offline" class="wide"><TableDemo state="offline" /></div>
	</div>
</section>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
	}
	.wide {
		width: 760px;
		max-width: 100%;
	}
	.narrow {
		width: 340px;
	}
</style>
