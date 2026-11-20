<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '../button/Button.svelte';
	import StripDemo from './StripDemo.svelte';

	const base = $derived([
		$t('ui.taskTabStrip.sampleForm'),
		$t('ui.taskTabStrip.sampleFlow'),
		$t('ui.taskTabStrip.sampleProject'),
		$t('ui.taskTabStrip.sampleLoading'),
		$t('ui.taskTabStrip.sampleIssues')
	]);
	const many = $derived([
		...base,
		$t('ui.taskTabStrip.sampleDraft'),
		`${$t('ui.taskTabStrip.sampleIssues')} 2`
	]);
	const eight = $derived([...many, `${$t('ui.taskTabStrip.sampleDraft')} 2`]);
	let blocking = $state(false);
</script>

<section data-gallery-section="task-tab-strip">
	<h2>task-tab-strip</h2>
	<div class="stack">
		<div data-gallery="task-tab-strip:default" class="wide">
			<StripDemo prefix="a" titles={base} dirty={[1]} changed={[2]} loading={[3]} />
		</div>
		<div data-gallery="task-tab-strip:overflow" class="mid">
			<StripDemo prefix="b" titles={many} dirty={[5]} />
		</div>
		<div data-gallery="task-tab-strip:collapsed" class="narrow">
			<StripDemo prefix="c" titles={base} />
		</div>
		<div data-gallery="task-tab-strip:blocking" class="wide">
			<StripDemo
				prefix="d"
				titles={eight}
				dirty={[0, 1, 2, 3, 4, 5, 6, 7]}
				pendingOpen={blocking}
			/>
			<span data-testid="tab-blocking-open">
				<Button onclick={() => (blocking = !blocking)}>{$t('ui.taskTabStrip.state')}</Button>
			</span>
		</div>
	</div>
</section>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
	}
	.wide {
		width: 900px;
		max-width: 100%;
	}
	.mid {
		width: 724px;
		max-width: 100%;
	}
	.narrow {
		width: 500px;
		max-width: 100%;
	}
</style>
