<script lang="ts">
	import { describeResult } from '#lib/i18n/describe.js';
	import { labelsApi } from '#lib/api/labels.js';
	import { membersApi, type WorkspaceMember } from '#lib/api/members.js';
	import { issuesApi, type Issue, type IssuePriority, type IssueStatus } from '#lib/api/issues.js';
	import type { WorkflowStateDef } from '#lib/api/workflows.js';
	import { sprintsApi, type Sprint } from '#lib/api/sprints.js';
	import { sprintChoices } from '#lib/sprints/rules.js';
	import { workflowStateLabel } from '#lib/utils/workflow-state.js';
	import { toast } from '#lib/stores/toast.js';
	import { renderMarkdown } from '#lib/utils/markdown.js';
	import {
		appendAttachmentsMarkdown,
		ISSUE_ATTACHMENT_ACCEPT,
		ISSUE_ATTACHMENT_MAX_SIZE_BYTES,
		type UploadedAttachment
	} from '#lib/utils/attachments.js';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { apiClient } from '#lib/api/client.js';
	import { isImageUploadMime, isAllowedUploadMime } from '#lib/utils/upload.js';
	import Button from '#lib/ui/button/Button.svelte';
	import FileUpload from './FileUpload.svelte';
	import Input from './Input.svelte';
	import Modal from './Modal.svelte';
	import Tag from './Tag.svelte';

	interface LabelOption {
		id: string;
		name: string;
		color: string;
	}

	interface Props {
		open?: boolean;
		workspaceId: string;
		projectId: string;
		initialStatus?: IssueStatus;
		initialPriority?: IssuePriority;
		initialSprintId?: string;
		workflowStates?: WorkflowStateDef[];
		onCreated?: (issue: Issue) => void;
	}

	let {
		open = $bindable(false),
		workspaceId,
		projectId,
		initialStatus = 'backlog',
		initialPriority = 'medium',
		initialSprintId = '',
		workflowStates = [],
		onCreated
	}: Props = $props();

	let loadingOptions = $state(false);
	let creating = $state(false);
	let descriptionTab = $state<'edit' | 'preview'>('edit');
	let members = $state<WorkspaceMember[]>([]);
	let sprints = $state<Sprint[]>([]);
	let labels = $state<LabelOption[]>([]);
	let selectedLabelIds = $state<string[]>([]);
	let attachments = $state<UploadedAttachment[]>([]);

	// The same offering as the issue page: no sprint, then the active and planned sprints.
	const sprintSelectChoices = $derived(
		sprintChoices(sprints, null, {
			none: $t('issue.noSprint'),
			status: (status) => $t(`cycles.status.${status}`)
		})
	);

	const statusSelectOptions = $derived.by(() => {
		if (workflowStates.length > 0) {
			return workflowStates.map((s) => ({ value: s.key, label: workflowStateLabel(s, get(t)) }));
		}
		return [
			{ value: 'backlog', label: get(t)('issue.backlog') },
			{ value: 'todo', label: get(t)('issue.todo') },
			{ value: 'in_progress', label: get(t)('issue.inProgress') },
			{ value: 'done', label: get(t)('issue.done') }
		];
	});

	let form = $state({
		title: '',
		description: '',
		status: 'backlog' as IssueStatus,
		priority: 'medium' as IssuePriority,
		assignee_id: '',
		sprint_id: ''
	});

	let wasOpen = false;
	$effect(() => {
		if (open && !wasOpen) {
			resetForm();
			void loadOptions();
		}
		wasOpen = open;
	});

	function resetForm(): void {
		const defaultStatus =
			initialStatus ||
			workflowStates.find((s) => s.is_initial)?.key ||
			workflowStates[0]?.key ||
			'backlog';
		form = {
			title: '',
			description: '',
			status: defaultStatus,
			priority: initialPriority,
			assignee_id: '',
			sprint_id: initialSprintId
		};
		selectedLabelIds = [];
		attachments = [];
		descriptionTab = 'edit';
	}

	async function loadOptions(): Promise<void> {
		loadingOptions = true;

		const [memberRes, sprintRes, labelRes] = await Promise.all([
			membersApi.list(workspaceId),
			sprintsApi.list(projectId),
			labelsApi.list(workspaceId)
		]);

		if (memberRes.code === 0 && memberRes.data) {
			members = memberRes.data.items ?? [];
		} else {
			toast.error(describeResult(memberRes, $t('issue.loadMembersFailed')));
		}

		if (sprintRes.code === 0 && sprintRes.data) {
			sprints = sprintRes.data.items ?? [];
		} else {
			toast.error(describeResult(sprintRes, $t('issue.loadSprintsFailed')));
		}

		if (labelRes.code === 0 && labelRes.data) {
			labels = (labelRes.data.items ?? []).map((item) => ({
				id: String(item.id),
				name: String(item.name),
				color: String(item.color || '#64748b')
			}));
		} else {
			toast.error(describeResult(labelRes, $t('issue.loadLabelsFailed')));
		}

		loadingOptions = false;
	}

	function toggleLabel(labelId: string): void {
		if (selectedLabelIds.includes(labelId)) {
			selectedLabelIds = selectedLabelIds.filter((id) => id !== labelId);
			return;
		}
		selectedLabelIds = [...selectedLabelIds, labelId];
	}

	async function handleCreate(): Promise<void> {
		if (creating) {
			return;
		}
		if (!form.title.trim()) {
			toast.error($t('issue.enterTitle'));
			return;
		}
		creating = true;
		const description = appendAttachmentsMarkdown(form.description, attachments).trim();
		const createRes = await issuesApi.create(projectId, {
			title: form.title.trim(),
			description: description || undefined,
			status: form.status,
			priority: form.priority,
			assignee_id: form.assignee_id || undefined,
			sprint_id: form.sprint_id || undefined
		});

		if (createRes.code !== 0 || !createRes.data) {
			toast.error(describeResult(createRes, $t('issue.createFailed')));
			creating = false;
			return;
		}

		if (selectedLabelIds.length > 0) {
			const labelRequests = selectedLabelIds.map((labelId) =>
				labelsApi.addToIssue(createRes.data!.id, labelId)
			);
			const labelResults = await Promise.all(labelRequests);
			const failed = labelResults.find((result) => result.code !== 0);
			if (failed) {
				toast.error(describeResult(failed, $t('issue.addLabelPartialFail')));
			}
		}

		const selectedLabels = labels
			.filter((label) => selectedLabelIds.includes(label.id))
			.map((label) => ({
				id: label.id,
				name: label.name,
				color: label.color
			}));

		const createdIssue: Issue = {
			...createRes.data,
			labels: selectedLabels.length > 0 ? selectedLabels : createRes.data.labels
		};

		onCreated?.(createdIssue);
		toast.success($t('issue.createSuccess'));
		open = false;
		resetForm();
		creating = false;
	}

	async function pasteUpload(file: File): Promise<void> {
		const formData = new FormData();
		formData.append('file', file);
		try {
			const res = await fetch('/api/v1/upload', {
				method: 'POST',
				headers: { Authorization: `Bearer ${apiClient.getToken()}` },
				body: formData
			});
			const result = await res.json();
			const url = result?.data?.url ?? result?.url;
			if (!url) {
				toast.error(get(t)('toast.uploadFail'));
				return;
			}
			const mime = file.type;
			const md = isImageUploadMime(mime)
				? `![image](${url})`
				: mime.startsWith('video/')
					? `<video controls src="${url}"></video>`
					: `[${file.name}](${url})`;
			const suffix = form.description.endsWith('\n') || !form.description ? '' : '\n';
			form.description = `${form.description}${suffix}${md}\n`;
			toast.success(get(t)('toast.uploadSuccess'));
		} catch {
			toast.error(get(t)('toast.uploadNetworkFail'));
		}
	}

	function handleDescriptionPaste(event: ClipboardEvent) {
		const items = event.clipboardData?.items;
		if (!items) return;
		for (const item of items) {
			if (item.kind === 'file' && isAllowedUploadMime(item.type)) {
				event.preventDefault();
				const file = item.getAsFile();
				if (file) void pasteUpload(file);
				return;
			}
		}
	}

	function handleUploadEvent(detail: { url: string; filename: string }) {
		const { url, filename } = detail;
		const att = attachments.find((a) => a.url === url);
		const mime = att?.mimeType ?? '';
		if (isImageUploadMime(mime)) {
			const md = `![${filename}](${url})`;
			const suffix = form.description.endsWith('\n') || !form.description ? '' : '\n';
			form.description = `${form.description}${suffix}${md}\n`;
		} else if (mime.startsWith('video/')) {
			const md = `<video controls src="${url}"></video>`;
			const suffix = form.description.endsWith('\n') || !form.description ? '' : '\n';
			form.description = `${form.description}${suffix}${md}\n`;
		}
	}

	function getMemberOptionLabel(member: WorkspaceMember): string {
		const base = member.name || member.email;
		return member.entity_type === 'bot' ? `[Bot] ${base}` : base;
	}
