import { onDestroy, onMount } from 'svelte';
import { afterNavigate, beforeNavigate } from '$app/navigation';
import { page } from '$app/state';
import { FlowObjectRepository } from '#lib/flow/object-repository.js';
import { FlowTaskSessions } from '#lib/workbench/flow-sessions.js';
import { navigateTask } from '#lib/workbench/navigation.js';
import { taskResource, type TaskWorkbench } from '#lib/workbench/tabs.js';

const REFRESH_MS = 15000;

export interface WorkbenchWiringOptions {
	workbench: TaskWorkbench;
	/** Signed-in user (null while signed out) and whether the profile check is still running. */
	session: { readonly userId: string | null; readonly loadingProfile: boolean };
	/** The element that scrolls the active task's content, and whether a Flow canvas owns scrolling. */
	scroll: { root(): HTMLElement | null; flowCanvas(): boolean };
}

export interface WorkbenchWiring {
	/** Flow task sessions of the signed-in user; null until the user is known. */
	readonly flowTasks: FlowTaskSessions | null;
}

/**
 * Connects the task workbench to navigation and to the signed-in user: tab capture before a
 * navigation, the ninth-tab rule (a blocked navigation is cancelled and the strip asks which tab
 * to close), tab visits and scroll restore after it, the account switch, the periodic refresh and
 * the unload guard. Call it during the layout's initialisation.
 */
export function wireWorkbench(options: WorkbenchWiringOptions): WorkbenchWiring {
	const { workbench, session, scroll } = options;
	let flowTasks = $state.raw<FlowTaskSessions | null>(null);
	let restoredAccount: string | null = null;

	function scrollOwner(): HTMLElement | null {
		const root = scroll.root();
		return scroll.flowCanvas()
			? (root?.querySelector<HTMLElement>('[data-flow-scroll-owner]') ?? root)
			: root;
	}
	function restoreScroll(value: number) {
		const owner = scrollOwner();
		if (owner) owner.scrollTop = value;
		window.scrollTo(0, scroll.flowCanvas() ? 0 : value);
	}
	function captureTask() {
		const key = workbench.current.active;
		if (key && taskResource(page.url.href)?.key === key) {
			workbench.setScroll(key, scrollOwner()?.scrollTop ?? window.scrollY);
			workbench.capture(key);
		}
	}
	workbench.onViewReady = (key) => {
		if (key !== workbench.current.active) return;
		const tab = workbench.current.tabs.find((item) => item.key === key);
		if (tab) requestAnimationFrame(() => restoreScroll(tab.scroll));
	};

	beforeNavigate((navigation) => {
		captureTask();
		if (navigation.to && !workbench.canOpen(navigation.to.url.href)) {
			workbench.block(navigation.to.url.href);
			navigation.cancel();
		}
	});
	afterNavigate(() => {
		if (!workbench.current.userId) return;
		workbench.visit(page.url.href);
		const tab = workbench.current.tabs.find((item) => item.key === workbench.current.active);
		if (tab) requestAnimationFrame(() => restoreScroll(tab.scroll));
	});

	$effect(() => {
		const user = session.userId;
		if (session.loadingProfile) {
			if (flowTasks && flowTasks.userId !== user) {
				flowTasks.dispose();
				flowTasks = null;
				workbench.activateUser(null);
			}
			return;
		}
		workbench.activateUser(user);
		if (flowTasks?.userId !== user) {
			flowTasks?.dispose();
			flowTasks = user ? new FlowTaskSessions(user, workbench, new FlowObjectRepository()) : null;
		}
		if (user && restoredAccount !== user) {
			restoredAccount = user;
			const previous = workbench.current.tabs.find((tab) => tab.key === workbench.current.active);
			if (page.url.pathname === '/workspace' && !page.url.search && previous) {
				void navigateTask(previous.url, true);
				return;
			}
			workbench.restoreRoute(page.url.href);
			return;
		}
		if (user) workbench.visit(page.url.href);
	});

	onMount(() => {
		const refresh = () => {
			if (document.visibilityState === 'visible' && navigator.onLine)
				void workbench.refreshActive();
		};
		const beforeUnload = (event: BeforeUnloadEvent) => {
			workbench.capture();
			if (workbench.hasDirty()) {
				event.preventDefault();
				event.returnValue = '';
			}
		};
		const timer = setInterval(refresh, REFRESH_MS);
		window.addEventListener('focus', refresh);
		window.addEventListener('beforeunload', beforeUnload);
		document.addEventListener('visibilitychange', refresh);
		return () => {
			clearInterval(timer);
			window.removeEventListener('focus', refresh);
			window.removeEventListener('beforeunload', beforeUnload);
			document.removeEventListener('visibilitychange', refresh);
		};
	});
	onDestroy(() => {
		flowTasks?.dispose();
		workbench.dispose();
	});

	return {
		get flowTasks() {
			return flowTasks;
		}
	};
}
