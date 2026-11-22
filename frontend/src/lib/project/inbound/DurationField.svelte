<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Select from '#lib/ui/select/Select.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import {
		DURATION_UNITS,
		changeUnit,
		toDuration,
		toSeconds,
		type Duration,
		type DurationUnit
	} from './duration';

	interface Props {
		label: string;
		/** The stored length in seconds. The editor opens on the largest exact unit. */
		seconds: number;
		/** Called with the new length in seconds, or `null` while the amount is not a whole number. */
		onchange: (seconds: number | null) => void;
		helper?: string;
		error?: string;
		disabled?: boolean;
		id?: string;
	}
	let { label, seconds, onchange, helper, error, disabled = false, id }: Props = $props();

	// The initial value only: the field owns the draft afterwards, so editing never rewrites it.
	// svelte-ignore state_referenced_locally
	let draft = $state<Duration>(toDuration(seconds));
	// svelte-ignore state_referenced_locally
	let text = $state(String(draft.amount));

	function report(next: Duration, raw: string) {
		const parsed = raw.trim() === '' ? NaN : Number(raw);
		onchange(Number.isFinite(parsed) ? toSeconds(parsed, next.unit) : null);
	}

	function amountChanged(raw: string) {
		text = raw;
		const parsed = raw.trim() === '' ? NaN : Number(raw);
		draft = { amount: Number.isFinite(parsed) ? parsed : draft.amount, unit: draft.unit };
		report(draft, raw);
	}

	function unitChanged(unit: DurationUnit | null) {
		if (unit === null) return;
		const raw = text;
		const parsed = raw.trim() === '' ? NaN : Number(raw);
		draft = Number.isFinite(parsed)
			? changeUnit({ amount: parsed, unit: draft.unit }, unit)
			: { ...draft, unit };
		text = String(draft.amount);
		report(draft, text);
	}
</script>

<div class="duration">
	<div class="amount">
		<TextField
			{label}
			{helper}
			{error}
			{disabled}
			{id}
			type="number"
			min="0"
			value={text}
			oninput={(event) => amountChanged(event.currentTarget.value)}
		/>
	</div>
	<div class="unit">
		<Select
			label={$t('inbound.rules.unit.label')}
			labelHidden
			{disabled}
			value={draft.unit}
			options={DURATION_UNITS.map((unit) => ({
				value: unit,
				label: $t(`inbound.rules.unit.${unit}`)
			}))}
			onchange={unitChanged}
		/>
	</div>
</div>

<style>
	.duration {
		display: flex;
		align-items: flex-start;
		gap: var(--rf-space-2);
	}
	.amount {
		width: 120px;
	}
	.unit {
		width: 140px;
		/* the unit select sits level with the number box, below the label line */
		padding-top: var(--rf-space-6);
	}
</style>
