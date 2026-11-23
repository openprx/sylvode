// The label of a workflow state. The four built-in states (`backlog`, `todo`, `in_progress`,
// `done`) have translated labels; the server stores their English names as data, so a state that
// still carries that default name (or none) is shown with the translation. A custom state, or a
// built-in key that someone renamed, is shown as it is.

const BUILT_IN: Record<string, { labelKey: string; defaults: readonly string[] }> = {
	backlog: { labelKey: 'issue.backlog', defaults: ['backlog'] },
	todo: { labelKey: 'issue.todo', defaults: ['todo', 'to do'] },
	in_progress: { labelKey: 'issue.inProgress', defaults: ['in progress'] },
	done: { labelKey: 'issue.done', defaults: ['done'] }
};

export interface WorkflowStateLike {
	key: string;
	display_name?: string | null;
}

/** Key of the translated label of a built-in state key; `null` for any other key. */
export function builtInStateLabelKey(key: string): string | null {
	return BUILT_IN[key]?.labelKey ?? null;
}

export function workflowStateLabel(
	state: WorkflowStateLike,
	translate: (key: string) => string
): string {
	const builtIn = BUILT_IN[state.key];
	const name = state.display_name?.trim() ?? '';
	if (builtIn && (name === '' || builtIn.defaults.includes(name.toLowerCase()))) {
		return translate(builtIn.labelKey);
	}
	return name || state.key;
}
