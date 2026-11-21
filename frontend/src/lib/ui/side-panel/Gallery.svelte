<script lang="ts">
	import PanelDemo from './PanelDemo.svelte';
	import SidePanel from './SidePanel.svelte';
	import type { PanelStatus } from './side-panel';

	const statuses: PanelStatus[] = ['loading', 'failed', 'forbidden', 'missing'];
</script>

<section data-gallery-section="side-panel">
	<h2>side-panel</h2>
	<div class="stack">
		<div data-gallery="side-panel:docked"><PanelDemo id="docked" containerWidth={1000} /></div>
		<div data-gallery="side-panel:overlay"><PanelDemo id="overlay" containerWidth={999} /></div>
		<div class="statuses">
			{#each statuses as status (status)}
				<div class="hosted" data-gallery="side-panel:hosted-{status}">
					<SidePanel
						title={status}
						{status}
						hosted
						width={340}
						onresize={() => {}}
						onclose={() => {}}
						onretry={() => {}}
					>
						<span>{status}</span>
					</SidePanel>
				</div>
			{/each}
		</div>
	</div>
</section>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
	}
	.statuses {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-5);
	}
	.hosted {
		width: 340px;
		border: var(--rf-border-width) solid var(--rf-line);
	}
</style>
