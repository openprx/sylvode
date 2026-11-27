<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { flowApi } from '#lib/api/flow.js';
	import { formsApi } from '#lib/api/forms.js';
	import { layoutApi } from '#lib/layout/api.js';
	import { layoutSlot, projectDiscovery } from '#lib/plugins/contributions.js';
	import { projectOptionsStore } from '#lib/stores/project-options.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import type { SelectOption } from '#lib/ui/select/listbox.js';
	import Switch from '#lib/ui/switch/Switch.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import { builtinInfo } from '../builtins';
	import type { LocaleLabels, NavDoc, NavEntryDoc, NavScope, NavTarget } from '../types';
	import { ICONS, cleanLabels, externalUrlOk, freeBuiltins, targetKinds, uniqueId } from './logic';

	/**
	 * Adds or edits one entry. The target is chosen with a picker per kind; what the pickers list
	 * is only a convenience, the server checks every target again on save.
	 */
	interface Props {
		open: boolean;
		scope: NavScope;
		/** Workspace id for the workspace navigation, project id for a project navigation. */
		scopeId: string;
		workspaceId: string;
		doc: NavDoc;
		/** The entry being edited; null adds a new one. */
		entry: NavEntryDoc | null;
		onsave: (entry: NavEntryDoc, name: string | null) => void;
		onclose: () => void;
	}
	let { open, scope, scopeId, workspaceId, doc, entry, onsave, onclose }: Props = $props();

	type Kind = NavTarget['kind'];
	const kinds = $derived(targetKinds(scope));

	let kind = $state<Kind>('builtin');
	let page = $state<string | null>(null);
	let projectId = $state<string | null>(null);
	let formProject = $state<string | null>(null);
	let formId = $state<string | null>(null);
	let flowId = $state<string | null>(null);
	let flowQuery = $state('');
	let layoutId = $state<string | null>(null);
	let layoutProblem = $state<string | null>(null);
	let appKey = $state<string | null>(null);
	let url = $state('');
	let labelEn = $state('');
	let labelZh = $state('');
	let icon = $state<string | null>(null);
	let hidden = $state(false);
	let pinned = $state(false);
	let locked = $state(false);

	let forms = $state.raw<SelectOption[]>([]);
	let flowHits = $state.raw<SelectOption[]>([]);
	let layouts = $state.raw<SelectOption[]>([]);
	let apps = $state.raw<SelectOption[]>([]);
	let appsBy = $state.raw<Record<string, { key: string; route: string }>>({});

	// Load the draft from the entry (or blanks) each time the dialog opens.
	$effect(() => {
		if (!open) return;
		const e = entry;
		kind = e?.target.kind ?? 'builtin';
		page = e?.target.kind === 'builtin' ? e.target.page : null;
		projectId = e?.target.kind === 'project' ? e.target.project_id : null;
		formId = e?.target.kind === 'form' ? e.target.form_id : null;
		formProject = scope === 'project' ? scopeId : null;
		flowId = e?.target.kind === 'flow_page' ? e.target.object_id : null;
		flowQuery = '';
		layoutId = e?.target.kind === 'layout_page' ? e.target.layout_id : null;
		layoutProblem = null;
		appKey = e?.target.kind === 'plugin_app' ? `${e.target.package_key}\n${e.target.route}` : null;
		url = e?.target.kind === 'external' ? e.target.url : '';
		labelEn = e?.label?.en ?? '';
		labelZh = e?.label?.zh ?? '';
		icon = e?.icon ?? null;
		hidden = e?.hidden ?? false;
		pinned = e?.pinned ?? false;
		locked = e?.locked ?? false;
		flowHits = [];
	});

	// --- pickers ---
	const projectOptions = $derived(
		$projectOptionsStore.items
			.filter((p) => p.workspace_id === workspaceId)
			.map((p) => ({ value: p.id, label: p.name }))
	);
	const builtinOptions = $derived(
		freeBuiltins(doc, scope, entry?.target.kind === 'builtin' ? entry.target.page : undefined).map(
			(p) => ({ value: p, label: $t(builtinInfo(p)?.labelKey ?? p) })
		)
	);

	$effect(() => {
		if (!open || kind !== 'form' || !formProject) {
			forms = [];
			return;
		}
		let cancelled = false;
		void formsApi.list(formProject, { per_page: 200 }).then((result) => {
			if (cancelled || result.code !== 0 || !result.data) return;
			forms = result.data.items.map((f) => ({ value: f.id, label: f.name }));
		});
		return () => {
			cancelled = true;
		};
	});

	$effect(() => {
		const q = flowQuery.trim();
		if (!open || kind !== 'flow_page' || q.length < 2) return;
		const controller = new AbortController();
		const timer = setTimeout(() => {
			void flowApi
				.searchObjects(
					workspaceId,
					{
						q,
						object_type: 'page',
						...(scope === 'project' ? { project_id: scopeId } : { all_visible: true })
					},
					controller.signal
				)
				.then((result) => {
					if (result.code !== 0 || !result.data) return;
					flowHits = result.data.items.map((hit) => ({
						value: hit.object.id,
						label: hit.object.title || hit.object.id
					}));
				})
				.catch(() => {});
		}, 300);
		return () => {
			clearTimeout(timer);
			controller.abort();
		};
	});

	$effect(() => {
		if (!open || kind !== 'layout_page') return;
		let cancelled = false;
		void layoutApi
			.listLayouts(scope === 'workspace' ? 'workspace' : 'project', scopeId)
			.then((result) => {
				if (cancelled || result.code !== 0 || !result.data) return;
				layouts = result.data.items
					.filter((l) => l.status === 'active' && !l.installation_id)
					.map((l) => ({ value: l.id, label: l.layout_key }));
			});
		return () => {
			cancelled = true;
		};
	});

	/** A layout must fit the `page` slot; the check is the server's validator. */
	async function chooseLayout(id: string | null) {
		layoutId = id;
		layoutProblem = null;
		if (!id) return;
		const record = await layoutApi.getLayout(id);
		if (record.code !== 0 || !record.data) return;
		const verdict = await layoutApi.validate({ spec: record.data.spec, slot_id: 'page' });
		if (verdict.code === 0 && verdict.data && !verdict.data.valid && layoutId === id)
			layoutProblem = $t('nav.editor.layoutNotPage');
	}

	$effect(() => {
		if (!open || kind !== 'plugin_app' || scope !== 'project') return;
		const current = projectDiscovery(scopeId);
		const stop = current.subscribe((state) => {
			if (state.status !== 'ready') return;
			const layout = layoutSlot(state.items, 'project.view');
			const items = [...layout.inline, ...layout.more, ...layout.hidden];
			apps = items.map((item) => ({
				value: `${item.key}\n${item.contribution.route ?? ''}`,
				label: item.contribution.label
			}));
			appsBy = Object.fromEntries(
				items.map((item) => [
					`${item.key}\n${item.contribution.route ?? ''}`,
					{ key: item.key, route: item.contribution.route ?? '' }
				])
			);
		});
		const release = current.retain();
		return () => {
			stop();
			release();
		};
	});

	// --- validation and result ---
	const urlError = $derived(
		kind === 'external' && url !== '' && !externalUrlOk(url)
			? $t('nav.editor.urlInvalid')
			: undefined
	);
	const target = $derived.by((): { target: NavTarget; name: string | null } | null => {
		switch (kind) {
			case 'builtin':
				return page ? { target: { kind, page }, name: null } : null;
			case 'project': {
				const name = projectOptions.find((p) => p.value === projectId)?.label ?? null;
				return projectId ? { target: { kind, project_id: projectId }, name } : null;
			}
			case 'form': {
				const name = forms.find((f) => f.value === formId)?.label ?? null;
				return formId ? { target: { kind, form_id: formId }, name } : null;
			}
			case 'flow_page': {
				const name = flowHits.find((f) => f.value === flowId)?.label ?? null;
				return flowId ? { target: { kind, object_id: flowId }, name } : null;
			}
			case 'layout_page': {
				const name = layouts.find((l) => l.value === layoutId)?.label ?? null;
				return layoutId && !layoutProblem ? { target: { kind, layout_id: layoutId }, name } : null;
			}
			case 'plugin_app': {
				const app = appKey ? appsBy[appKey] : undefined;
				const name = apps.find((a) => a.value === appKey)?.label ?? null;
				return app ? { target: { kind, package_key: app.key, route: app.route }, name } : null;
			}
			case 'external':
				return externalUrlOk(url) ? { target: { kind, url }, name: null } : null;
		}
	});
	const canSave = $derived(target !== null);

	function save() {
		if (!target) return;
		const labels: LocaleLabels = { en: labelEn, zh: labelZh };
		const base = cleanLabels(labels);
		const id = entry?.id ?? uniqueId(doc, base?.en ?? target.name ?? target.target.kind);
		onsave({ id, target: target.target, label: base, icon, hidden, pinned, locked }, target.name);
	}

	const kindOptions = $derived(
		kinds.map((k) => ({ value: k, label: $t(`nav.editor.kinds.${k}`) }))
	);
	const iconOptions = $derived(ICONS.map((i) => ({ value: i, label: i })));
	const projectForFormOptions = $derived(projectOptions);
