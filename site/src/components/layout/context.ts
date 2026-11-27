import { createContext, useContext } from "solid-js";
import type { LayoutRuntime } from "../../lib/layout/runtime";

export const RuntimeContext = createContext<LayoutRuntime>();

export function useRuntime(): LayoutRuntime {
  return useContext(RuntimeContext);
}
