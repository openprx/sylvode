<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '../button/Button.svelte';
	import Field from './Field.svelte';
	import SubmitReason from './SubmitReason.svelte';
	import { FieldState, FormState } from './field-state';

	const missing = (n: number) =>
		['title', 'due', 'owner', 'status', 'labels']
			.slice(0, n)
			.map((id) => ({ id, label: $t(`ui.form.sample.${id}`) }));

	// One live form: the title is validated on blur, then on every input.
	const form = new FormState('required');
	const title = form.register(
		'gallery-form-title',
		$t('ui.form.sample.title'),
		new FieldState<string>('', (v) => (v.trim() ? null : $t('ui.form.sample.titleError')), {
			required: true
		})
	);
	const missingNow = $derived(form.missing());
</script>

<section data-gallery-section="form">
	<h2>form</h2>
	<div class="col">
		<div data-gallery="form:required">
			<Field
				id="gallery-form-required"
				label={$t('ui.form.sample.title')}
				requirement="required"
				marking="required"
				helper={$t('ui.gallery.sample')}
			>
				{#snippet children({ describedBy, invalid, required })}
					<input
						id="gallery-form-required"
						aria-describedby={describedBy}
						aria-invalid={invalid || undefined}
						aria-required={required}
					/>
				{/snippet}
			</Field>
		</div>
		<div data-gallery="form:optional">
			<Field
				id="gallery-form-optional"
				label={$t('ui.form.sample.owner')}
				requirement="optional"
				marking="optional"
			>
				{#snippet children({ describedBy })}
					<input id="gallery-form-optional" aria-describedby={describedBy} />
				{/snippet}
			</Field>
		</div>
		<div data-gallery="form:error">
			<Field
				id="gallery-form-error"
				label={$t('ui.form.sample.title')}
				error={$t('ui.form.sample.titleError')}
			>
				{#snippet children({ describedBy, invalid })}
					<input
						id="gallery-form-error"
						class="invalid"
						aria-describedby={describedBy}
						aria-invalid={invalid || undefined}
						value="x"
					/>
				{/snippet}
			</Field>
		</div>
		<div data-gallery="form:live">
			<Field
				id="gallery-form-title"
				label={$t('ui.form.sample.title')}
				requirement="required"
				marking="required"
				error={title.shownError}
			>
				{#snippet children({ describedBy, invalid, required })}
					<input
						id="gallery-form-title"
						class:invalid
						aria-describedby={describedBy}
						aria-invalid={invalid || undefined}
						aria-required={required}
						value={title.value}
						oninput={(e) => title.input(e.currentTarget.value)}
						onblur={() => title.blur()}
					/>
				{/snippet}
			</Field>
		</div>
		<div data-gallery="form:reason" class="row">
			<SubmitReason missing={missingNow} action={$t('ui.form.sample.action')} />
			<Button variant="primary" disabled={!form.valid}>{$t('ui.gallery.confirm')}</Button>
		</div>
		<div data-gallery="form:reason-many">
			<SubmitReason missing={missing(5)} action={$t('ui.form.sample.action')} />
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
	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-5);
	}
	input {
		box-sizing: border-box;
		height: var(--rf-field-height);
		padding: 0 var(--rf-space-5);
		border: var(--rf-border-width) solid var(--rf-input-border);
		border-radius: var(--rf-radius-input);
		background: var(--rf-input);
		color: var(--rf-text);
		font: inherit;
		font-size: var(--rf-size-body);
	}
	input:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	input.invalid {
		background: var(--rf-danger-bg);
		border-color: var(--rf-danger);
	}
</style>
