import { apiClient } from '#lib/api/client.js';
/** Account/TaskTab-owned projection draft. No Page repository, bootstrap, WS, history or source ID. */
import type { TaskWorkbench } from '../../workbench/tabs';
import type { SharedMember, SharedCommand, SharedTargetContext } from './contracts';
import { SharedBlockDraft, type SharedDraftState } from './drafts';
import { SharedMemberEditor } from './editor';
import { sharedBlockApi } from './client';
import { pageInlineRuns, type PageInlineSpan } from '../inline-runs';
export type SharedEditorIssue =
	| 'structure'
	| 'finish_composition'
	| 'unsaved_member'
	| 'remote_updated'
	| 'member_changed'
	| null;
export interface SharedSessionSnapshot {
	loading: boolean;
	authorized: boolean;
	loadError: 'unavailable' | 'failed' | null;
	draft: SharedDraftState | null;
	editing: { member: SharedMember; checked: boolean; collapsed: boolean } | null;
	editor: SharedMemberEditor | null;
	issue: SharedEditorIssue;
}
function shape(value: { text: string; spans?: PageInlineSpan[] }): string {
	const runs: { text: string; marks: unknown[] }[] = [];
	for (const run of pageInlineRuns(value.text, value.spans ?? [])) {
		const previous = runs[runs.length - 1];
		if (previous && JSON.stringify(previous.marks) === JSON.stringify(run.marks))
			previous.text += run.text;
		else runs.push(structuredClone(run));
	}
	return JSON.stringify(runs);
}
export class SharedViewSession {
	private readonly authEpoch = apiClient.getAuthSessionEpoch();
	private readonly authSignal = apiClient.authSessionSignal(this.authEpoch);
	private readonly authChanged = () => this.invalidateAuth();
	private listeners = new Set<(value: SharedSessionSnapshot) => void>();
	private controller: SharedBlockDraft | null = null;
	private releaseDraft = () => {};
	private request: AbortController | null = null;
	private refreshing: Promise<void> | null = null;
	private loading = false;
	private allowed = false;
	private disposed = false;
	private loadError: 'unavailable' | 'failed' | null = null;
	private member: SharedMember | null = null;
	private engine: SharedMemberEditor | null = null;
	private value: { text: string; spans: PageInlineSpan[] } | null = null;
	private checked = false;
	private collapsed = false;
	private issue: SharedEditorIssue = null;
	private editorLabel: string | undefined;
	constructor(
		readonly shareId: string,
		private readonly owner: string,
		private readonly workbench: TaskWorkbench
	) {
		this.authSignal.addEventListener('abort', this.authChanged, { once: true });
	}
	private invalidateAuth(): void {
		if (this.disposed) return;
		this.allowed = false;
		this.loading = false;
		this.loadError = 'unavailable';
		this.request?.abort();
		this.request = null;
		this.engine?.setEditable(false);
		this.controller?.unavailable();
		// Invalidation can only remove canonical content; never apply an old response to a new actor.
		for (const listener of this.listeners) listener(this.current());
	}

