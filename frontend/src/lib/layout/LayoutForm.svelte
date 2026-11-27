<svelte:options runes />

<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import { useRuntime } from './context';
	import { resolveText } from './refs';

	interface FormFieldDef {
		key: string;
		label: string;
		type?: string;
		required?: boolean;
		read_only?: boolean;
		options?: string[];
	}

	let { props }: { props: Record<string, unknown> } = $props();

	const runtime = useRuntime();
	const loc = $derived($locale ?? 'en');
	const sourceName = $derived(
		typeof props.data === 'string' ? props.data.replace(/^\$data\./, '') : ''
	);
	const submitName = $derived(typeof props.submit === 'string' ? props.submit : '');
	const fields = $derived.by(() => {
		const source = runtime.data[sourceName] as { fields?: FormFieldDef[] } | undefined;
		const all = source?.fields ?? [];
		const subset = Array.isArray(props.fields) ? (props.fields as string[]) : null;
		return all.filter((field) => !field.read_only && (!subset || subset.includes(field.key)));
	});

	let draft = $state<Record<string, unknown>>({});
	const unavailable = $derived(runtime.unavailable(submitName));
	const busy = $derived(runtime.actions[submitName]?.status === 'running');
	const failed = $derived(runtime.actions[submitName]?.status === 'error');

	function coerce(field: FormFieldDef, raw: unknown): unknown {
		if (raw === '' || raw === undefined) return undefined;
		if (['number', 'integer', 'amount', 'rating', 'progress'].includes(field.type ?? '')) {
			const number = Number(raw);
			return Number.isFinite(number) ? number : raw;
		}
		return raw;
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		const values: Record<string, unknown> = {};
		for (const field of fields) {
			const coerced = coerce(field, draft[field.key]);
			if (coerced !== undefined) values[field.key] = coerced;
		}
		if (await runtime.run(submitName, values)) draft = {};
	}

	const input = 'w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg';
</script>

<form class="flex min-w-0 flex-col gap-3" data-layout-node="form" onsubmit={submit}>
	{#each fields as field (field.key)}
		<label class="flex flex-col gap-1 text-sm">
			<span class="text-xs font-medium text-fg-muted"
				>{field.label}{field.required ? ' *' : ''}</span
			>
			{#if field.type === 'boolean'}
				<input
					type="checkbox"
					checked={draft[field.key] === true}
					onchange={(event) => (draft[field.key] = event.currentTarget.checked)}
				/>
			{:else if field.type === 'textarea' || field.type === 'rich_text'}
				<textarea
					class={input}
					rows="3"
					value={String(draft[field.key] ?? '')}
					oninput={(event) => (draft[field.key] = event.currentTarget.value)}></textarea>
			{:else if field.type === 'single_select' && field.options?.length}
				<select
					class={input}
					value={String(draft[field.key] ?? '')}
					onchange={(event) => (draft[field.key] = event.currentTarget.value)}
				>
					<option value=""></option>
					{#each field.options as option (option)}<option value={option}>{option}</option>{/each}
				</select>
			{:else}
				<input
					class={input}
					type={['number', 'integer', 'amount', 'rating', 'progress'].includes(field.type ?? '')
						? 'number'
						: field.type === 'date'
							? 'date'
							: field.type === 'datetime'
								? 'datetime-local'
								: 'text'}
					value={String(draft[field.key] ?? '')}
					oninput={(event) => (draft[field.key] = event.currentTarget.value)}
				/>
			{/if}
		</label>
	{/each}
	{#if failed}<p class="text-xs text-danger" role="alert">{$t('layout.actionFailed')}</p>{/if}
	<div title={unavailable ?? undefined}>
		<Button type="submit" variant="primary" disabled={unavailable !== null} loading={busy}>
			{props.submit_label
				? resolveText(props.submit_label, runtime.scope, loc)
				: $t('layout.submit')}
		</Button>
	</div>
</form>
