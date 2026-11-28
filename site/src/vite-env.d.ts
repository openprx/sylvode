/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Deployment base, starts and ends with `/`; default `/site/`. */
  readonly VITE_SV_SITE_BASE?: string;
  readonly VITE_SV_SITE_API?: string;
  readonly VITE_SV_SITE_ROUTING?: "hash" | "browser";
  readonly VITE_SV_SITE_SLUG?: string;
  /** `1` builds the fixture-backed mock mode (never enabled by default). */
  readonly VITE_SV_SITE_MOCK?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
