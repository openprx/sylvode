<script lang="ts">
	import { t } from 'svelte-i18n';
	import StateBlock from './StateBlock.svelte';
</script>

<section data-gallery-section="state-block">
	<h2>state-block</h2>
	<div class="row">
		<span data-gallery="state-block:empty">
			<StateBlock
				kind="empty"
				thing={$t('ui.stateBlock.sample.thing')}
				action={{ label: $t('ui.stateBlock.sample.create'), run: () => {}, primary: true }}
			/>
		</span>
		<span data-gallery="state-block:loading">
			<StateBlock kind="loading" thing={$t('ui.stateBlock.sample.thing')} />
		</span>
		<span data-gallery="state-block:load-failed">
			<StateBlock
				kind="loadFailed"
				thing={$t('ui.stateBlock.sample.thing')}
				action={{ run: () => {} }}
			/>
		</span>
		<span data-gallery="state-block:forbidden">
			<StateBlock
				kind="forbidden"
				grantor={$t('ui.stateBlock.sample.grantor')}
				place={$t('ui.stateBlock.sample.place')}
			/>
		</span>
	</div>
</section>

<style>
	.row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-6);
		align-items: flex-start;
	}
</style>