	private alive(): boolean {
		return (
			!this.disposed &&
			apiClient.isAuthSessionEpoch(this.authEpoch) &&
			this.workbench.current.userId === this.owner
		);
	}
	authorized = (): boolean => this.alive() && this.allowed;
	current(): SharedSessionSnapshot {
		return {
			loading: this.loading,
			authorized: this.authorized(),
			loadError: this.loadError,
			draft: this.controller?.current() ?? null,
			editing: this.member
				? { member: structuredClone(this.member), checked: this.checked, collapsed: this.collapsed }
				: null,
			editor: this.engine,
			issue: this.issue
		};
	}
	subscribe(listener: (value: SharedSessionSnapshot) => void): () => void {
		this.listeners.add(listener);
		listener(this.current());
		return () => this.listeners.delete(listener);
	}
	private emit(): void {
		if (!this.alive()) return;
		const state = this.controller?.current();
		this.engine?.setEditable(
			this.allowed && state?.remote?.effective_permission === 'edit' && state.phase !== 'saving'
		);
		const dirty = this.dirty(),
			key = `shared-block:${this.shareId}`;
		if (this.workbench.current.tabs.find((tab) => tab.key === key)?.dirty !== dirty)
			this.workbench.markDirty(key, dirty);
		for (const listener of this.listeners) listener(this.current());
	}
	dirty(): boolean {
		const local =
			this.member &&
			'text' in this.member.content &&
			this.value &&
			(shape(this.value) !== shape(this.member.content) ||
				(this.member.content.kind === 'todo' && this.checked !== this.member.content.checked) ||
				(this.member.content.kind === 'toggle' &&
					this.collapsed !== this.member.content.collapsed));
		return (
			!!local ||
			!!this.controller?.current().commands.length ||
			this.controller?.current().phase === 'saving'
		);
	}
	refresh(context?: SharedTargetContext, requireFresh = false): Promise<void> {
		if (this.refreshing) return this.refreshing;
		if (!this.alive() || this.controller?.current().phase === 'saving') return Promise.resolve();
		const run = async () => {
			this.loading = true;
			if (requireFresh) this.allowed = false;
			this.loadError = null;
			this.emit();
			const request = new AbortController();
			this.request = request;
			const result = await sharedBlockApi.read(this.shareId, request.signal, context);
			if (!this.alive() || this.request !== request) return;
			this.request = null;
			this.loading = false;
			if (result.code !== 0 || !result.data) {
				this.allowed = false;
				this.loadError = [401, 403, 404].includes(result.code) ? 'unavailable' : 'failed';
				this.controller?.unavailable();
				this.emit();
				return;
			}
			this.allowed = true;
			const previous = this.controller?.current(),
				dirty = !!previous?.commands.length;
			if (!this.controller) {
				this.controller = new SharedBlockDraft(result.data, sharedBlockApi);
				this.releaseDraft = this.controller.subscribe(() => this.emit());
			} else this.controller.receive(result.data);
			if (
				this.member &&
				!dirty &&
				previous?.remote?.projection.revision !== result.data.projection.revision
			) {
				const next = result.data.projection.members.find(
					(member) => member.member_id === this.member!.member_id
				);
				if (!next || !('text' in next.content)) {
					this.clearEditor();
				} else if (
					shape(next.content) !==
					shape(this.member.content as { text: string; spans?: PageInlineSpan[] })
				) {
					this.installEditor(next);
					this.issue = 'remote_updated';
				} else {
					this.member = structuredClone(next);
					this.checked = next.content.kind === 'todo' ? next.content.checked : false;
					this.collapsed = next.content.kind === 'toggle' ? next.content.collapsed : false;
				}
			}
			this.emit();
		};
		this.refreshing = run().finally(() => {
			this.refreshing = null;
		});
		return this.refreshing;
	}
	openMember(id: string, label: string): boolean {
		const state = this.controller?.current();
		if (!this.authorized() || state?.remote?.effective_permission !== 'edit') return false;
		const member = state.remote.projection.members.find((value) => value.member_id === id);
		if (!member || !('text' in member.content)) return false;
		if (this.member?.member_id === id && this.engine) return true;
		if (this.dirty()) {
			this.issue = 'unsaved_member';
			this.emit();
			return false;
		}
		this.editorLabel = label;
		this.installEditor(member, label);
		this.emit();
		return true;
	}
	private installEditor(member: SharedMember, label?: string): void {
		this.clearEditor();
		this.issue = null;
		this.member = structuredClone(member);
		this.checked = member.content.kind === 'todo' ? member.content.checked : false;
		this.collapsed = member.content.kind === 'toggle' ? member.content.collapsed : false;
		this.value =
			'text' in member.content
				? {
						text: member.content.text,
						spans: 'spans' in member.content ? structuredClone(member.content.spans) : []
					}
				: null;
		this.engine = new SharedMemberEditor(
			member.content,
			(value) => {
				this.value = value;
				this.updateCommands();
			},
			(reason) => {
				this.issue = reason;
				this.emit();
			},
			label ?? this.editorLabel
		);
	}
	private updateCommands(): void {
		if (!this.member || !this.controller || !this.value || !('text' in this.member.content)) return;
		const commands: SharedCommand[] = [];
		if (shape(this.value) !== shape(this.member.content))
			commands.push({
				type: 'replace_text',
				member_id: this.member.member_id,
				...structuredClone(this.value)
			});
		if (this.member.content.kind === 'todo' && this.checked !== this.member.content.checked)
			commands.push({
				type: 'set_checked',
				member_id: this.member.member_id,
				checked: this.checked
			});
		if (this.member.content.kind === 'toggle' && this.collapsed !== this.member.content.collapsed)
			commands.push({
				type: 'set_collapsed',
				member_id: this.member.member_id,
				collapsed: this.collapsed
			});
		if (JSON.stringify(commands) !== JSON.stringify(this.controller.current().commands))
			this.controller.edit(commands);
		this.emit();
	}
	setChecked(value: boolean): void {
		if (this.authorized() && this.member?.content.kind === 'todo') {
			this.checked = value;
			this.updateCommands();
		}
	}
	setCollapsed(value: boolean): void {
		if (this.authorized() && this.member?.content.kind === 'toggle') {
			this.collapsed = value;
			this.updateCommands();
		}
	}
	async save(): Promise<boolean> {
		if (!this.authorized() || this.loading || !this.controller) return false;
		if (this.engine?.isComposing()) {
			this.issue = 'finish_composition';
			this.emit();
			return false;
		}
		const saved = await this.controller.save();
		if (saved && this.alive() && this.member) {
			const next = this.controller
				.current()
				.remote?.projection.members.find((member) => member.member_id === this.member!.member_id);
			if (next && 'text' in next.content) {
				if (this.value && shape(next.content) !== shape(this.value)) {
					this.installEditor(next);
				} else {
					this.member = structuredClone(next);
					this.checked = next.content.kind === 'todo' ? next.content.checked : false;
					this.collapsed = next.content.kind === 'toggle' ? next.content.collapsed : false;
				}
			}
		}
		this.emit();
		return saved;
	}
	keepDraft(): boolean {
		const state = this.controller?.current();
		if (!state?.remote || !this.member || !this.value) return false;
		const latest = state.remote.projection.members.find(
			(member) => member.member_id === this.member!.member_id
		);
		if (!latest || !('text' in latest.content)) return false;
		if (latest.content.kind !== this.member.content.kind) {
			this.issue = 'member_changed';
			this.emit();
			return false;
		}
		if (!state.commands.some((command) => command.type === 'set_checked'))
			this.checked = latest.content.kind === 'todo' ? latest.content.checked : false;
		if (!state.commands.some((command) => command.type === 'set_collapsed'))
			this.collapsed = latest.content.kind === 'toggle' ? latest.content.collapsed : false;
		this.member = structuredClone(latest);
		this.updateCommands();
		const applied = this.controller!.rebaseDraft();
		this.emit();
		return applied;
	}
	discard(): void {
		this.controller?.discardDraft();
		this.clearEditor();
		this.issue = null;
		this.emit();
	}
	dismissIssue(): void {
		this.issue = null;
		this.emit();
	}
	dismissReceipt(): void {
		this.controller?.dismissReceipt();
	}
	private clearEditor(): void {
		this.engine?.destroy();
		this.engine = null;
		this.member = null;
		this.value = null;
	}
	dispose(): void {
		if (this.disposed) return;
		this.authSignal.removeEventListener('abort', this.authChanged);
		this.allowed = false;
		this.request?.abort();
		this.request = null;
		this.controller?.unavailable();
		this.clearEditor();
		this.releaseDraft();
		this.controller?.dispose();
		this.disposed = true;
		this.listeners.clear();
	}
}
const sessions = new WeakMap<TaskWorkbench, Map<string, SharedViewSession>>();
export function retainedSharedSession(
	workbench: TaskWorkbench,
	shareId: string
): SharedViewSession | null {
	const owner = workbench.current.userId,
		key = `shared-block:${shareId}`;
	if (!owner || !workbench.current.tabs.some((tab) => tab.key === key)) return null;
	let map = sessions.get(workbench);
	if (!map) {
		map = new Map();
		sessions.set(workbench, map);
	}
	const existing = map.get(shareId);
	if (existing) return existing;
	const session = new SharedViewSession(shareId, owner, workbench);
	map.set(shareId, session);
	if (
		!workbench.retain(key, () => {
			session.dispose();
			map!.delete(shareId);
		})
	) {
		map.delete(shareId);
		session.dispose();
		return null;
	}
	return session;
}
