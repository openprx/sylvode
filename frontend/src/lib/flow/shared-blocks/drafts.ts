/** Recipient CAS drafts are local state, never a whole-Page CRDT/session or a hidden rebase. */
import type { ApiResult } from '#lib/api/client.js';
import {
	parseSharedRead,
	sharedSpansValid,
	type SharedCommand,
	type SharedPatch,
	type SharedRead
} from './contracts';
export interface SharedDraftTransport {
	patch(
		shareId: string,
		patch: SharedPatch,
		key: string,
		signal?: AbortSignal
	): Promise<ApiResult<SharedRead>>;
}
export interface SharedDraftState {
	remote: SharedRead | null;
	ack: {
		acceptedRevision: number;
		currentRevision: number;
		replayed: boolean;
		newerContent: boolean;
	} | null;
	lastSent: SharedCommand[];
	commands: SharedCommand[];
	expectedRevision: number;
	phase: 'ready' | 'saving' | 'conflict' | 'unavailable' | 'failed';
	error: 'conflict' | 'unavailable' | 'write_failed' | 'invalid_draft' | null;
}
function commandsValid(read: SharedRead, commands: SharedCommand[]): boolean {
	if (commands.length > 64) return false;
	return commands.every((command) => {
		const member = read.projection.members.find((item) => item.member_id === command.member_id);
		if (!member) return false;
		const content = member.content;
		if (command.type === 'set_checked')
			return content.kind === 'todo' && typeof command.checked === 'boolean';
		if (command.type === 'set_collapsed')
			return content.kind === 'toggle' && typeof command.collapsed === 'boolean';
		if (command.type === 'replace_text')
			return (
				'text' in content &&
				typeof command.text === 'string' &&
				sharedSpansValid(command.text, command.spans) &&
				(content.kind !== 'code' || !command.spans.length)
			);
		if (command.type !== 'set_marks' || !('spans' in content) || command.edits.length > 256)
			return false;
		return command.edits.every((edit) => {
			if (
				!['bold', 'italic', 'strike', 'code', 'link'].includes(edit.mark) ||
				(edit.clear !== undefined && typeof edit.clear !== 'boolean')
			)
				return false;
			return sharedSpansValid(content.text, [
				{
					start: edit.start,
					end: edit.end,
					mark: edit.clear ? 'bold' : edit.mark,
					...(!edit.clear && edit.href ? { href: edit.href } : {})
				}
			]);
		});
	});
}
function immutableRead(read: SharedRead): SharedRead {
	const freeze = (value: unknown) => {
		if (value && typeof value === 'object' && !Object.isFrozen(value)) {
			for (const child of Object.values(value)) freeze(child);
			Object.freeze(value);
		}
	};
	freeze(read);
	return read;
}

export class SharedBlockDraft {
	private state: SharedDraftState;
	private key = crypto.randomUUID();
	private listeners = new Set<(state: SharedDraftState) => void>();
	private controller: AbortController | null = null;
	private disposed = false;
	constructor(
		read: SharedRead,
		private readonly transport: SharedDraftTransport
	) {
		const remote = immutableRead(parseSharedRead(read));
		this.state = {
			remote,
			ack: null,
			lastSent: [],
			commands: [],
			expectedRevision: remote.projection.revision,
			phase: 'ready',
			error: null
		};
	}
	subscribe(listener: (state: SharedDraftState) => void): () => void {
		this.listeners.add(listener);
		listener(this.snapshot());
		return () => this.listeners.delete(listener);
	}
	private snapshot(): SharedDraftState {
		return {
			...this.state,
			remote: this.state.remote,
			commands: structuredClone(this.state.commands),
			lastSent: structuredClone(this.state.lastSent),
			ack: this.state.ack ? { ...this.state.ack } : null
		};
	}
	private emit(): void {
		if (!this.disposed) for (const listener of this.listeners) listener(this.snapshot());
	}
	current(): SharedDraftState {
		return this.snapshot();
	}
	receive(value: SharedRead): void {
		if (this.disposed) return;
		const previous = this.state.remote;
		const remote = immutableRead(parseSharedRead(value, previous?.projection.share_id));
		if (previous && remote.projection.revision < previous.projection.revision) return;
		this.state.remote = remote;
		if (this.state.phase !== 'saving') {
			if (
				this.state.commands.length &&
				remote.projection.revision !== this.state.expectedRevision
			) {
				this.state.phase = 'conflict';
				this.state.error = 'conflict';
			} else if (!this.state.commands.length) {
				this.state.expectedRevision = remote.projection.revision;
				this.state.phase = 'ready';
				this.state.error = null;
			}
		}
		this.emit();
	}
	unavailable(): void {
		if (this.disposed) return;
		this.state.remote = null;
		this.state.phase = 'unavailable';
		this.state.error = 'unavailable';
		this.emit();
	}