</script>

<Modal bind:open title={$t('issue.create')} maxWidthClass="max-w-2xl">
	{#if loadingOptions}
		<div class="space-y-3">
			<div class="h-10 animate-pulse rounded-control bg-surface"></div>
			<div class="h-56 animate-pulse rounded-control bg-surface"></div>
			<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
				<div class="h-10 animate-pulse rounded-control bg-surface"></div>
				<div class="h-10 animate-pulse rounded-control bg-surface"></div>
			</div>
		</div>
	{:else}
		<form
			class="space-y-5"
			onsubmit={(event) => {
				event.preventDefault();
				handleCreate();
			}}
		>
			<Input
				label={$t('issue.title')}
				required
				bind:value={form.title}
				placeholder={$t('issue.titleExample')}
			/>

			<div class="space-y-2">
				<div class="flex items-center justify-between">
					<label class="text-sm font-medium text-fg" for="issueDescription"
						>{$t('issue.description')}</label
					>
					<div class="inline-flex rounded-control border border-line bg-surface p-0.5">
						<button
							type="button"
							class={`rounded px-2 py-1 text-xs ${
								descriptionTab === 'edit' ? 'bg-raised text-fg' : 'text-fg-muted hover:text-fg'
							}`}
							onclick={() => (descriptionTab = 'edit')}
						>
							{$t('issue.editMode')}
						</button>
						<button
							type="button"
							class={`rounded px-2 py-1 text-xs ${
								descriptionTab === 'preview' ? 'bg-raised text-fg' : 'text-fg-muted hover:text-fg'
							}`}
							onclick={() => (descriptionTab = 'preview')}
						>
							{$t('issue.previewMode')}
						</button>
					</div>
				</div>

				{#if descriptionTab === 'edit'}
					<textarea
						id="issueDescription"
						bind:value={form.description}
						class="min-h-[220px] w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted"
						placeholder={$t('issue.descriptionPlaceholder')}
						onpaste={handleDescriptionPaste}></textarea>
					<FileUpload
						bind:value={attachments}
						accept={ISSUE_ATTACHMENT_ACCEPT}
						maxSize={ISSUE_ATTACHMENT_MAX_SIZE_BYTES}
						multiple
						onupload={handleUploadEvent}
					/>
				{:else}
					<div
						class="markdown-body min-h-[220px] rounded-control border border-line bg-surface px-3 py-2 text-sm text-fg"
					>
						{#if form.description.trim()}
							<!-- eslint-disable-next-line svelte/no-at-html-tags -- renderMarkdown escapes its input before it builds markup (tests/markdown-sanitize.test.ts) -->
							{@html renderMarkdown(form.description)}
						{:else}
							<p class="text-fg-muted">{$t('issue.noDescriptionPreview')}</p>
						{/if}
					</div>
				{/if}
			</div>

			<div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
				<div>
					<label class="mb-1 block text-sm font-medium text-fg" for="issueStatus"
						>{$t('issue.status')}</label
					>
					<select
						id="issueStatus"
						bind:value={form.status}
						class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					>
						{#each statusSelectOptions as opt (opt.value)}
							<option value={opt.value}>{opt.label}</option>
						{/each}
					</select>
				</div>

				<div>
					<label class="mb-1 block text-sm font-medium text-fg" for="issuePriority"
						>{$t('issue.priority')}</label
					>
					<select
						id="issuePriority"
						bind:value={form.priority}
						class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					>
						<option value="low">{$t('issue.low')}</option>
						<option value="medium">{$t('issue.medium')}</option>
						<option value="high">{$t('issue.high')}</option>
						<option value="urgent">{$t('issue.urgent')}</option>
					</select>
				</div>

				<div>
					<label class="mb-1 block text-sm font-medium text-fg" for="issueAssignee"
						>{$t('issue.assignee')}</label
					>
					<select
						id="issueAssignee"
						bind:value={form.assignee_id}
						class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					>
						<option value="">{$t('common.unassigned')}</option>
						{#each members as member (member.user_id)}
							<option value={member.user_id}>{getMemberOptionLabel(member)}</option>
						{/each}
					</select>
				</div>

				<div>
					<label class="mb-1 block text-sm font-medium text-fg" for="issueSprint"
						>{$t('issue.sprint')}</label
					>
					<select
						id="issueSprint"
						bind:value={form.sprint_id}
						class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					>
						{#each sprintSelectChoices as choice (choice.value)}
							<option value={choice.value} disabled={choice.disabled}>{choice.label}</option>
						{/each}
					</select>
				</div>
			</div>

			<fieldset class="space-y-2">
				<legend class="block text-sm font-medium text-fg">{$t('issue.labels')}</legend>
				{#if labels.length === 0}
					<p
						class="rounded-control border border-dashed border-input bg-surface px-3 py-2 text-sm text-fg-muted"
					>
						{$t('issue.noWorkspaceLabels')}
					</p>
				{:else}
					<div class="max-h-36 space-y-1 overflow-y-auto rounded-control border border-line p-2">
						{#each labels as label (label.id)}
							<label
								class="flex cursor-pointer items-center justify-between rounded px-2 py-1 text-sm hover:bg-surface"
							>
								<div class="flex items-center gap-2">
									<input
										type="checkbox"
										checked={selectedLabelIds.includes(label.id)}
										onchange={() => toggleLabel(label.id)}
									/>
									<Tag label={label.name} color={label.color} />
								</div>
							</label>
						{/each}
					</div>
				{/if}
			</fieldset>
		</form>
	{/if}

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (open = false)}>{$t('common.cancel')}</Button>
		<Button variant="primary" loading={creating} onclick={handleCreate}>{$t('issue.create')}</Button
		>
	{/snippet}
</Modal>

<style>
	:global(.markdown-body img) {
		max-width: 100%;
		height: auto;
		border-radius: 0.375rem;
		margin: 0.5rem 0;
	}

	:global(.markdown-body p) {
		margin-top: 0.5rem;
		margin-bottom: 0.5rem;
	}

	:global(.markdown-body code) {
		background: var(--rf-surface);
		border-radius: 0.25rem;
		padding: 0.125rem 0.25rem;
	}
</style>
