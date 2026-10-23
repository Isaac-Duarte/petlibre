// Auto follows the system; a choice is the viewer's own, kept in this browser.

export type Theme = "auto" | "light" | "dark";
const KEY = "petlibre.theme";

function stored(): Theme {
  try {
    const value = localStorage.getItem(KEY);
    return value === "light" || value === "dark" ? value : "auto";
  } catch {
    return "auto";
  }
}

class ThemeChoice {
  value = $state<Theme>("auto");

  constructor() {
    if (typeof document === "undefined") return;
    this.value = stored();
    this.#apply();
  }

  cycle() {
    this.value = this.value === "auto" ? "light" : this.value === "light" ? "dark" : "auto";
    try {
      localStorage.setItem(KEY, this.value);
    } catch {
      // Private windows may refuse storage; the choice lasts this visit.
    }
    this.#apply();
  }

  #apply() {
    if (this.value === "auto") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = this.value;
  }
}

export const theme = new ThemeChoice();
