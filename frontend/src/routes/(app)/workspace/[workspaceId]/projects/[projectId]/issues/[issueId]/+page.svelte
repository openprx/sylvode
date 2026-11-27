<script lang="ts">
	import { describeResult } from '#lib/i18n/describe.js';
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount, tick } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { activityApi, type Activity } from '#lib/api/activity.js';
	import { commentsApi, type Comment } from '#lib/api/comments.js';
	import { issuesApi, type Issue, type IssuePriority, type IssueStatus } from '#lib/api/issues.js';
	import { labelsApi, type Label } from '#lib/api/labels.js';
	import { sprintsApi, type Sprint } from '#lib/api/sprints.js';
	import { milestonesApi, type Milestone } from '#lib/api/milestones.js';
	import { membersApi, type WorkspaceMember } from '#lib/api/members.js';
	import { workflowsApi, type WorkflowStateDef } from '#lib/api/workflows.js';
	import { authStore } from '#lib/stores/auth.js';
	import { isWorkspaceAdminRole } from '#lib/workspace/home.js';
	import { toast } from '#lib/stores/toast.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import { renderMarkdown } from '#lib/utils/markdown.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import {
		appendAttachmentsMarkdown,
		extractMarkdownAttachments,
		ISSUE_ATTACHMENT_ACCEPT,
		ISSUE_ATTACHMENT_MAX_SIZE_BYTES,
		type UploadedAttachment
	} from '#lib/utils/attachments.js';
	import Button from '#lib/ui/button/Button.svelte';
	import { isImageUploadMime, isAllowedUploadMime } from '#lib/utils/upload.js';
	import { apiClient } from '#lib/api/client.js';
	import FileUpload from '#lib/components/FileUpload.svelte';
	import Input from '#lib/components/Input.svelte';
	import Select from '#lib/components/Select.svelte';
	import Tag from '#lib/components/Tag.svelte';
	import DatePicker from '#lib/ui/date/DatePicker.svelte';
	import { sprintChoices } from '#lib/sprints/rules.js';
	import {
		loadAllMilestones,
		milestoneActivityText,
		milestoneChoices,
		milestoneNameOf
	} from '#lib/milestones/rules.js';
	import { workflowStateLabel } from '#lib/utils/workflow-state.js';
	import { dueDateToTimestamp, timestampToDueDate } from '#lib/utils/due-date.js';
	import Textarea from '#lib/components/Textarea.svelte';
	import BotIcon from '#lib/components/BotIcon.svelte';
	import AgentTasksPanel from '#lib/project/agent-tasks/AgentTasksPanel.svelte';
	import ActivityPanel from '#lib/project/issue-detail/ActivityPanel.svelte';
	import { itemFieldActivityText } from '#lib/project/issue-detail/activity-text.js';
	import {
		ACTIVITY_PAGE_SIZE,
		COMMENTS_PAGE_SIZE,
		appendPage,
		nextPageToLoad,
		refreshNewest,
		remainingCount
	} from '#lib/project/issue-detail/paging.js';
	import AlertPanel from '#lib/project/alert-panel/AlertPanel.svelte';
	import LinkedPages from '#lib/project/page-links/LinkedPages.svelte';
	import { pageActivityText } from '#lib/project/page-links/rules.js';
	import {
		directoryName,
		personLabel,
		personLabelById,
		someoneText,
		type Person
	} from '#lib/people/display.js';
	import {
		highlightMentions,
		insertMentionText,
		mentionTrigger,
		pickedIds,
		withPicked,
		type PickedMention
	} from '#lib/people/mentions.js';

	type SelectOption = {
		label: string;
		value: string;
		disabled?: boolean;
	};

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
	const issueId = $derived(requireRouteParam(page.params.issueId, 'issueId'));

	let workflowStates = $state<WorkflowStateDef[]>([]);
	let statusOptions = $state<SelectOption[]>([
		{ label: get(t)('issue.backlog'), value: 'backlog' },
		{ label: get(t)('issue.todo'), value: 'todo' },
		{ label: get(t)('issue.inProgress'), value: 'in_progress' },
		{ label: get(t)('issue.done'), value: 'done' }
	]);

	const priorityOptions: SelectOption[] = [
		{ label: get(t)('issue.low'), value: 'low' },
		{ label: get(t)('issue.medium'), value: 'medium' },
		{ label: get(t)('issue.high'), value: 'high' },
		{ label: get(t)('issue.urgent'), value: 'urgent' }
	];

	let loading = $state(true);
	let issue = $state<Issue | null>(null);
	let comments = $state<Comment[]>([]);
	let activities = $state<Activity[]>([]);
	// Both lists are newest first; `*Total` is what the server holds, loaded or not.
	let commentsTotal = $state(0);
	let commentsLoadingMore = $state(false);
	let activitiesTotal = $state(0);
	let activitiesLoadingMore = $state(false);
	// The people the activity entries name, by user id (from the activity answer).
	let activityPeople = $state<Record<string, Person>>({});
	let projectLabels = $state<Label[]>([]);
	let issueLabels = $state<Label[]>([]);
	let sprints = $state<Sprint[]>([]);
	let milestones = $state<Milestone[]>([]);
	let workspaceMembers = $state<WorkspaceMember[]>([]);
	let memberNameMap = $state<Record<string, string>>({});
	let memberEntityTypeMap = $state<Record<string, string>>({});

	let editingIssue = $state(false);
	let issueForm = $state({
		title: '',
		description: ''
	});
	let savingIssue = $state(false);
	let deletingIssue = $state(false);
	let issueDescriptionAttachments = $state<UploadedAttachment[]>([]);

	let statusValue = $state<IssueStatus>('backlog');
	let priorityValue = $state<IssuePriority>('medium');
	let assigneeValue = $state('');
	let sprintValue = $state('');
	let milestoneValue = $state('');
	let dueValue = $state<string | null>(null);
	let updatingField = $state<
		'status' | 'priority' | 'assignee' | 'sprint' | 'milestone' | 'due' | null
	>(null);

	let selectedLabelId = $state('');
	let labelSaving = $state(false);
	let newLabelName = $state('');
	let newLabelColor = $state('#2563eb');
	let creatingLabel = $state(false);

	let agentTasksRefresh = $state(0);
	let newComment = $state('');
	let commentSubmitting = $state(false);
	let editingCommentId = $state<string | null>(null);
	let editingCommentContent = $state('');
	let commentBusyId = $state<string | null>(null);
	let showMentionDropdown = $state(false);
	let mentionFilter = $state('');
	let mentionAnchorIndex = $state<number | null>(null);
	let commentTextareaEl = $state<HTMLTextAreaElement | null>(null);
	let showEditMentionDropdown = $state(false);
	let editMentionFilter = $state('');
	let editMentionAnchorIndex = $state<number | null>(null);
	let editTextareaEl = $state<HTMLTextAreaElement | null>(null);
	let commentAttachments = $state<UploadedAttachment[]>([]);
	// People picked from the `@` picker; typed `@Name` needs no pick (the server resolves it).
	let pickedNew = $state<PickedMention[]>([]);
	let pickedEdit = $state<PickedMention[]>([]);

	onMount(async () => {
		await loadPageData();
	});

	const currentUserId = $derived($authStore.user?.id ?? '');

	const assigneeOptions = $derived.by<SelectOption[]>(() => {
		const opts = Object.entries(memberNameMap)
			.sort(([, a], [, b]) => a.localeCompare(b))
			.map(([id, name]) => ({
				label: memberEntityTypeMap[id] === 'bot' ? `[Bot] ${name}` : name,
				value: id
			}));
		return [{ label: get(t)('common.unassigned'), value: '' }, ...opts];
	});

	const availableLabelOptions = $derived.by<SelectOption[]>(() => {
		const selected = new Set((issueLabels || []).map((item) => item.id));
		return projectLabels
			.filter((item) => !selected.has(item.id))
			.map((item) => ({ label: item.name, value: item.id }));
	});

	// "No sprint" and the sprints the issue can be moved into: planned and active ones. A completed
	// or cancelled sprint is listed only while the issue is still in it, and cannot be chosen.
	const sprintSelectOptions = $derived.by<SelectOption[]>(() =>
		sprintChoices(sprints, issue?.sprint_id ?? null, {
			none: get(t)('issue.noSprint'),
			status: (status) => get(t)(`cycles.status.${status}`)
		})
	);

	// "No milestone" and the open milestones of the workspace; a closed milestone is listed only
	// while the item is still in it, and cannot be chosen.
	const milestoneSelectOptions = $derived.by<SelectOption[]>(() =>
		milestoneChoices(milestones, issue?.milestone_id ?? null, {
			none: get(t)('milestones.field.none'),
			closed: get(t)('milestones.field.closed')
		})
	);

	const mentionCandidates = $derived.by(() =>
		workspaceMembers.filter((member) => member.user_id !== currentUserId)
	);

	const filteredMentionMembers = $derived.by(() => {
		const keyword = mentionFilter.trim().toLowerCase();
		return mentionCandidates
			.filter((member) => {
				if (!keyword) {
					return true;
				}

				return (
					member.name.toLowerCase().includes(keyword) ||
					member.email.toLowerCase().includes(keyword)
				);
			})
			.slice(0, 8);
	});

	const filteredEditMentionMembers = $derived.by(() => {
		const keyword = editMentionFilter.trim().toLowerCase();
		return mentionCandidates
			.filter((member) => {
				if (!keyword) {
					return true;
				}

				return (
					member.name.toLowerCase().includes(keyword) ||
					member.email.toLowerCase().includes(keyword)
				);
			})
			.slice(0, 8);
	});

	const existingIssueAttachments = $derived.by(() =>
		extractMarkdownAttachments(issueForm.description)
	);
	const terminalStateKey = $derived.by(
		() => workflowStates.find((s) => s.is_terminal)?.key || 'done'
	);

	async function loadPageData() {
		loading = true;

		const [
			issueResponse,
			commentsResponse,
			activityResponse,
			labelsResponse,
			issueLabelsResponse,
			sprintsResponse,
			workflowResponse,
			milestonesResponse
		] = await Promise.all([
			issuesApi.get(issueId),
			commentsApi.list(issueId, 1, COMMENTS_PAGE_SIZE),
			activityApi.list(issueId, 1, ACTIVITY_PAGE_SIZE),
			labelsApi.list(workspaceId),
			labelsApi.listForIssue(issueId),
			sprintsApi.list(projectId),
			workflowsApi.getEffectiveByProject(projectId),
			loadAllMilestones(milestonesApi.list, workspaceId)
		]);

		if (issueResponse.code !== 0) {
			toast.error(describeResult(issueResponse));
		} else if (issueResponse.data) {
			issue = issueResponse.data;
			issueForm = {
				title: issueResponse.data.title,
				description: issueResponse.data.description ?? ''
			};
			statusValue = issueResponse.data.status;
			priorityValue = issueResponse.data.priority;
			assigneeValue = issueResponse.data.assignee_id ?? '';
			sprintValue = issueResponse.data.sprint_id ?? '';
			milestoneValue = issueResponse.data.milestone_id ?? '';
			dueValue = timestampToDueDate(issueResponse.data.due_at);
			issueLabels = extractIssueLabels(issueResponse.data);
		}

		if (commentsResponse.code !== 0) {
			toast.error(describeResult(commentsResponse));
		} else if (commentsResponse.data) {
			comments = commentsResponse.data.items ?? [];
			commentsTotal = commentsResponse.data.total;
		}

		if (activityResponse.code !== 0) {
			toast.error(describeResult(activityResponse));
		} else if (activityResponse.data) {
			activities = activityResponse.data.items ?? [];
			activitiesTotal = activityResponse.data.total;
			activityPeople = activityResponse.data.people;
		}

		if (labelsResponse.code !== 0) {
			toast.error(describeResult(labelsResponse));
		} else if (labelsResponse.data) {
			projectLabels = labelsResponse.data.items ?? [];
		}

		if (issueLabelsResponse.code === 0 && issueLabelsResponse.data) {
			issueLabels = issueLabelsResponse.data.items ?? [];
		}

		if (sprintsResponse.code !== 0) {
			toast.error(describeResult(sprintsResponse));
		} else if (sprintsResponse.data) {
			sprints = sprintsResponse.data.items ?? [];
		}

		if (milestonesResponse.code !== 0) {
			toast.error(get(t)('milestones.field.loadFailed'));
		} else if (milestonesResponse.data) {
			milestones = milestonesResponse.data;
		}

		if (workflowResponse.code === 0 && workflowResponse.data?.states?.length) {
			workflowStates = workflowResponse.data.states;
			statusOptions = workflowResponse.data.states.map((s) => ({
				label: workflowStateLabel(s, get(t)),
				value: s.key
			}));
		}

		loading = false;

		// Load workspace members for assignee names
		membersApi
			.list(workspaceId)
			.then((res) => {
				if (res.code === 0 && res.data) {
					workspaceMembers = res.data.items ?? [];
					const map: Record<string, string> = {};
					const entityMap: Record<string, string> = {};
					for (const member of res.data.items ?? []) {
						map[member.user_id] = directoryName(member, get(t));
						entityMap[member.user_id] = member.entity_type || 'human';
					}
					memberNameMap = map;
					memberEntityTypeMap = entityMap;
				}
			})
			.catch(() => {});
	}

	function extractIssueLabels(data: Issue): Label[] {
		const ext = data as Issue & { issue_labels?: Label[] };
		if (Array.isArray(ext.labels)) {
			return ext.labels.map((label) => ({
				id: label.id,
				name: label.name,
				color: label.color,
				workspace_id: workspaceId,
				description: '',
				created_at: ''
			}));
		}
		if (Array.isArray(ext.issue_labels)) {
			return ext.issue_labels;
		}
		return [];
	}

	function formatDate(dateText: string): string {
		return new Date(dateText).toLocaleString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function formatIssueCode(data: Issue): string {
		return data.identifier;
	}

	async function updateIssue(data: {
		title?: string;
		description?: string;
		status?: IssueStatus;
		priority?: IssuePriority;
		assignee_id?: string | null;
		sprint_id?: string | null;
		milestone_id?: string | null;
		due_at?: string | null;
	}): Promise<boolean> {
		if (!issue) {
			return false;
		}

		const response = await issuesApi.update(issue.id, data);
		if (response.code !== 0) {
			toast.error(describeResult(response));
			return false;
		}

		if (response.data) {
			issue = response.data;
			issueLabels = extractIssueLabels(response.data);
			issueForm = {
				title: response.data.title,
				description: response.data.description ?? ''
			};
			statusValue = response.data.status;
			priorityValue = response.data.priority;
			assigneeValue = response.data.assignee_id ?? '';
			sprintValue = response.data.sprint_id ?? '';
			milestoneValue = response.data.milestone_id ?? '';
			dueValue = timestampToDueDate(response.data.due_at);
		}

		void reloadActivities();
		return true;
	}

	async function saveIssueEdit() {
		if (!issue || savingIssue) {
			return;
		}
		const title = issueForm.title.trim();
		if (!title) {
			toast.error(get(t)('issue.titleRequired'));
			return;
		}

		savingIssue = true;
		const description = appendAttachmentsMarkdown(
			issueForm.description,
			issueDescriptionAttachments
		).trim();
		const success = await updateIssue({
			title,
			description: description || undefined
		});
		savingIssue = false;
		if (success) {
			editingIssue = false;
			issueDescriptionAttachments = [];
			toast.success(get(t)('issue.updated'));
		}
	}

	function toggleIssueEditMode() {
		editingIssue = !editingIssue;
		issueDescriptionAttachments = [];
	}

	async function handleStatusChange() {
		if (!issue || updatingField) {
			return;
		}
		if (issue.status === statusValue) {
			return;
		}

		updatingField = 'status';
		const previous = issue.status;
		const success = await updateIssue({ status: statusValue });
		if (!success) {
			statusValue = previous;
		} else {
			toast.success(get(t)('issue.statusUpdated'));
		}
		updatingField = null;
	}

	async function handlePriorityChange() {
		if (!issue || updatingField) {
			return;
		}
		if (issue.priority === priorityValue) {
			return;
		}

		updatingField = 'priority';
		const previous = issue.priority;
		const success = await updateIssue({ priority: priorityValue });
		if (!success) {
			priorityValue = previous;
		} else {
			toast.success(get(t)('issue.priorityUpdated'));
		}
		updatingField = null;
	}

	async function handleAssigneeChange() {
		if (!issue || updatingField) {
			return;
		}
		if ((issue.assignee_id ?? '') === assigneeValue) {
			return;
		}

		updatingField = 'assignee';
		const previous = issue.assignee_id ?? '';
		const success = await updateIssue({ assignee_id: assigneeValue || null });
		if (!success) {
			assigneeValue = previous;
		} else {
			toast.success(get(t)('issue.assigneeUpdated'));
		}
		updatingField = null;
	}

	async function handleSprintChange() {
		if (!issue || updatingField) {
			return;
		}
		if ((issue.sprint_id ?? '') === sprintValue) {
			return;
		}

		updatingField = 'sprint';
		const previous = issue.sprint_id ?? '';
		const success = await updateIssue({ sprint_id: sprintValue || null });
		if (!success) {
			sprintValue = previous;
		} else {
			toast.success(get(t)('issue.sprintUpdated'));
		}
		updatingField = null;
	}

	async function handleMilestoneChange() {
		if (!issue || updatingField) {
			return;
		}
		if ((issue.milestone_id ?? '') === milestoneValue) {
			return;
		}

		updatingField = 'milestone';
		const previous = issue.milestone_id ?? '';
		const success = await updateIssue({ milestone_id: milestoneValue || null });
		if (!success) {
			milestoneValue = previous;
		} else {
			toast.success(get(t)('milestones.field.updated'));
		}
		updatingField = null;
	}

	async function handleDueChange(next: string | null) {
		if (!issue || updatingField) {
			return;
		}
		const previous = timestampToDueDate(issue.due_at);
		if (next === previous) {
			return;
		}

		dueValue = next;
		updatingField = 'due';
		const success = await updateIssue({ due_at: dueDateToTimestamp(next) });
		if (!success) {
			dueValue = previous;
		} else {
			toast.success(get(t)('issue.dueDateUpdated'));
		}
		updatingField = null;
	}

	async function handleCloseIssue() {
		if (!issue) {
			return;
		}

		const terminalState = workflowStates.find((s) => s.is_terminal)?.key || 'done';
		if (issue.status === terminalState) {
			return;
		}

		statusValue = terminalState;
		await handleStatusChange();
	}

	async function handleDeleteIssue() {
		if (!issue || deletingIssue) {
			return;
		}

		if (
			!(await confirmAction({
				tone: 'destructive',
				title: get(t)('issue.deleteTitle'),
				body: get(t)('issue.deleteBody'),
				confirmLabel: get(t)('issue.deleteAction')
			}))
		) {
			return;
		}

		deletingIssue = true;
		const response = await issuesApi.delete(issue.id);
		deletingIssue = false;

		if (response.code !== 0) {
			toast.error(describeResult(response));
			return;
		}

		toast.success(get(t)('issue.deleted'));
		goto(
			resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/issues', {
				workspaceId,
				projectId
			})
		);
	}

	async function handleAddLabel() {
		if (!issue || !selectedLabelId || labelSaving) {
			return;
		}

		labelSaving = true;
		const response = await labelsApi.addToIssue(issue.id, selectedLabelId);
		labelSaving = false;

		if (response.code !== 0) {
			toast.error(describeResult(response));
			return;
		}

		const selectedLabel = projectLabels.find((item) => item.id === selectedLabelId);
		if (selectedLabel && !issueLabels?.some((item) => item.id === selectedLabel.id)) {
			issueLabels = [...(issueLabels || []), selectedLabel];
		}
		selectedLabelId = '';
		void reloadActivities();
		toast.success(get(t)('issue.labelAdded'));
	}

	async function handleRemoveLabel(labelId: string) {
		if (!issue || labelSaving) {
			return;
		}

		labelSaving = true;
		const response = await labelsApi.removeFromIssue(issue.id, labelId);
		labelSaving = false;

		if (response.code !== 0) {
			toast.error(describeResult(response));
			return;
		}

		issueLabels = (issueLabels || []).filter((item) => item.id !== labelId);
		void reloadActivities();
		toast.success(get(t)('issue.labelRemoved'));
	}

	async function handleCreateLabel() {
		if (creatingLabel) {
			return;
		}
		const name = newLabelName.trim();
		if (!name) {
			toast.error(get(t)('issue.labelNameRequired'));
			return;
		}

		creatingLabel = true;
		const response = await labelsApi.create(workspaceId, {
			name,
			color: newLabelColor
		});
		creatingLabel = false;

		if (response.code !== 0) {
			toast.error(describeResult(response));
			return;
		}

		if (response.data) {
			projectLabels = [...projectLabels, response.data];
			newLabelName = '';
			selectedLabelId = response.data.id;
			toast.success(get(t)('issue.labelCreated'));
		}
	}

	function canManageComment(comment: Comment): boolean {
		return Boolean(currentUserId && currentUserId === comment.user_id);
	}

	function handleNewCommentInput(event: Event) {
		const textarea = event.target as HTMLTextAreaElement;
		commentTextareaEl = textarea;
		const mentionState = mentionTrigger(textarea.value, textarea.selectionStart ?? 0);
		mentionAnchorIndex = mentionState.start;
		mentionFilter = mentionState.filter;
		showMentionDropdown = mentionState.start !== null;
	}

	function handleEditCommentInput(event: Event) {
		const textarea = event.target as HTMLTextAreaElement;
		editTextareaEl = textarea;
		const mentionState = mentionTrigger(textarea.value, textarea.selectionStart ?? 0);
		editMentionAnchorIndex = mentionState.start;
		editMentionFilter = mentionState.filter;
		showEditMentionDropdown = mentionState.start !== null;
	}

	function insertMention(
		textarea: HTMLTextAreaElement,
		currentText: string,
		anchorIndex: number | null,
		member: WorkspaceMember
	): { nextText: string; cursorPos: number } {
		const cursorPos = textarea.selectionStart ?? currentText.length;
		const inserted = insertMentionText(
			currentText,
			anchorIndex,
			cursorPos,
			directoryName(member, get(t))
		);
		return { nextText: inserted.text, cursorPos: inserted.cursor };
	}

	async function selectMention(member: WorkspaceMember) {
		if (!commentTextareaEl) {
			return;
		}
		const inserted = insertMention(commentTextareaEl, newComment, mentionAnchorIndex, member);
		newComment = inserted.nextText;
		pickedNew = withPicked(pickedNew, {
			id: member.user_id,
			text: directoryName(member, get(t))
		});
		await tick();
		commentTextareaEl.focus();
		commentTextareaEl.setSelectionRange(inserted.cursorPos, inserted.cursorPos);
		showMentionDropdown = false;
		mentionFilter = '';
		mentionAnchorIndex = null;
	}

	async function selectEditMention(member: WorkspaceMember) {
		if (!editTextareaEl) {
			return;
		}
		const inserted = insertMention(
			editTextareaEl,
			editingCommentContent,
			editMentionAnchorIndex,
			member
		);
		editingCommentContent = inserted.nextText;
		pickedEdit = withPicked(pickedEdit, {
			id: member.user_id,
			text: directoryName(member, get(t))
		});
		await tick();
		editTextareaEl.focus();
		editTextareaEl.setSelectionRange(inserted.cursorPos, inserted.cursorPos);
		showEditMentionDropdown = false;
		editMentionFilter = '';
		editMentionAnchorIndex = null;
	}

	async function pasteUpload(file: File, target: 'description' | 'comment'): Promise<void> {
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
			let md: string;
			if (isImageUploadMime(mime)) {
				md = `![image](${url})`;
			} else if (mime.startsWith('video/')) {
				md = `<video controls src="${url}"></video>`;
			} else {
				md = `[${file.name}](${url})`;
			}
			if (target === 'description') {
				const suffix = issueForm.description.endsWith('\n') || !issueForm.description ? '' : '\n';
				issueForm.description = `${issueForm.description}${suffix}${md}\n`;
			} else {
				const suffix = newComment.endsWith('\n') || !newComment ? '' : '\n';
				newComment = `${newComment}${suffix}${md}\n`;
			}
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
				if (file) void pasteUpload(file, 'description');
				return;
			}
		}
	}

	function handleCommentPaste(event: ClipboardEvent) {
		const items = event.clipboardData?.items;
		if (!items) return;
		for (const item of items) {
			if (item.kind === 'file' && isAllowedUploadMime(item.type)) {
				event.preventDefault();
				const file = item.getAsFile();
				if (file) void pasteUpload(file, 'comment');
				return;
			}
		}
	}

	function handleDescriptionUpload(detail: { url: string; filename: string }) {
		const { url, filename } = detail;
		const att = issueDescriptionAttachments.find((a) => a.url === url);
		const mime = att?.mimeType ?? '';
		if (isImageUploadMime(mime)) {
			const md = `![${filename}](${url})`;
			const suffix = issueForm.description.endsWith('\n') || !issueForm.description ? '' : '\n';
			issueForm.description = `${issueForm.description}${suffix}${md}\n`;
		} else if (mime.startsWith('video/')) {
			const md = `<video controls src="${url}"></video>`;
			const suffix = issueForm.description.endsWith('\n') || !issueForm.description ? '' : '\n';
			issueForm.description = `${issueForm.description}${suffix}${md}\n`;
		}
	}

	function handleCommentUpload(detail: { url: string; filename: string }) {
		const { url, filename } = detail;
		const att = commentAttachments.find((a) => a.url === url);
		const mime = att?.mimeType ?? '';
		if (isImageUploadMime(mime)) {
			const md = `![${filename}](${url})`;
			const suffix = newComment.endsWith('\n') || !newComment ? '' : '\n';
			newComment = `${newComment}${suffix}${md}\n`;
		} else if (mime.startsWith('video/')) {
			const md = `<video controls src="${url}"></video>`;
			const suffix = newComment.endsWith('\n') || !newComment ? '' : '\n';
			newComment = `${newComment}${suffix}${md}\n`;
		}
	}

	async function handleCreateComment() {
		if (commentSubmitting) {
			return;
		}

		const content = appendAttachmentsMarkdown(newComment, commentAttachments).trim();
		if (!content) {
			return;
		}

		commentSubmitting = true;
		const mentionIds = pickedIds(newComment, pickedNew);
		const response = await commentsApi.create(issueId, {
			content,
			mentions: mentionIds.length > 0 ? mentionIds : undefined
		});
		commentSubmitting = false;

		if (response.code !== 0) {
			toast.error(describeResult(response));
			return;
		}

		if (response.data) {
			comments = appendPage([response.data], comments);
			commentsTotal += 1;
			agentTasksRefresh += 1;
			void reloadActivities();
			newComment = '';
			pickedNew = [];
			commentAttachments = [];
			showMentionDropdown = false;
			mentionFilter = '';
			mentionAnchorIndex = null;
			toast.success(get(t)('issue.commentAdded'));
		}
	}

	function startEditComment(comment: Comment) {
		if (!canManageComment(comment)) {
			return;
		}
		editingCommentId = comment.id;
		editingCommentContent = comment.content;
		pickedEdit = comment.mentions.flatMap((person) =>
			person.name ? [{ id: person.id, text: person.name }] : []
		);
	}

	function cancelEditComment() {
		editingCommentId = null;
		editingCommentContent = '';
		pickedEdit = [];
		showEditMentionDropdown = false;
		editMentionFilter = '';
		editMentionAnchorIndex = null;
		editTextareaEl = null;
	}

	async function handleUpdateComment() {
		if (!editingCommentId || !editingCommentContent.trim() || commentBusyId) {
			return;
		}

		commentBusyId = editingCommentId;
		const mentionIds = pickedIds(editingCommentContent, pickedEdit);
		const response = await commentsApi.update(editingCommentId, {
			content: editingCommentContent.trim(),
			mentions: mentionIds.length > 0 ? mentionIds : undefined
		});
		commentBusyId = null;

		if (response.code !== 0) {
			toast.error(describeResult(response));
			return;
		}

		if (response.data) {
			comments = comments.map((item) => (item.id === response.data?.id ? response.data : item));
			cancelEditComment();
			void reloadActivities();
			toast.success(get(t)('issue.commentUpdated'));
		}
	}

	async function handleDeleteComment(id: string) {
		if (commentBusyId) {
			return;
		}
		if (
			!(await confirmAction({
				tone: 'destructive',
				title: get(t)('issue.commentDeleteTitle'),
				body: get(t)('issue.commentDeleteBody'),
				confirmLabel: get(t)('issue.commentDeleteAction')
			}))
		) {
			return;
		}

		commentBusyId = id;
		const response = await commentsApi.delete(id);
		commentBusyId = null;

		if (response.code !== 0) {
			toast.error(describeResult(response));
			return;
		}

		comments = comments.filter((item) => item.id !== id);
		commentsTotal = Math.max(0, commentsTotal - 1);
		void reloadActivities();
		if (editingCommentId === id) {
			cancelEditComment();
		}
		toast.success(get(t)('issue.commentDeleted'));
	}

	function formatStatus(status: string): string {
		return workflowStateLabel(
			workflowStates.find((s) => s.key === status) ?? { key: status },
			get(t)
		);
	}

	function formatPriority(priority: string): string {
		const map: Record<string, string> = {
			low: get(t)('issue.low'),
			medium: get(t)('issue.medium'),
			high: get(t)('issue.high'),
			urgent: get(t)('issue.urgent')
		};
		return map[priority] || priority;
	}

	// Who made an activity entry or got an item: the person the answer carries, else the member
	// list, else the neutral text. Never an id.
	function getMemberDisplay(userId: string | undefined): string {
		if (!userId) {
			return '';
		}
		const person = activityPeople[userId];
		if (person) {
			return personLabel(person, get(t));
		}
		return memberNameMap[userId] || someoneText(get(t));
	}

	// Highlights only the people the saved comment mentions; a plain `@word` stays plain.
	function renderCommentContent(comment: Comment): string {
		return highlightMentions(renderMarkdown(comment.content), comment.mentions, get(t));
	}

	function isBotMember(member: WorkspaceMember): boolean {
		return member.entity_type === 'bot';
	}

	function getSprintDisplay(sprintId: string | undefined): string {
		if (!sprintId) {
			return '';
		}
		return sprints.find((item) => item.id === sprintId)?.name || get(t)('people.unknownSprint');
	}

	/** Reads the newest activity again; the older entries already loaded stay below it. */
	async function reloadActivities() {
		const response = await activityApi.list(issueId, 1, ACTIVITY_PAGE_SIZE);
		if (response.code === 0 && response.data) {
			activities = refreshNewest(activities, response.data.items ?? []);
			activitiesTotal = response.data.total;
			activityPeople = { ...activityPeople, ...response.data.people };
		}
	}

	async function loadMoreActivities() {
		activitiesLoadingMore = true;
		const response = await activityApi.list(
			issueId,
			nextPageToLoad(activities.length, ACTIVITY_PAGE_SIZE),
			ACTIVITY_PAGE_SIZE
		);
		activitiesLoadingMore = false;
		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));
			return;
		}
		activities = appendPage(activities, response.data.items ?? []);
		activitiesTotal = response.data.total;
		activityPeople = { ...activityPeople, ...response.data.people };
	}

	async function loadMoreComments() {
		commentsLoadingMore = true;
		const response = await commentsApi.list(
			issueId,
			nextPageToLoad(comments.length, COMMENTS_PAGE_SIZE),
			COMMENTS_PAGE_SIZE
		);
		commentsLoadingMore = false;
		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));
			return;
		}
		comments = appendPage(comments, response.data.items ?? []);
		commentsTotal = response.data.total;
	}

	function getActivityAuthor(activity: Activity): string {
		return personLabelById(activityPeople, activity.user_id, get(t));
	}

	function formatActivity(activity: Activity): string {
		const detail = activity.detail || {};
		const from = typeof detail.from === 'string' ? detail.from : '';
		const to = typeof detail.to === 'string' ? detail.to : '';
		const labelName = typeof detail.label_name === 'string' ? detail.label_name : '';

		switch (activity.action) {
			case 'assigned':
				if (to) {
					return get(t)('activity.assigned', { values: { name: getMemberDisplay(to) } });
				}
				return get(t)('activity.unassigned');
			case 'status_changed':
				return get(t)('activity.statusChanged', {
					values: { from: formatStatus(from), to: formatStatus(to) }
				});
			case 'priority_changed':
				return get(t)('activity.priorityChanged', {
					values: { from: formatPriority(from), to: formatPriority(to) }
				});
			case 'sprint_changed':
				if (to) {
					return get(t)('activity.sprintChanged', {
						values: { name: getSprintDisplay(to) }
					});
				}
				return get(t)('activity.sprintRemoved');
			case 'milestone_changed':
				return milestoneActivityText(get(t), detail, (id) => milestoneNameOf(milestones, id));
			case 'label_added':
				return get(t)('activity.labelAdded', { values: { name: labelName } });
			case 'label_removed':
				return get(t)('activity.labelRemoved', { values: { name: labelName } });
			case 'comment_added':
				return get(t)('activity.commentAdded');
			case 'comment_edited':
				return get(t)('activity.commentEdited');
			case 'comment_deleted': {
				const preview =
					typeof detail.content_preview === 'string' ? detail.content_preview.trim() : '';
				return preview
					? get(t)('activity.commentDeletedWithPreview', { values: { preview } })
					: get(t)('activity.commentDeleted');
			}
			case 'page_linked':
			case 'page_unlinked':
				return pageActivityText(get(t), activity.action, detail) ?? '';
			default:
				return (
					itemFieldActivityText(get(t), activity.action, detail) ??
					(activity.action || get(t)('common.unknownAction'))
				);
		}
	}
