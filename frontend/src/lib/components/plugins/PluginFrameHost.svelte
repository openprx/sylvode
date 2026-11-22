<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import {
		bridgeProtocolV2,
		clampFrameHeight,
		isShellSignal,
		parseBridgeV2,
		shellInitMessage,
		verifyPanelHtml,
		type BridgeErrorCode,
		type PluginHostContext as Context
	} from '#lib/plugins/bridge.js';
	import {
		createFrameLifecycle,
		suspendGraceMs,
		type FrameLifecycle
	} from '#lib/plugins/frame-lifecycle.js';
	import { createHostDispatcher, type HostOutcome } from '#lib/plugins/host-methods.js';

	let {
		html,
		webSha256,
		grants,
		context,
		transport,
		title,
		minHeight,
		maxHeight,
		onClose,
		onRetired,
		onConfirmApply
	}: {
		html: string;
		webSha256: string;
		grants: string[];
		context: Context;
		transport: (
			method: string,
			params: Record<string, unknown>,
			requestId: string
		) => Promise<{ ok: true; result: unknown } | { ok: false; code: string }>;
		title: string;
		minHeight: number;
		maxHeight: number;
		onClose: (reason: '' | string) => void;
		/**
		 * Called when this host runs again after it retired its frame silently (see
		 * `retiredSilently`). Read once, when the host opens. Without it, `onClose` gets the
		 * suspended reason, so the caller never shows an empty host.
		 */
		onRetired?: () => void;
		onConfirmApply?: () => Promise<boolean>;
	} = $props();

	// Everything that defines what runs in the frame is fixed when the host opens. A later
	// change to any of it (new version, new grants, new digest) suspends and closes the frame;
	// callers mount a fresh host for the new state.
	const identity = (values: { html: string; webSha256: string; grants: string[] }) =>
		JSON.stringify([values.html, values.webSha256, [...values.grants].sort()]);
	const opened = untrack(() => ({ html, webSha256, grants: [...grants] }));
	const openedIdentity = identity(opened);
	const reasons = {
		integrity: 'plugins.host.integrityFailed',
		load: 'plugins.host.loadFailed',
		suspended: 'plugins.host.suspended',
		changed: 'plugins.host.suspended',
		timeout: 'plugins.actionTimedOut'
	};
	const messages: Record<BridgeErrorCode, string> = {
		method_unavailable: 'plugins.host.methodUnavailable',
		invalid_params: 'plugins.host.invalidParams',
		busy: 'plugins.actionBusy',
		denied: 'plugins.accessChanged',
		stale: 'plugins.versionChanged',
		not_found: 'plugins.host.notFound',
		rate_limited: 'plugins.host.rateLimited',
		timeout: 'plugins.actionTimedOut',
		canceled: 'plugins.actionCanceled',
		preview_required: 'plugins.previewRequired',
		failed: 'plugins.actionFailed'
	};

	let stage = $state<'verifying' | 'opening' | 'ready' | 'closed'>('verifying');
	let height = $state(untrack(() => clampFrameHeight(minHeight, minHeight, maxHeight)));
	let frame: HTMLIFrameElement | undefined = $state();
	let port: MessagePort | undefined;
	let lifecycle: FrameLifecycle | undefined;
	let loadTimer: ReturnType<typeof setTimeout> | undefined;
	let finished = false;
	let sentContext = '';
	/**
	 * Set when the frame was retired because the caller unmounted or re-keyed this host (no
	 * `onClose`). Svelte keeps such a host alive during the outro grace period and resumes it,
	 * instead of mounting a new one, when the same block becomes current again before the outro
	 * ends (for example browser Back within 300 ms). A resumed host has no frame and can never get
	 * one, so it tells its caller as soon as it runs again; a host that is destroyed never does.
	 */
	let retiredSilently = $state(false);
	const retired = untrack(() => onRetired);

	const dispatcher = createHostDispatcher({
		grants: () => opened.grants,
		context: () => currentContext(),
		transport: (method, params, requestId) => transport(method, params, requestId),
		confirmApply: () => (onConfirmApply ? onConfirmApply() : Promise.resolve(false))
	});

	function currentContext(): Context {
		return $state.snapshot(context) as Context;
	}

	/**
	 * Stops serving the frame and retires its port. With a reason on a frame whose port
	 * is open, the plugin first gets `suspended` and a short grace period (see frame-lifecycle.ts):
	 * serving stops immediately, only the removal is delayed. `notify` is false when the caller
	 * unmounted us.
	 */
	function finish(reason: string, notify = true) {
		if (finished) return;
		finished = true;
		dispatcher.dispose();
		clearTimeout(loadTimer);
		if (reason !== '' && stage !== 'verifying')
			lifecycle?.suspend({
				protocol: bridgeProtocolV2,
				type: 'event',
				name: 'suspended',
				data: null
			});
		else lifecycle?.close();
		port = undefined;
		stage = 'closed';
		if (notify) onClose(reason);
		else retiredSilently = true;
	}

	/**
	 * Outro of the frame element. It runs when the frame is removed, including when a parent
	 * unmounts or re-keys this component without going through finish(): Svelte then keeps the
	 * element mounted for the returned duration, and the lifecycle closes it at the latest then.
	 */
	function holdWhileSuspending(node: HTMLElement) {
		finish(reasons.suspended, false);
		if (lifecycle?.state !== 'suspending') return { duration: 0 };
		node.inert = true;
		node.style.display = 'none';
		return { duration: suspendGraceMs };
	}

	onMount(() => {
		let active = true;
		void (async () => {
			let verified = false;
			try {
				verified = await verifyPanelHtml({ html: opened.html, web_sha256: opened.webSha256 });
			} catch {
				/* Unavailable integrity verification fails closed. */
			}
			if (!active || finished) return;
			if (!verified) {
				finish(reasons.integrity);
				return;
			}
			stage = 'opening';
			loadTimer = setTimeout(() => finish(reasons.load), 7000);
		})();
		return () => {
			active = false;
			finish(reasons.suspended, false);
			lifecycle?.close();
		};
	});

	// Effects of a host that is being removed (paused for its outro, or destroyed) do not run, so
	// this only fires once the host is live again after a silent retire.
	$effect(() => {
		if (!retiredSilently) return;
		untrack(() => {
			retiredSilently = false;
			if (retired) retired();
			else onClose(reasons.suspended);
		});
	});

	$effect(() => {
		const next = identity({ html, webSha256, grants });
		if (next !== openedIdentity) untrack(() => finish(reasons.changed));
	});

	$effect(() => {
		const snapshot = JSON.stringify($state.snapshot(context));
		untrack(() => {
			if (stage !== 'ready' || !port || snapshot === sentContext) return;
			sentContext = snapshot;
			port.postMessage({
				protocol: bridgeProtocolV2,
				type: 'event',
				name: 'context',
				data: JSON.parse(snapshot)
			});
		});
	});

	function onWindowMessage(event: MessageEvent) {
		if (finished || !frame || event.source !== frame.contentWindow || event.origin !== 'null')
			return;
		if (isShellSignal(event.data, 'sylvode-plugin-shell-invalidated')) {
			finish(reasons.load);
			return;
		}
		if (stage !== 'opening' || port || !isShellSignal(event.data, 'sylvode-plugin-shell-ready'))
			return;
		const initContext = currentContext();
		const channel = new MessageChannel();
		port = channel.port1;
		port.onmessage = ({ data }) => receive(data);
		const node = frame;
		lifecycle = createFrameLifecycle({
			port,
			dispose: () => dispatcher.dispose(),
			removeFrame: () => node.remove()
		});
		sentContext = JSON.stringify(initContext);
		const init = shellInitMessage({
			html: opened.html,
			grants: [...opened.grants],
			context: initContext
		});
		frame.contentWindow?.postMessage(init, '*', [channel.port2]);
	}

	function markReady() {
		clearTimeout(loadTimer);
		stage = 'ready';
		// A context change that arrived during the handshake is delivered once the plugin is ready.
		const latest = currentContext();
		const snapshot = JSON.stringify(latest);
		if (!port || snapshot === sentContext) return;
		sentContext = snapshot;
		port.postMessage({ protocol: bridgeProtocolV2, type: 'event', name: 'context', data: latest });
	}

	function receive(data: unknown) {
		if (finished) return;
		const message = parseBridgeV2(data);
		if (!message) return;
		if (message.type === 'ready') {
			if (stage === 'opening') markReady();
			return;
		}
		if (stage !== 'ready') return;
		if (message.type === 'close') finish('');
		else if (message.type === 'resize')
			height = clampFrameHeight(message.height, minHeight, maxHeight);
		else
			void dispatcher
				.handle(message.requestId, message.method, message.params)
				.then((outcome) => answer(message.requestId, outcome));
	}

	function answer(requestId: string, outcome: HostOutcome) {
		if (finished || !port) return;
		port.postMessage({
			protocol: bridgeProtocolV2,
			type: 'result',
			request_id: requestId,
			...(outcome.ok
				? { ok: true, result: outcome.result }
				: { ok: false, error: { code: outcome.code, message: $t(messages[outcome.code]) } })
		});
		if (!outcome.ok && outcome.close === 'suspend') finish(reasons.suspended);
		else if (!outcome.ok && outcome.close === 'timeout') finish(reasons.timeout);
	}

	// While focus is inside the frame, the parent's active element is the frame.
	let frameFocused = $state(false);
	function syncFrameFocus(): void {
		frameFocused = !!frame && document.activeElement === frame;
	}
</script>

<svelte:window
	onmessage={onWindowMessage}
	onblur={syncFrameFocus}
	onfocus={syncFrameFocus}
	onfocusin={syncFrameFocus}
/>

{#if stage === 'verifying' || stage === 'opening'}
	<p class="px-3 pb-2 text-sm text-fg-muted" role="status">
		{$t('plugins.host.loading')}
	</p>
{/if}
{#if stage === 'opening' || stage === 'ready'}
	<iframe
		bind:this={frame}
		{title}
		src="/plugin-host/frame.html"
		out:holdWhileSuspending|global
		sandbox="allow-scripts"
		referrerpolicy="no-referrer"
		class="block w-full border-0"
		class:keyboard-ring={frameFocused}
		onfocus={syncFrameFocus}
		onblur={syncFrameFocus}
		style:height={`${height}px`}
		hidden={stage !== 'ready'}
	></iframe>
{/if}

<style>
	/* The frame's own :focus and :focus-visible do not match while focus is inside its document,
	   so the ring follows `frameFocused`, which is read from the parent's active element. */
	iframe.keyboard-ring {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 2px;
	}
</style>
