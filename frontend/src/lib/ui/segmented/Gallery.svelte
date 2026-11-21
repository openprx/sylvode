<script lang="ts">
	import { t } from 'svelte-i18n';
	import Segmented from './Segmented.svelte';

	let view = $state('table');
	let two = $state('light');
	const three = $derived([
		{ value: 'table', label: $t('ui.segmented.sample.table') },
		{ value: 'board', label: $t('ui.segmented.sample.board') },
		{ value: 'calendar', label: $t('ui.segmented.sample.calendar') }
	]);
	const withDisabled = $derived([
		{ value: 'table', label: $t('ui.segmented.sample.table') },
		{
			value: 'board',
			label: $t('ui.segmented.sample.board'),
			disabled: true,
			disabledReason: $t('ui.gallery.reason')
		},
		{ value: 'calendar', label: $t('ui.segmented.sample.calendar') }
	]);
	const pair = $derived([
		{ value: 'light', label: $t('ui.segmented.sample.light') },
		{ value: 'dark', label: $t('ui.segmented.sample.dark') }
	]);
</script>

<section data-gallery-section="segmented">
	<h2>segmented</h2>
	<div class="col">
		<div data-gallery="segmented:three">
			<Segmented options={three} bind:value={view} label={$t('ui.segmented.sample.label')} />
		</div>
		<div data-gallery="segmented:two">
			<Segmented options={pair} bind:value={two} label={$t('ui.segmented.sample.label')} />
		</div>
		<div data-gallery="segmented:disabled-segment">
			<Segmented options={withDisabled} value="table" label={$t('ui.segmented.sample.label')} />
		</div>
		<div data-gallery="segmented:disabled">
			<Segmented options={pair} value="light" disabled label={$t('ui.segmented.sample.label')} />
		</div>
	</div>
</section>

<style>
	.col {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
</style>