	edit(commands: SharedCommand[]): boolean {
		if (
			this.disposed ||
			this.state.phase === 'saving' ||
			!this.state.remote ||
			this.state.remote.effective_permission !== 'edit' ||
			!commandsValid(this.state.remote, commands)
		)
			return false;
		this.state.commands = structuredClone(commands);
		this.state.ack = null;
		this.state.lastSent = [];
		this.key = crypto.randomUUID();
		this.emit();
		return true;
	}
	/** Conflict never changes expectedRevision or discards local commands. This is an explicit user choice. */
	rebaseDraft(): boolean {
		if (
			this.state.phase !== 'conflict' ||
			!this.state.remote ||
			this.state.remote.effective_permission !== 'edit' ||
			!commandsValid(this.state.remote, this.state.commands)
		)
			return false;
		this.state.expectedRevision = this.state.remote.projection.revision;
		this.state.phase = 'ready';
		this.state.error = null;
		this.key = crypto.randomUUID();
		this.emit();
		return true;
	}
	discardDraft(): void {
		if (this.state.phase === 'saving') return;
		this.state.commands = [];
		if (this.state.remote) {
			this.state.expectedRevision = this.state.remote.projection.revision;
			this.state.phase = 'ready';
			this.state.error = null;
		}
		this.key = crypto.randomUUID();
		this.emit();
	}
	async save(): Promise<boolean> {
		if (
			this.disposed ||
			this.state.phase === 'saving' ||
			this.state.phase === 'conflict' ||
			!this.state.remote ||
			this.state.remote.effective_permission !== 'edit' ||
			!this.state.commands.length ||
			!commandsValid(this.state.remote, this.state.commands)
		)
			return false;
		const shareId = this.state.remote.projection.share_id;
		const patch = {
			expected_revision: this.state.expectedRevision,
			commands: structuredClone(this.state.commands)
		};
		const controller = new AbortController();
		this.controller = controller;
		this.state.phase = 'saving';
		this.state.error = null;
		this.emit();
		let result: ApiResult<SharedRead>;
		try {
			result = await this.transport.patch(shareId, patch, this.key, controller.signal);
		} catch {
			result = { code: 500, message: 'write_failed', data: null };
		}
		if (this.disposed || this.controller !== controller) return false;
		this.controller = null;
		if (result.code === 0 && result.data) {
			try {
				const remote = immutableRead(parseSharedRead(result.data, shareId));
				const receipt = remote.receipt;
				const acceptedRevision = receipt?.accepted_revision ?? remote.projection.revision;
				const newerContent = !!receipt?.replayed && remote.projection.revision > acceptedRevision;
				this.state = {
					remote,
					ack: {
						acceptedRevision,
						currentRevision: remote.projection.revision,
						replayed: receipt?.replayed ?? false,
						newerContent
					},
					lastSent: newerContent ? structuredClone(patch.commands) : [],
					commands: [],
					expectedRevision: remote.projection.revision,
					phase: 'ready',
					error: null
				};
				this.key = crypto.randomUUID();
				this.emit();
				return true;
			} catch {
				this.state.phase = 'failed';
				this.state.error = 'write_failed';
				this.emit();
				return false;
			}
		}
		const details =
			result.details && typeof result.details === 'object'
				? (result.details as Record<string, unknown>)
				: null;
		if (details?.reason === 'block_share_revision_conflict') {
			try {
				const remote = immutableRead(
					parseSharedRead(
						{
							projection: details.current_projection,
							effective_permission: details.effective_permission
						},
						shareId
					)
				);
				if (remote.projection.revision !== details.revision) throw new Error('invalid_conflict');
				this.state.remote = remote;
				this.state.phase = 'conflict';
				this.state.error = 'conflict';
				this.emit();
				return false;
			} catch {
				this.state.remote = null;
				this.state.phase = 'unavailable';
				this.state.error = 'unavailable';
				this.emit();
				return false;
			}
		}
		if (
			result.code === 401 ||
			result.code === 403 ||
			result.code === 404 ||
			details?.reason === 'block_share_unavailable'
		) {
			this.state.remote = null;
			this.state.phase = 'unavailable';
			this.state.error = 'unavailable';
		} else {
			this.state.phase = 'failed';
			this.state.error = 'write_failed';
		}
		this.emit();
		return false;
	}
	dismissReceipt(): void {
		this.state.ack = null;
		this.state.lastSent = [];
		this.emit();
	}

	dispose(): void {
		this.disposed = true;
		this.controller?.abort();
		this.controller = null;
		this.listeners.clear();
	}
}