</script>

<div class="mx-auto max-w-7xl pb-44 md:pb-0">
	{#if loading}
		<div class="rounded-card border border-line bg-raised p-10">
			<div class="flex items-center justify-center gap-3 text-fg-muted">
				<svg class="h-5 w-5 animate-spin" viewBox="0 0 24 24" fill="none">
					<circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="3"
					></circle>
					<path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v3a5 5 0 00-5 5H4z"
					></path>
				</svg>
				<span>{$t('issue.loadingDetail')}</span>
			</div>
		</div>
	{:else if !issue}
		<div class="rounded-card border border-line bg-raised p-10 text-center text-fg-muted">
			{$t('issue.notFound')}
		</div>
	{:else}
		<div class="space-y-4">
			<div
				class="flex flex-col gap-3 rounded-card border border-line bg-raised p-4 sm:flex-row sm:items-center sm:justify-between"
			>
				<div class="text-sm text-fg-muted">
					<a href={resolve('/(app)/workspace')} class="hover:underline">{$t('nav.myWorkspace')}</a>
					<span class="mx-1 text-fg-muted">/</span>
					<a
						href={resolve('/(app)/workspace/[workspaceId]/projects', { workspaceId })}
						class="hover:underline">{$t('project.title')}</a
					>
					<span class="mx-1 text-fg-muted">/</span>
					<a
						href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/issues', {
							workspaceId,
							projectId
						})}
						class="hover:underline">{$t('issue.list')}</a
					>
					<span class="mx-1 text-fg-muted">/</span>
					<span class="font-medium text-fg">{formatIssueCode(issue)}</span>
				</div>
				<div class="flex flex-wrap gap-2">
					<Button variant="text" size="sm" onclick={toggleIssueEditMode}>
						{editingIssue ? $t('issue.cancelEdit') : $t('common.edit')}
					</Button>
					<Button
						variant="danger-quiet"
						size="sm"
						onclick={handleDeleteIssue}
						loading={deletingIssue}
					>
						{$t('common.delete')}
					</Button>
					<Button
						variant="secondary"
						size="sm"
						onclick={handleCloseIssue}
						disabled={issue.status === terminalStateKey}
					>
						{$t('issue.close')}
					</Button>
				</div>
			</div>

			<div class="grid grid-cols-1 gap-4 lg:grid-cols-10">
				<div class="space-y-4 lg:col-span-7">
					<div class="rounded-card border border-line bg-raised p-5">
						<p class="mb-2 text-xs font-mono text-fg-muted">
							{formatIssueCode(issue)}
						</p>
						{#if issue.proposal_id || issue.governance_exempt}
							<div class="mb-3 flex flex-wrap items-center gap-2">
								{#if issue.proposal_id}
									<a
										href={resolve('/(app)/proposals/[id]', { id: issue.proposal_id })}
										class="inline-flex items-center rounded-full border bg-surface px-2 py-0.5 text-xs font-medium text-fg"
									>
										{$t('issue.sourceProposal')}: {issue.proposal_id}
									</a>
								{/if}
								{#if issue.governance_exempt}
									<span
										class="inline-flex items-center rounded-full border bg-warning-soft px-2 py-0.5 text-xs font-medium text-warning"
										title={issue.governance_exempt_reason || ''}
									>
										{$t('issue.governanceExempt')}
									</span>
								{/if}
							</div>
						{/if}
						{#if editingIssue}
							<div class="space-y-3">
								<Input
									label={$t('issue.title')}
									bind:value={issueForm.title}
									maxlength={200}
									required
								/>
								<Textarea
									label={$t('issue.addDescription')}
									bind:value={issueForm.description}
									rows={8}
									placeholder={$t('issue.descriptionInputPlaceholder')}
									onpaste={handleDescriptionPaste}
								/>
								<FileUpload
									bind:value={issueDescriptionAttachments}
									accept={ISSUE_ATTACHMENT_ACCEPT}
									maxSize={ISSUE_ATTACHMENT_MAX_SIZE_BYTES}
									multiple
									onupload={handleDescriptionUpload}
								/>
								{#if existingIssueAttachments.length > 0}
									<div class="rounded-control border border-line bg-surface px-3 py-2 text-sm">
										<p class="mb-1 font-medium text-fg">
											{$t('issue.existingAttachments')}
										</p>
										<div class="space-y-1">
											{#each existingIssueAttachments as attachment (attachment.url)}
												<!-- eslint-disable svelte/no-navigation-without-resolve -- attachment and user-supplied URLs are not app routes -->
												<a
													href={attachment.url}
													target="_blank"
													rel="noreferrer"
													class="block truncate text-fg underline"
												>
													{attachment.isImage ? '🖼️' : '📎'}
													{attachment.filename}
												</a>
												<!-- eslint-enable svelte/no-navigation-without-resolve -->
											{/each}
										</div>
									</div>
								{/if}
								<div class="flex gap-2">
									<Button variant="primary" onclick={saveIssueEdit} loading={savingIssue}
										>{$t('issue.saveChanges')}</Button
									>
									<Button variant="text" onclick={toggleIssueEditMode}>{$t('common.cancel')}</Button
									>
								</div>
							</div>
						{:else}
							<h1 class="mb-4 text-2xl font-semibold text-fg">
								{issue.title}
							</h1>
							<div class="rounded-control border border-line bg-surface p-4">
								{#if issue.description?.trim()}
									<div class="prose prose-sm max-w-none markdown-body text-sm text-fg">
										<!-- eslint-disable-next-line svelte/no-at-html-tags -- renderMarkdown escapes its input before it builds markup (tests/markdown-sanitize.test.ts) -->
										{@html renderMarkdown(issue.description)}
									</div>
								{:else}
									<p class="text-sm text-fg-muted">
										{$t('issue.noDescriptionPreview')}
									</p>
								{/if}
							</div>
						{/if}
					</div>

					<AlertPanel
						{issueId}
						{workspaceId}
						{projectId}
						isAdmin={isWorkspaceAdminRole(
							workspaceMembers.find((member) => member.user_id === currentUserId)?.role
						)}
						refreshKey={agentTasksRefresh}
					/>

					<LinkedPages {issueId} {workspaceId} onchange={reloadActivities} />

					<div class="rounded-card border border-line bg-raised p-5">
						<h2 class="mb-4 text-lg font-semibold text-fg">
							{$t('issue.comments')}（{commentsTotal}）
						</h2>
						<div class="space-y-4">
							{#if comments.length === 0}
								<p class="text-sm text-fg-muted">{$t('issue.noComments')}</p>
							{:else}
								{#each comments as comment (comment.id)}
									<div class="rounded-card border border-line p-4">
										<div class="mb-2 flex items-start justify-between gap-3">
											<div>
												<p class="text-sm font-medium text-fg">
													{personLabel(comment.author, $t)}
												</p>
												<p class="text-xs text-fg-muted">
													{formatDate(comment.created_at)}
												</p>
											</div>
											{#if canManageComment(comment)}
												<div class="flex gap-1">
													<Button variant="text" size="sm" onclick={() => startEditComment(comment)}
														>{$t('common.edit')}</Button
													>

													<Button
														variant="text"
														size="sm"
														onclick={() => handleDeleteComment(comment.id)}
														disabled={commentBusyId === comment.id}
													>
														{$t('common.delete')}
													</Button>
												</div>
											{/if}
										</div>

										{#if editingCommentId === comment.id}
											<div class="space-y-2 relative">
												{#if showEditMentionDropdown && filteredEditMentionMembers.length > 0}
													<div
														class="absolute bottom-full left-0 z-50 mb-1 max-h-48 w-64 overflow-y-auto rounded-float border border-line bg-overlay shadow-overlay"
													>
														{#each filteredEditMentionMembers as member (member.user_id)}
															<button
																class="w-full px-3 py-2 text-left text-sm hover:bg-surface"
																onclick={() => selectEditMention(member)}
															>
																<span class="font-medium inline-flex items-center gap-1">
																	{#if isBotMember(member)}
																		<BotIcon
																			class="h-3.5 w-3.5 text-fg-muted"
																			title={$t('admin.botBadge')}
																		/>
																	{/if}
																	<span>{directoryName(member, $t)}</span>
																</span>
																<span class="ml-1 text-fg-muted">{member.email}</span>
															</button>
														{/each}
													</div>
												{/if}
												<Textarea
													bind:value={editingCommentContent}
													rows={4}
													maxlength={5000}
													oninput={handleEditCommentInput}
												/>
												<div class="flex gap-2">
													<Button
														variant="primary"
														size="sm"
														onclick={handleUpdateComment}
														disabled={commentBusyId === comment.id}
													>
														{$t('common.save')}
													</Button>
													<Button variant="text" size="sm" onclick={cancelEditComment}
														>{$t('common.cancel')}</Button
													>
												</div>
											</div>
										{:else}
											<div class="prose prose-sm max-w-none markdown-body text-sm text-fg">
												<!-- eslint-disable-next-line svelte/no-at-html-tags -- renderMarkdown escapes its input before it builds markup (tests/markdown-sanitize.test.ts) -->
												{@html renderCommentContent(comment)}
											</div>
										{/if}
									</div>
								{/each}
								{#if remainingCount(commentsTotal, comments.length) > 0}
									<Button size="sm" loading={commentsLoadingMore} onclick={loadMoreComments}>
										{$t('issue.loadMoreComments', {
											values: { count: remainingCount(commentsTotal, comments.length) }
										})}
									</Button>
								{/if}
							{/if}
						</div>
					</div>
				</div>

				<div class="space-y-4 lg:col-span-3">
					<div class="space-y-3 rounded-card border border-line bg-raised p-5">
						<Select
							label={$t('common.status')}
							options={statusOptions}
							bind:value={statusValue}
							onchange={handleStatusChange}
							disabled={updatingField !== null}
						/>
						<Select
							label={$t('common.priority')}
							options={priorityOptions}
							bind:value={priorityValue}
							onchange={handlePriorityChange}
							disabled={updatingField !== null}
						/>
						<Select
							label={$t('common.assignee')}
							options={assigneeOptions}
							bind:value={assigneeValue}
							onchange={handleAssigneeChange}
							disabled={updatingField !== null}
						/>
						<Select
							label={$t('issue.sprint')}
							options={sprintSelectOptions}
							bind:value={sprintValue}
							onchange={handleSprintChange}
							disabled={updatingField !== null}
						/>
						<Select
							label={$t('milestones.field.label')}
							options={milestoneSelectOptions}
							bind:value={milestoneValue}
							onchange={handleMilestoneChange}
							disabled={updatingField !== null}
						/>
						<DatePicker
							label={$t('issue.dueDate')}
							helper={$t('issue.dueDateHelper')}
							value={dueValue}
							onchange={handleDueChange}
							disabled={updatingField !== null}
						/>

						<div class="space-y-2">
							<p class="text-sm font-medium text-fg">
								{$t('issue.labels')}
							</p>
							<div class="flex flex-wrap gap-2">
								{#if issueLabels.length === 0}
									<span class="text-xs text-fg-muted">{$t('issue.noLabels')}</span>
								{:else}
									{#each issueLabels as label (label.id)}
										<Tag
											label={label.name}
											color={label.color}
											removable
											onremove={() => handleRemoveLabel(label.id)}
										/>
									{/each}
								{/if}
							</div>
							<div class="flex gap-2">
								<div class="flex-1">
									<Select
										options={availableLabelOptions}
										bind:value={selectedLabelId}
										placeholder={$t('issue.selectLabel')}
										disabled={labelSaving || availableLabelOptions.length === 0}
									/>
								</div>
								<Button
									variant="primary"
									size="sm"
									onclick={handleAddLabel}
									disabled={!selectedLabelId || labelSaving}
								>
									{$t('common.create')}
								</Button>
							</div>
							<div class="grid grid-cols-1 gap-2">
								<Input
									bind:value={newLabelName}
									placeholder={$t('issue.newLabelName')}
									maxlength={50}
								/>
								<div class="flex gap-2">
									<input
										type="color"
										bind:value={newLabelColor}
										class="h-9 w-12 rounded border border-input bg-field"
									/>
									<Button
										size="sm"
										variant="text"
										onclick={handleCreateLabel}
										loading={creatingLabel}
									>
										{$t('issue.createLabel')}
									</Button>
								</div>
							</div>
						</div>
					</div>

					<AgentTasksPanel {issueId} names={memberNameMap} refreshKey={agentTasksRefresh} />

					<ActivityPanel
						{activities}
						total={activitiesTotal}
						loadingMore={activitiesLoadingMore}
						onloadmore={loadMoreActivities}
						text={formatActivity}
						author={getActivityAuthor}
						date={formatDate}
					/>
				</div>
			</div>

			<div
				class="fixed inset-x-0 bottom-0 z-20 border-t border-line bg-canvas/95 p-3 backdrop-blur md:static md:z-auto md:border-0 md:bg-transparent md:p-0"
			>
				<div
					class="relative mx-auto max-w-7xl rounded-card border border-line bg-raised p-3 md:p-4"
				>
					{#if showMentionDropdown && filteredMentionMembers.length > 0}
						<div
							class="absolute bottom-full left-3 z-50 mb-1 max-h-48 w-64 overflow-y-auto rounded-float border border-line bg-overlay shadow-overlay"
						>
							{#each filteredMentionMembers as member (member.user_id)}
								<button
									class="w-full px-3 py-2 text-left text-sm hover:bg-surface"
									onclick={() => selectMention(member)}
								>
									<span class="font-medium inline-flex items-center gap-1">
										{#if isBotMember(member)}
											<BotIcon class="h-3.5 w-3.5 text-fg-muted" title={$t('admin.botBadge')} />
										{/if}
										<span>{directoryName(member, $t)}</span>
									</span>
									<span class="ml-1 text-fg-muted">{member.email}</span>
								</button>
							{/each}
						</div>
					{/if}
					<Textarea
						label={$t('issue.addComment')}
						bind:value={newComment}
						rows={3}
						maxlength={5000}
						placeholder={$t('issue.commentPlaceholder')}
						oninput={handleNewCommentInput}
						onpaste={handleCommentPaste}
					/>
					<div class="mt-2 flex items-start justify-between gap-3">
						<div class="min-w-0 flex-1">
							<FileUpload
								compact
								bind:value={commentAttachments}
								accept={ISSUE_ATTACHMENT_ACCEPT}
								maxSize={ISSUE_ATTACHMENT_MAX_SIZE_BYTES}
								multiple
								onupload={handleCommentUpload}
							/>
						</div>
						<Button
							variant="primary"
							onclick={handleCreateComment}
							disabled={!newComment.trim() && commentAttachments.length === 0}
							loading={commentSubmitting}
						>
							{$t('issue.submitComment')}
						</Button>
					</div>
				</div>
			</div>
		</div>
	{/if}
</div>

<style>
	:global(.markdown-body h1),
	:global(.markdown-body h2),
	:global(.markdown-body h3) {
		font-weight: 600;
		margin-top: 0.75rem;
		margin-bottom: 0.5rem;
	}

	:global(.markdown-body p) {
		margin: 0.5rem 0;
	}

	:global(.markdown-body img) {
		max-width: 100%;
		height: auto;
		border-radius: 0.375rem;
		display: block;
		margin: 0.5rem 0;
	}

	:global(.markdown-body pre) {
		border-radius: 0.375rem;
		padding: 0.75rem;
		background: rgb(15 23 42);
		color: rgb(248 250 252);
		overflow: auto;
	}

	:global(.markdown-body code) {
		font-size: 0.85em;
	}

	:global(.markdown-body ul),
	:global(.markdown-body ol) {
		margin: 0.5rem 0;
		padding-left: 1.25rem;
	}

	:global(.markdown-body .comment-mention) {
		color: rgb(37 99 235);
		font-weight: 600;
	}
</style>
