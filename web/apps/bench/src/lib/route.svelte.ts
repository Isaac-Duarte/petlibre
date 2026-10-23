// Hash routes, so a link to a run or session survives a reload and can be
// pasted to someone on the same tailnet: #/tests/itest-20261022-07.

export const TABS = ["overview", "tests", "sessions", "log"] as const;
export type Tab = (typeof TABS)[number];

function parse(hash: string): { tab: Tab; detail: string | null } {
  const [tab, ...rest] = hash.replace(/^#\/?/, "").split("/");
  const known = TABS.find((t) => t === tab) ?? "overview";
  const detail = rest.length ? decodeURIComponent(rest.join("/")) : null;
  return { tab: known, detail };
}

class Route {
  tab = $state<Tab>("overview");
  detail = $state<string | null>(null);

  constructor() {
    if (typeof window === "undefined") return;
    this.#read();
    window.addEventListener("hashchange", () => this.#read());
  }

  #read() {
    const { tab, detail } = parse(window.location.hash);
    this.tab = tab;
    this.detail = detail;
  }

  href(tab: Tab, detail?: string | null): string {
    return `#/${tab}${detail ? `/${encodeURIComponent(detail)}` : ""}`;
  }

  go(tab: Tab, detail?: string | null) {
    window.location.hash = this.href(tab, detail);
  }
}

export const route = new Route();
export { parse as parseRoute };
