<svelte:options runes />

<script lang="ts">
	import { untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { authStore } from '#lib/stores/auth.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import Textarea from '#lib/ui/textarea/Textarea.svelte';
	import { layoutApi } from '../api';
	import LayoutView from '../LayoutView.svelte';
	import type {
		LayoutSpec,
		LayoutSummary,
		LayoutValidationError,
		ScopeKind,
		SlotContextParams,
		SlotDescriptor
	} from '../types';
	import {
		errorLines,
		offeredScopes,
		parseSpec,
		placeable,
		refusalLines,
		starterSpec,
		type OfferedScope
	} from './logic';

	/**
	 * "Create here": the per-slot panel that puts a layout into slot `slotId`. Choose a layout that
	 * already exists in the scope, or write one as JSON (server validate on every pause, live
	 * preview through the renderer), then save it and create the placement. Nothing here grants
	 * anything: every write is decided by the server.
	 */
	let {
		slotId,
		context,
		workspaceId,
		descriptor,
		onsaved,
		onclose
	}: {
		slotId: string;
		context: SlotContextParams;
		workspaceId: string;
		descriptor: SlotDescriptor;
		onsaved: () => void;
		onclose: () => void;
	} = $props();

	type Mode = 'existing' | 'json';

	const scopes = $derived(offeredScopes(descriptor, context, workspaceId, $authStore.user?.role));
	let scopeKey = $state<string | null>(null);
	const scope = $derived<OfferedScope | null>(
		scopes.find((item) => `${item.kind}:${item.id}` === scopeKey) ?? scopes[0] ?? null
	);

	let mode = $state<Mode>('existing');
	let condition = $state('');
	let busy = $state(false);
	let failure = $state.raw<string[]>([]);

	// Existing layouts: the chosen scope and the workspace above it (a workspace layout can be
	// placed at a project).
	let existing = $state.raw<LayoutSummary[]>([]);
	let existingLoading = $state(false);
	let pickedId = $state<string | null>(null);
	let pickedSpec = $state.raw<LayoutSpec | null>(null);

	// JSON editor.
	let jsonText = $state(
		untrack(() => JSON.stringify(starterSpec(descriptor, Date.now().toString(36)), null, 2))
	);
	let verdict = $state.raw<{ valid: boolean; errors: LayoutValidationError[] } | null>(null);
	let syntaxError = $state(false);
	let validating = $state(false);

	const parsed = $derived(parseSpec(jsonText));
	const jsonSpec = $derived(
		parsed.ok && verdict?.valid ? (parsed.spec as unknown as LayoutSpec) : null
	);
	const previewSpec = $derived(mode === 'json' ? jsonSpec : pickedSpec);
	const previewKey = $derived(mode === 'json' ? jsonText : (pickedId ?? ''));

	$effect(() => {
		const current = scope;
		if (!current) {
			existing = [];
			return;
		}
		let cancelled = false;
		existingLoading = true;
		const kinds: ScopeKind[] = [current.kind];
		const scopeIds: Record<string, string> = { [current.kind]: current.id };
		if (current.kind !== 'workspace' && descriptor.scope_kinds.includes('workspace')) {
			kinds.push('workspace');
			scopeIds.workspace = workspaceId;
		}
		void Promise.all(kinds.map((kind) => layoutApi.listLayouts(kind, scopeIds[kind])))
			.then((results) => {
				if (cancelled) return;
				existing = placeable(
					results.flatMap((result) => (result.code === 0 && result.data ? result.data.items : []))
				);
			})
			.catch(() => {
				if (!cancelled) existing = [];
			})
			.finally(() => {
				if (!cancelled) existingLoading = false;
			});
		return () => {
			cancelled = true;
		};
	});

	let pickToken = 0;
	async function pick(id: string | null) {
		pickedId = id;
		pickedSpec = null;
		failure = [];
		if (!id) return;
		const token = ++pickToken;
		const result = await layoutApi.getLayout(id);
		if (token !== pickToken) return;
		if (result.code === 0 && result.data) pickedSpec = result.data.spec;
		else failure = refusalLines(result);
	}

	let validateToken = 0;
	$effect(() => {
		if (mode !== 'json') return;
		const text = jsonText;
		const current = scope;
		const attempt = parseSpec(text);
		if (!attempt.ok) {
			syntaxError = true;
			verdict = null;
			return;
		}
		syntaxError = false;
		const token = ++validateToken;
		validating = true;
		const timer = setTimeout(() => {
			layoutApi
				.validate({
					spec: attempt.spec,
					slot_id: slotId,
					...(current ? { scope_kind: current.kind, scope_id: current.id } : {})
				})
				.then((result) => {
					if (token !== validateToken) return;
					verdict =
						result.code === 0 && result.data
							? result.data
							: {
									valid: false,
									errors: [{ path: '', code: result.error_code ?? 'request_failed' }]
								};
				})
				.catch(() => {
					if (token === validateToken) {
						verdict = { valid: false, errors: [{ path: '', code: 'request_failed' }] };
					}
				})
				.finally(() => {
					if (token === validateToken) validating = false;
				});
		}, 400);
		return () => clearTimeout(timer);
	});

	const canPlace = $derived(
		!busy &&
			scope !== null &&
			(mode === 'existing' ? pickedId !== null : jsonSpec !== null && !validating)
	);

	async function place() {
		if (!scope || busy) return;
		busy = true;
		failure = [];
		try {
			let layoutId = pickedId;
			if (mode === 'json') {
				if (!parsed.ok) return;
				const saved = await layoutApi.createLayout({
					scope_kind: scope.kind,
					scope_id: scope.id,
					spec: parsed.spec
				});
				if (saved.code !== 0 || !saved.data) {
					failure = refusalLines(saved);
					return;
				}
				layoutId = saved.data.id;
			}
			if (!layoutId) return;
			const trimmed = condition.trim();
			const placed = await layoutApi.createPlacement({
				slot_id: slotId,
				scope_kind: scope.kind,
				scope_id: scope.id,
				layout_id: layoutId,
				...(trimmed ? { condition: trimmed } : {})
			});
			if (placed.code !== 0) {
				failure = refusalLines(placed);
				if (mode === 'json') {
					// The layout is saved; from here on it is listed under "Use a layout".
					mode = 'existing';
					await pick(layoutId);
				}
				return;
			}
			onsaved();
		} catch {
			failure = ['request_failed'];
		} finally {
			busy = false;
		}
	}

	const scopeOptions = $derived(
		scopes.map((item) => ({
			value: `${item.kind}:${item.id}`,
			label: $t(`layout.editor.scope.${item.kind}`)
		}))
	);
	const layoutOptions = $derived(
		existing.map((item) => ({ value: item.id, label: item.layout_key, meta: item.scope_kind }))
	);
</script>

<div
	class="min-w-0 rounded-card border border-line bg-raised p-3"
	data-layout-editor={slotId}
	role="group"
	aria-label={$t('layout.editor.title')}
>
	<div class="mb-3 flex flex-wrap items-center justify-between gap-2">
		<h3 class="text-sm font-semibold text-fg">{$t('layout.editor.title')}</h3>
		<button type="button" class="text-xs text-fg-muted hover:text-fg" onclick={onclose}>
			{$t('layout.editor.close')}
		</button>
	</div>

	{#if scopes.length === 0}
		<p class="text-sm text-fg-muted">{$t('layout.editor.noScope')}</p>
	{:else}
		<div class="mb-3 flex flex-wrap items-end gap-3">
			<div class="min-w-48">
				<Select
					label={$t('layout.editor.scopeLabel')}
					value={scope ? `${scope.kind}:${scope.id}` : null}
					options={scopeOptions}
					onchange={(value) => (scopeKey = value)}
				/>
			</div>
			<div class="flex gap-1" role="group" aria-label={$t('layout.editor.modeLabel')}>
				<Button
					variant={mode === 'existing' ? 'primary' : 'secondary'}
					aria-pressed={mode === 'existing'}
					onclick={() => (mode = 'existing')}
				>
					{$t('layout.editor.modeExisting')}
				</Button>
				<Button
					variant={mode === 'json' ? 'primary' : 'secondary'}
					aria-pressed={mode === 'json'}
					onclick={() => (mode = 'json')}
				>
					{$t('layout.editor.modeJson')}
				</Button>
				<Button variant="secondary" disabled disabledReason={$t('layout.editor.marketLater')}>
					{$t('layout.editor.modeMarket')}
				</Button>
			</div>
		</div>

		<div class="grid min-w-0 gap-3 lg:grid-cols-2">
			<div class="min-w-0">
				{#if mode === 'existing'}
					{#if existingLoading}
						<p class="text-sm text-fg-muted">{$t('layout.editor.loading')}</p>
					{:else if layoutOptions.length === 0}
						<p class="text-sm text-fg-muted" data-layout-editor-empty>
							{$t('layout.editor.noLayouts')}
						</p>
					{:else}
						<Select
							label={$t('layout.editor.pickLabel')}
							value={pickedId}
							options={layoutOptions}
							onchange={(value) => void pick(value)}
						/>
					{/if}
				{:else}
					<Textarea
						label={$t('layout.editor.jsonLabel')}
						rows={16}
						spellcheck={false}
						bind:value={jsonText}
					/>
					<div class="mt-2 text-xs" aria-live="polite" data-layout-editor-verdict>
						{#if syntaxError}
							<p class="text-danger">{$t('layout.editor.syntax')}</p>
						{:else if validating}
							<p class="text-fg-muted">{$t('layout.editor.validating')}</p>
						{:else if verdict?.valid}
							<p class="text-success">{$t('layout.editor.valid')}</p>
						{:else if verdict}
							<ul class="text-danger">
								{#each errorLines(verdict.errors) as line, index (index)}
									<li class="break-all font-mono">{line}</li>
								{/each}
							</ul>
						{/if}
					</div>
				{/if}
			</div>
			<div class="min-w-0 rounded-control border border-dashed border-line p-2" data-layout-preview>
				<p class="mb-1 text-xs uppercase text-fg-muted">{$t('layout.editor.preview')}</p>
				{#if previewSpec}
					{#key previewKey}
						<svelte:boundary>
							<LayoutView spec={previewSpec} {context} {workspaceId} installations={{}} />
							{#snippet failed()}
								<p class="text-xs text-fg-muted">{$t('layout.failed')}</p>
							{/snippet}
						</svelte:boundary>
					{/key}
				{:else}
					<p class="text-sm text-fg-muted">{$t('layout.editor.previewEmpty')}</p>
				{/if}
			</div>
		</div>

		<div class="mt-3 flex flex-wrap items-end gap-3">
			<div class="min-w-56 flex-1">
				<TextField
					label={$t('layout.editor.conditionLabel')}
					helper={$t('layout.editor.conditionHelp')}
					bind:value={condition}
					spellcheck={false}
				/>
			</div>
			<Button variant="primary" loading={busy} disabled={!canPlace} onclick={() => void place()}>
				{mode === 'json' ? $t('layout.editor.saveAndPlace') : $t('layout.editor.place')}
			</Button>
		</div>

		{#if failure.length > 0}
			<ul class="mt-2 text-xs text-danger" role="alert" data-layout-editor-error>
				{#each failure as line, index (index)}
					<li class="break-all font-mono">{line}</li>
				{/each}
			</ul>
		{/if}
	{/if}
</div>
