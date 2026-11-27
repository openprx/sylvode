/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_SV_SITE_API?: string;
  readonly VITE_SV_SITE_ROUTING?: "hash" | "browser";
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
