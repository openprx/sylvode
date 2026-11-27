import type { Component } from 'svelte';
import AppWindow from '@lucide/svelte/icons/app-window';
import Bell from '@lucide/svelte/icons/bell';
import Bot from '@lucide/svelte/icons/bot';
import Briefcase from '@lucide/svelte/icons/briefcase';
import Clock from '@lucide/svelte/icons/clock';
import FileText from '@lucide/svelte/icons/file-text';
import Flag from '@lucide/svelte/icons/flag';
import ExternalLink from '@lucide/svelte/icons/external-link';
import Folder from '@lucide/svelte/icons/folder';
import FolderKanban from '@lucide/svelte/icons/folder-kanban';
import House from '@lucide/svelte/icons/house';
import Globe from '@lucide/svelte/icons/globe';
import Kanban from '@lucide/svelte/icons/kanban';
import Landmark from '@lucide/svelte/icons/landmark';
import LayoutTemplate from '@lucide/svelte/icons/layout-template';
import LayoutList from '@lucide/svelte/icons/layout-list';
import Link from '@lucide/svelte/icons/link';
import ListChecks from '@lucide/svelte/icons/list-checks';
import Puzzle from '@lucide/svelte/icons/puzzle';
import Repeat from '@lucide/svelte/icons/repeat';
import Settings from '@lucide/svelte/icons/settings';
import Star from '@lucide/svelte/icons/star';
import Sparkles from '@lucide/svelte/icons/sparkles';
import Table2 from '@lucide/svelte/icons/table-2';
import Users from '@lucide/svelte/icons/users';
import Workflow from '@lucide/svelte/icons/workflow';
import type { GlyphName } from './rows';

export type GlyphComponent = Component<{ size?: number | string; 'aria-hidden'?: 'true' }>;

export const GLYPHS: Record<GlyphName, GlyphComponent> = {
	house: House,
	bell: Bell,
	'list-checks': ListChecks,
	clock: Clock,
	folder: Folder,
	'folder-kanban': FolderKanban,
	'layout-list': LayoutList,
	kanban: Kanban,
	repeat: Repeat,
	'table-2': Table2,
	'file-text': FileText,
	'app-window': AppWindow,
	settings: Settings,
	workflow: Workflow,
	bot: Bot,
	sparkles: Sparkles,
	landmark: Landmark,
	flag: Flag,
	users: Users,
	briefcase: Briefcase,
	link: Link,
	'external-link': ExternalLink,
	star: Star,
	'layout-template': LayoutTemplate,
	globe: Globe,
	puzzle: Puzzle
};
