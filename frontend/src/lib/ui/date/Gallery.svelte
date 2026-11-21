<script lang="ts">
	import { t } from 'svelte-i18n';
	import DatePicker from './DatePicker.svelte';
	import DateRangePicker from './DateRangePicker.svelte';
	import type { DateRange } from './calendar';

	let date = $state<string | null>('2026-11-24');
	let empty = $state<string | null>(null);
	let stamp = $state<string | null>('2026-11-24T14:30');
	let range = $state<DateRange | null>({ start: '2026-11-10', end: '2026-11-18' });
</script>

<section data-gallery-section="date">
	<h2>date</h2>
	<div class="col">
		<div data-gallery="date:default">
			<DatePicker label={$t('ui.date.sample.due')} value={date} onchange={(v) => (date = v)} />
		</div>
		<div data-gallery="date:empty">
			<DatePicker label={$t('ui.date.sample.due')} value={empty} onchange={(v) => (empty = v)} />
		</div>
		<div data-gallery="date:datetime">
			<DatePicker
				kind="datetime"
				label={$t('ui.date.sample.starts')}
				value={stamp}
				onchange={(v) => (stamp = v)}
				zoneLabel={$t('ui.date.sample.zone')}
			/>
		</div>
		<div data-gallery="date:bounded">
			<DatePicker
				label={$t('ui.date.sample.due')}
				value={null}
				onchange={() => {}}
				min="2026-11-10"
				max="2026-11-20"
			/>
		</div>
		<div data-gallery="date:invalid">
			<DatePicker
				label={$t('ui.date.sample.due')}
				value={null}
				onchange={() => {}}
				required
				marking="required"
				error={$t('ui.date.sample.required')}
			/>
		</div>
		<div data-gallery="date:disabled">
			<DatePicker
				label={$t('ui.date.sample.due')}
				value="2026-11-24"
				onchange={() => {}}
				disabled
				disabledReason={$t('ui.gallery.reason')}
			/>
		</div>
		<div data-gallery="date:readonly">
			<DatePicker
				label={$t('ui.date.sample.due')}
				value="2026-11-24"
				onchange={() => {}}
				readonly
			/>
		</div>
		<div data-gallery="date:compact">
			<DatePicker
				label={$t('ui.date.sample.due')}
				value="2026-11-24"
				onchange={() => {}}
				density="compact"
			/>
		</div>
		<div data-gallery="date:range">
			<DateRangePicker
				label={$t('ui.date.sample.period')}
				value={range}
				onchange={(v) => (range = v)}
			/>
		</div>
	</div>
</section>

<style>
	.col {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		max-width: 360px;
	}
</style>
