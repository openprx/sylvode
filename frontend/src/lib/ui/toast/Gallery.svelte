<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '../button/Button.svelte';
	import { appToasts as toasts } from './app-toasts';

	let undoCalls = $state(0);
	let failNext = $state(false);

	function showNeutral() {
		toasts.show({ message: $t('ui.toast.sampleNeutral') });
	}
	function showResult() {
		toasts.result($t('ui.toast.sampleResult'), { href: '#result' });
	}
	function showUndo() {
		toasts.undo(
			$t('ui.toast.sampleDeleted'),
			async () => {
				undoCalls += 1;
				if (failNext) throw new Error('server did not respond');
			},
			() => $t('ui.toast.sampleUndoFailed')
		);
	}
</script>

<section data-gallery-section="toast">
	<h2>toast</h2>
	<div class="row">
		<span data-gallery="toast:neutral"
			><Button onclick={showNeutral}>{$t('ui.toast.showNeutral')}</Button></span
		>
		<span data-gallery="toast:result"
			><Button onclick={showResult}>{$t('ui.toast.showResult')}</Button></span
		>
		<span data-gallery="toast:undo"
			><Button onclick={showUndo}>{$t('ui.toast.showUndo')}</Button></span
		>
		<label data-gallery="toast:field"
			><span>{$t('ui.toast.field')}</span> <input type="text" /></label
		>
		<label data-gallery="toast:fail"
			><input type="checkbox" bind:checked={failNext} /> {$t('ui.toast.retry')}</label
		>
		<output data-gallery="toast:calls" data-calls={undoCalls}
			>{$t('ui.toast.undoCalls', { values: { count: undoCalls } })}</output
		>
	</div>
</section>

<style>
	.row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
		align-items: center;
	}
</style>
