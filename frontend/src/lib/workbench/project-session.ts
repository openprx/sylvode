export interface ProjectEditable {
	name: string;
	description?: string;
	updated_at: string;
}
export function projectConflicts(
	base: ProjectEditable,
	local: { name: string; description: string },
	remote: ProjectEditable
): Array<'name' | 'description'> {
	return (['name', 'description'] as const).filter(
		(field) =>
			local[field] !== (base[field] ?? '') &&
			(remote[field] ?? '') !== (base[field] ?? '') &&
			(remote[field] ?? '') !== local[field]
	);
}

export function reviewProjectDraft(
	base: ProjectEditable,
	local: { name: string; description: string },
	remote: ProjectEditable
): { name: string; description: string } {
	return {
		name: local.name !== base.name ? local.name : remote.name,
		description:
			local.description !== (base.description ?? '')
				? local.description
				: (remote.description ?? '')
	};
}
