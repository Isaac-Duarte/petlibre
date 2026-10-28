import { describe, expect, test } from "vitest";
import { NAV } from "#lib/nav.ts";
import { docs, resolveLink } from "#lib/server/content.ts";

const listed = NAV.flatMap((group) => group.entries.map((entry) => entry.file));

describe("the sidebar", () => {
  test("lists every doc exactly once", () => {
    expect([...listed].sort()).toEqual([...docs.keys()].sort());
  });
});

describe("links", () => {
  test("a doc link becomes its route, a folder README the folder", () => {
    expect(resolveLink("pairing.md#keys", "firmware/device-api.md")).toBe("/firmware/pairing#keys");
    expect(resolveLink("../devices/plaf103/README.md", "firmware/qa.md")).toBe("/devices/plaf103");
    expect(resolveLink("README.md", "README.md")).toBe("/");
  });

  test("a link outside docs/ goes to GitHub", () => {
    expect(resolveLink("../../deny.toml", "development/rust.md")).toBe(
      "https://github.com/Isaac-Duarte/wellfed/blob/master/deny.toml",
    );
  });

  test("anchors and absolute URLs are left alone", () => {
    expect(resolveLink("#evidence", "firmware/qa.md")).toBe("#evidence");
    expect(resolveLink("https://example.com/a.md", "firmware/qa.md")).toBe("https://example.com/a.md");
  });
});
