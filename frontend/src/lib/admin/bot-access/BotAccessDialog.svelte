<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { botsApi, type Bot } from '#lib/api/bots.js';
	import { projectsApi, type Project } from '#lib/api/projects.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import Modal from '#lib/components/Modal.svelte';
	import { toast } from '#lib/stores/toast.js';
	import {
		DEFAULT_LEVELS,
		GRANT_LEVELS,
		WORKSPACE_FORM,
		accessOutcome,
		accessRequest,
		formFromAccess,
		isGrantLevel,
		needsWritePermission,
		withGrant,
		withScope,
		type AccessForm,
		type ProjectScope
	} from './rules.js';

	interface Props {
		open: boolean;
		workspaceId: string;
		/** The token whose reach is edited. */
		bot: Bot | null;
		/** Called after a save, with nothing to read: the caller reloads its list. */
		onsaved: () => void;
	}

	let { open = $bindable(false), workspaceId, bot, onsaved }: Props = $props();

	type Phase = 'loading' | 'ready' | 'managed' | 'failed';
	let phase = $state<Phase>('loading');
	let failure = $state('');
	let form = $state<AccessForm>({ ...WORKSPACE_FORM, grants: {} });
	let projects = $state<Project[]>([]);
	let saving = $state(false);
	let saveError = $state('');

	const writeHint = $derived(needsWritePermission(form, bot?.permissions ?? []));

	// Reads the token's access and the workspace's projects each time the dialog opens.
	$effect(() => {
		if (!open || !bot) return;
		void load(bot);
	});

	async function load(target: Bot) {
		phase = 'loading';
		failure = '';
		saveError = '';
		const [access, listed] = await Promise.all([
			botsApi.getProjectAccess(workspaceId, target.id),
			projectsApi.list(workspaceId)
		]);
		if (!open || bot?.id !== target.id) return;
		if (access.code !== 0 || !access.data) {
			const outcome = accessOutcome(access);
			if (outcome.kind === 'managed') phase = 'managed';
			else {
				failure = describeResult(access);
				phase = 'failed';
			}
			return;
		}
		if (listed.code !== 0) {
			failure = describeResult(listed);
			phase = 'failed';
			return;
		}
		form = formFromAccess(access.data);
		projects = [...(listed.data?.items ?? [])].sort(
			(a, b) => a.key.localeCompare(b.key) || a.name.localeCompare(b.name)
		);
		phase = 'ready';
	}

	async function save() {
		if (saving || !bot || phase !== 'ready') return;
		saving = true;
		saveError = '';
		const response = await botsApi.putProjectAccess(
			workspaceId,
			bot.id,
			accessRequest(
				form,
				projects.map((project) => project.id)
			)
		);
		saving = false;
		if (response.code !== 0) {
			// Shown in the dialog: a toast would sit behind it.
			if (accessOutcome(response).kind === 'managed') phase = 'managed';
			else saveError = describeResult(response);
			return;
		}
		open = false;
		toast.success($t('botAccess.saved'));
		onsaved();
	}

	function pickScope(scope: ProjectScope) {
		form = withScope(form, scope);
	}
</script>

