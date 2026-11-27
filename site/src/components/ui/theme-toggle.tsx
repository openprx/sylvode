import type { JSX } from "@solidjs/web";
import { Moon, Sun } from "lucide";
import { createSignal } from "solid-js";
import { Button } from "./button";
import { Icon } from "./icon";

export type Theme = "light" | "dark";
const KEY = "sv-site-theme";

function systemTheme(): Theme {
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function currentTheme(): Theme {
  const pinned = document.documentElement.dataset.theme;
  return pinned === "light" || pinned === "dark" ? pinned : systemTheme();
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    // storage may be unavailable; the choice then lasts for this page view only
  }
}

export function ThemeToggle(): JSX.Element {
  const [theme, setTheme] = createSignal<Theme>(currentTheme());
  const toggle = () => {
    const next: Theme = theme() === "dark" ? "light" : "dark";
    applyTheme(next);
    setTheme(next);
  };
  return (
    <Button
      variant="ghost"
      size="sm"
      aria-label={theme() === "dark" ? "Switch to light theme" : "Switch to dark theme"}
      onClick={toggle}
    >
      <Icon icon={theme() === "dark" ? Sun : Moon} />
    </Button>
  );
}
