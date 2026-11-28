import tailwindcss from "@tailwindcss/vite";
import { defineConfig, loadEnv } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig(({ mode }) => {
  const base = loadEnv(mode, ".", "VITE_").VITE_SV_SITE_BASE ?? "/site/";
  if (!base.startsWith("/") || !base.endsWith("/")) {
    throw new Error(`VITE_SV_SITE_BASE must start and end with "/", got "${base}"`);
  }
  return {
    base,
    plugins: [solid(), tailwindcss()],
    build: {
      target: "es2022",
      outDir: "dist",
    },
  };
});