</script>

<Dialog
	{open}
	title={entry ? $t('nav.editor.editEntry') : $t('nav.editor.addEntry')}
	{onclose}
	size="wide"
	testId="nav-entry-dialog"
>
	<div class="form">
		<Select
			label={$t('nav.editor.kind')}
			value={kind}
			options={kindOptions}
			disabled={entry !== null}
			onchange={(v) => v && (kind = v as Kind)}
		/>

		{#if kind === 'builtin'}
			<Select
				label={$t('nav.editor.builtin')}
				value={page}
				options={builtinOptions}
				onchange={(v) => (page = v)}
			/>
		{:else if kind === 'project'}
			<Select
				label={$t('nav.editor.project')}
				value={projectId}
				options={projectOptions}
				onchange={(v) => (projectId = v)}
			/>
		{:else if kind === 'form'}
			{#if scope === 'workspace'}
				<Select
					label={$t('nav.editor.project')}
					value={formProject}
					options={projectForFormOptions}
					onchange={(v) => {
						formProject = v;
						formId = null;
					}}
				/>
			{/if}
			<Select
				label={$t('nav.editor.form')}
				value={formId}
				options={forms}
				disabled={!formProject}
				onchange={(v) => (formId = v)}
			/>
		{:else if kind === 'flow_page'}
			<TextField label={$t('nav.editor.flowSearch')} bind:value={flowQuery} />
			<Select
				label={$t('nav.editor.flowPage')}
				value={flowId}
				options={flowHits}
				onchange={(v) => (flowId = v)}
			/>
		{:else if kind === 'layout_page'}
			<Select
				label={$t('nav.editor.layoutPage')}
				value={layoutId}
				options={layouts}
				error={layoutProblem ?? undefined}
				onchange={(v) => void chooseLayout(v)}
			/>
		{:else if kind === 'plugin_app'}
			<Select
				label={$t('nav.editor.app')}
				value={appKey}
				options={apps}
				onchange={(v) => (appKey = v)}
			/>
		{:else if kind === 'external'}
			<SecurityWarning title={$t('nav.editor.externalTitle')}>
				{$t('nav.editor.externalBody')}
			</SecurityWarning>
			<TextField
				label={$t('nav.editor.url')}
				type="url"
				bind:value={url}
				error={urlError}
				helper={$t('nav.editor.urlHelper')}
			/>
		{/if}

		<div class="pair">
			<TextField
				label={$t('nav.editor.labelEn')}
				bind:value={labelEn}
				maxlength={64}
				helper={$t('nav.editor.labelHelper')}
			/>
			<TextField label={$t('nav.editor.labelZh')} bind:value={labelZh} maxlength={64} />
		</div>
		<Select
			label={$t('nav.editor.icon')}
			value={icon}
			options={iconOptions}
			allowEmpty
			onchange={(v) => (icon = v)}
		/>
		<div class="switches">
			<Switch bind:checked={hidden} label={$t('nav.editor.hidden')} />
			<Switch bind:checked={pinned} label={$t('nav.editor.pinned')} />
			<Switch bind:checked={locked} label={$t('nav.editor.locked')} />
		</div>
		<p class="note">{$t('nav.editor.lockedNote')}</p>
	</div>
	{#snippet actions()}
		<Button variant="text" onclick={onclose}>{$t('nav.editor.cancel')}</Button>
		<Button
			variant="primary"
			disabled={!canSave}
			disabledReason={$t('nav.editor.chooseTarget')}
			onclick={save}>{$t('nav.editor.apply')}</Button
		>
	{/snippet}
</Dialog>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
	}
	.pair {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--rf-space-6);
	}
	.switches {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-8);
	}
	.note {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
	}
</style>
