// Applies the dark class before the first paint, so a user on the system dark theme (the default)
// never sees the light page flash. Loaded synchronously from <head> in app.html (the Content
// Security Policy allows 'self' scripts, not inline ones). Mirrors resolveTheme() in src/lib/theme.ts:
// the stored choice is 'light', 'dark' or 'system'; anything else means 'system'.
(function () {
	try {
		var stored = localStorage.getItem('theme');
		var mode = stored === 'light' || stored === 'dark' || stored === 'system' ? stored : 'system';
		var dark =
			mode === 'dark' ||
			(mode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
		if (dark) document.documentElement.classList.add('dark');
	} catch (error) {
		// Storage or matchMedia unavailable: leave the light default.
	}
})();
