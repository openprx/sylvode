import { getContext, setContext } from 'svelte';
import type { FormsPage } from '#lib/forms/state/forms-page.js';

export const FORMS_PAGE = Symbol('forms-page');

export function setFormsPageContext(fp: FormsPage): FormsPage {
	return setContext(FORMS_PAGE, fp);
}

export function getFormsPageContext(): FormsPage {
	const fp = getContext<FormsPage | undefined>(FORMS_PAGE);
	if (!fp) throw new Error('getFormsPageContext() outside the forms page');
	return fp;
}
