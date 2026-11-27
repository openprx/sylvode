import type { ProjectType } from '#lib/api/projects.js';

type Translate = (id: string) => string;

function translated(t: Translate, key: string, fallback: string): string {
	const value = t(key);
	return value && value !== key ? value : fallback;
}

function fallbackName(value: string): string {
	return value.replaceAll('_', ' ');
}

export function projectTypeName(
	typeKey: string,
	projectTypes: ProjectType[],
	t: Translate
): string {
	const projectType = projectTypes.find((item) => item.key === typeKey);
	const fallback = projectType?.name ?? fallbackName(typeKey);
	return translated(t, `projectTypes.systemNames.${typeKey}`, fallback);
}

export function projectTypeDescription(projectType: ProjectType, t: Translate): string {
	return translated(
		t,
		`projectTypes.systemDescriptions.${projectType.key}`,
		projectType.description
	);
}

export function projectTypeDomain(domain: string, t: Translate): string {
	return translated(t, `projectTypes.domains.${domain}`, fallbackName(domain));
}
