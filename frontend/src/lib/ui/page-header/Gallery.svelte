<script lang="ts">
	import { t } from 'svelte-i18n';
	import PageHeader from './PageHeader.svelte';
	import type { SurfaceAction } from './page-header';

	let log = $state('');
	const noop = () => {};
	const context = $derived({
		segments: [
			{ id: 'project', label: $t('ui.contextLine.sampleProject'), href: '#project' },
			{ id: 'forms', label: $t('ui.contextLine.sampleForms'), href: '#forms' }
		],
		current: $t('ui.pageHeader.sampleTitle')
	});
	const primary = $derived<SurfaceAction>({
		id: 'new',
		label: $t('ui.pageHeader.sampleNew'),
		kind: 'primary',
		run: () => {
			log = 'new';
		}
	});
	const disabledPrimary = $derived<SurfaceAction>({
		...primary,
		disabledReason: $t('ui.pageHeader.sampleReason')
	});
	const secondary = $derived<SurfaceAction[]>([
		{
			id: 'export',
			label: $t('ui.pageHeader.sampleExport'),
			kind: 'secondary',
			run: () => {
				log = 'export';
			}
		},
		{ id: 'import', label: $t('ui.pageHeader.sampleImport'), kind: 'secondary', run: noop }
	]);
</script>

<section data-gallery-section="page-header" data-log={log}>
	<h2>page-header</h2>
	<div class="stack">
		<div data-gallery="page-header:standard" class="wide">
			<PageHeader title={$t('ui.pageHeader.sampleTitle')} {context} {primary} {secondary} />
		</div>
		<div data-gallery="page-header:compact" class="wide">
			<PageHeader
				title={$t('ui.pageHeader.sampleTitle')}
				variant="compact"
				{context}
				{primary}
				{secondary}
			/>
		</div>
		<div data-gallery="page-header:compact-600" class="w600">
			<PageHeader
				title={$t('ui.pageHeader.sampleTitle')}
				variant="compact"
				{context}
				{primary}
				{secondary}
			/>
		</div>
		<div data-gallery="page-header:compact-390" class="w390">
			<PageHeader
				title={$t('ui.pageHeader.sampleTitle')}
				variant="compact"
				{context}
				{primary}
				{secondary}
			/>
		</div>
		<div data-gallery="page-header:disabled-primary" class="wide">
			<PageHeader
				title={$t('ui.pageHeader.sampleTitle')}
				variant="compact"
				primary={disabledPrimary}
			/>
		</div>
		<div data-gallery="page-header:loading" class="wide">
			<PageHeader title="" variant="compact" loading />
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
	.w600 {
		width: 600px;
	}
	.w390 {
		width: 390px;
	}
</style>
