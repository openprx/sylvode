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
	nav: unknown;
	position: number;
	installation_id: string | null;
}

export interface SiteBinding {
	site_id: string;
	binding_key: string;
	form_id: string;
	projection: {
		visitor?: {
			read?: { fields: string[]; scope: string };
			submit?: { fields: string[] };
		};
		operator?: {
			read?: { fields: string[]; scope: string };
			update?: { fields: string[] };
		};
	};
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
