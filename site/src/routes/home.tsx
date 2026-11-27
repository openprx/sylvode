import type { JSX } from "@solidjs/web";
import SitePage from "./page";

/** `/` is the manifest page with route `/`. */
export default function Home(): JSX.Element {
  return <SitePage />;
}
