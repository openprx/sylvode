/** The site administration API of `apps/api/src/site` (contract C3, section 6.6). */

export type SiteStatus = 'draft' | 'published' | 'disabled';

/** A text value: a plain string, or a map of locales that always holds `en`. */
export type SiteText = string | Record<string, string>;

export interface SiteInstallation {
	id: string;
	package_key: string;
}

export interface SiteSettings {
	allowed_origin?: string;
	frame_ancestors?: string[];
	theme?: string;
}

export interface Site {
	id: string;
	project_id: string;
	slug: string;
	title: SiteText;
	settings: SiteSettings;
	status: SiteStatus;
	created_at: string;
	/** When the site last went from not published to published; kept while it is unpublished. */
	published_at: string | null;
	/** Where the site is opened; `null` when nothing serves it. */
	url: string | null;
	/** `separate`: the site's own origin; `same_origin`: served by this server (trial). */
	url_mode: 'separate' | 'same_origin' | null;
	installation: SiteInstallation | null;
}

export type PageAudience = 'visitor' | 'operator';

export interface SitePage {
	id: string;
	site_id: string;
	route: string;
	layout_id: string;
	audience: PageAudience;
	/** The menu label; `null` keeps the page out of the menu. */
	nav: SiteText | null;
	position: number;
	/** Set for a page a package brings: read-only here. */
	installation_id: string | null;
	generation: number | null;
}

/** What `POST /sites/{id}/pages` and `PATCH /site-pages/{id}` take; absent members stay on update. */
export interface PageInput {
	route?: string;
	audience?: PageAudience;
	nav?: SiteText | null;
	layout_id?: string;
	position?: number;
}

/** What visitors or operators may read through a binding. */
export interface BindingRead {
	fields: string[];
	/** Visitors: `own` or `none`; operators: `all` or `none`. */
	scope: string;
	filter?: string[];
	sort?: string[];
}

export interface BindingProjection {
	visitor?: {
		read?: BindingRead;
		submit?: { fields: string[]; max_per_hour?: number; captcha?: boolean };
	};
	operator?: {
		read?: BindingRead;
		update?: { fields: string[] };
	};
}

export interface SiteBinding {
	site_id: string;
	binding_key: string;
	form_id: string;
	projection: BindingProjection;
}

export interface SiteRole {
	site_id: string;
	user_id: string;
	role: 'site_operator' | 'site_editor';
}

/** What `PUT /sites/{id}/settings` takes; the call replaces every key, so send the stored ones too. */
export interface SettingsInput {
	allowed_origin?: string;
	frame_ancestors?: string[];
	theme?: string;
}