{#snippet footer()}
	<button
		type="button"
		onclick={() => (open = false)}
		class="rounded-control border border-fg px-4 py-2 text-sm text-fg hover:bg-surface"
	>
		{$t('common.cancel')}
	</button>
	{#if phase === 'ready'}
		<button
			type="button"
			onclick={save}
			disabled={saving}
			data-testid="bot-access-save"
			class="rounded-control rf-filled bg-primary px-4 py-2 text-sm text-on-primary disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
		>
			{saving ? $t('common.saving') : $t('common.save')}
		</button>
	{/if}
{/snippet}

<Modal
	bind:open
	title={$t('botAccess.title', { values: { name: bot?.name ?? '' } })}
	maxWidthClass="max-w-2xl"
	{footer}
>
	{#if phase === 'loading'}
		<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
	{:else if phase === 'managed'}
		<p class="text-sm text-fg" data-testid="bot-access-managed">{$t('botAccess.managed')}</p>
		<p class="mt-1 text-xs text-fg-muted">{$t('botAccess.managedHelp')}</p>
	{:else if phase === 'failed'}
		<p class="text-sm text-danger" role="alert">{failure}</p>
	{:else}
		<div class="space-y-4">
			<fieldset class="space-y-2">
				<legend class="mb-1 text-sm font-medium text-fg">{$t('botAccess.scope')}</legend>
				<label class="flex cursor-pointer items-start gap-3 text-sm text-fg">
					<input
						type="radio"
						name="bot-access-scope"
						value="workspace"
						checked={form.scope === 'workspace'}
						onchange={() => pickScope('workspace')}
						class="mt-0.5 h-4 w-4 border-input accent-primary"
					/>
					<span>
						<span class="block font-medium">{$t('botAccess.scopeWorkspace')}</span>
						<span class="block text-xs text-fg-muted">{$t('botAccess.scopeWorkspaceHelp')}</span>
					</span>
				</label>
				<label class="flex cursor-pointer items-start gap-3 text-sm text-fg">
					<input
						type="radio"
						name="bot-access-scope"
						value="projects"
						checked={form.scope === 'projects'}
						onchange={() => pickScope('projects')}
						class="mt-0.5 h-4 w-4 border-input accent-primary"
					/>
					<span>
						<span class="block font-medium">{$t('botAccess.scopeProjects')}</span>
						<span class="block text-xs text-fg-muted">{$t('botAccess.scopeProjectsHelp')}</span>
					</span>
				</label>
			</fieldset>

			{#if form.scope === 'projects'}
				<div>
					<label for="bot-access-default" class="mb-1 block text-sm font-medium text-fg"
						>{$t('botAccess.otherProjects')}</label
					>
					<select
						id="bot-access-default"
						value={form.defaultLevel}
						onchange={(event) => {
							const next = event.currentTarget.value;
							form = {
								...form,
								defaultLevel: DEFAULT_LEVELS.find((level) => level === next) ?? 'none'
							};
						}}
						class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					>
						{#each DEFAULT_LEVELS as level (level)}
							<option value={level}>{$t(`botAccess.defaultLevel.${level}`)}</option>
						{/each}
					</select>
				</div>

				<div>
					<p class="mb-1 text-sm font-medium text-fg">{$t('botAccess.projects')}</p>
					{#if projects.length === 0}
						<p class="text-sm text-fg-muted">{$t('botAccess.noProjects')}</p>
					{:else}
						<div class="max-h-72 overflow-y-auto rounded-control border border-line">
							<table class="min-w-full divide-y divide-line">
								<tbody class="divide-y divide-line">
									{#each projects as project (project.id)}
										<tr>
											<td class="px-3 py-2 text-sm text-fg">
												<span class="font-mono text-xs text-fg-muted">{project.key}</span>
												{project.name}
											</td>
											<td class="px-3 py-2 text-right">
												<select
													aria-label={$t('botAccess.levelFor', { values: { name: project.name } })}
													data-testid="bot-access-level"
													value={form.grants[project.id] ?? ''}
													onchange={(event) => {
														const next = event.currentTarget.value;
														form = withGrant(form, project.id, isGrantLevel(next) ? next : '');
													}}
													class="rounded-control border border-input bg-field px-2 py-1 text-sm text-fg"
												>
													<option value="">{$t('botAccess.likeOthers')}</option>
													{#each GRANT_LEVELS as level (level)}
														<option value={level}>{$t(`botAccess.grantLevel.${level}`)}</option>
													{/each}
												</select>
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{/if}
				</div>
			{/if}

			{#if writeHint}
				<p
					class="rounded-control border bg-warning-soft p-3 text-sm text-warning"
					role="note"
					data-testid="bot-access-write-hint"
				>
					{$t('botAccess.needsWrite')}
				</p>
			{/if}
			{#if saveError}
				<p class="text-sm text-danger" role="alert">{saveError}</p>
			{/if}
		</div>
	{/if}
</Modal>
