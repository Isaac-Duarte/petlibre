import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { contrast } from "./contrast.ts";
import { measure, parse, problems, render, type Tokens } from "./tokens.ts";

const source = join(import.meta.dirname, "../../../../../assets/brand/tokens.json");
const generated = join(import.meta.dirname, "tokens.gen.css");
const tokens = parse(JSON.parse(readFileSync(source, "utf8")));

describe("contrast", () => {
  it("matches the WCAG reference values", () => {
    expect(contrast("#000000", "#FFFFFF")).toBeCloseTo(21, 5);
    expect(contrast("#FFFFFF", "#FFFFFF")).toBe(1);
    // #767676 on white is the classic lightest AA grey.
    expect(contrast("#767676", "#FFFFFF")).toBeCloseTo(4.54, 2);
  });

  it("refuses anything but #RRGGBB", () => {
    expect(() => contrast("#fff", "#000000")).toThrow("not a #RRGGBB color");
  });
});

describe("tokens.json", () => {
  it("meets every named contrast pair in both themes", () => {
    expect(problems(tokens)).toEqual([]);
    expect(measure(tokens)).toHaveLength(tokens.contrast.length * 2);
  });

  it("is what tokens.gen.css was generated from", () => {
    // A stale commit fails here; `pnpm generate` rewrites the file.
    expect(readFileSync(generated, "utf8")).toBe(render(tokens));
  });

  it("names a contrast failure by theme and roles", () => {
    const dim: Tokens = { ...tokens, color: { ...tokens.color, text_muted: "#B0B8C8" } };
    expect(problems(dim)).toContainEqual(
      expect.stringMatching(/^light: text_muted on surface is \d\.\d\d:1, under 4.5:1$/),
    );
  });

  it("refuses a role that names no color, and themes that disagree", () => {
    const broken: Tokens = {
      ...tokens,
      theme: { light: { ...tokens.theme.light, accent: "missing" }, dark: { ...tokens.theme.dark, extra: "night" } },
    };
    expect(problems(broken)).toEqual([
      expect.stringContaining("the themes define different roles"),
      "theme.light.accent names missing, which is not in color",
    ]);
  });

  it("refuses a file out of shape", () => {
    expect(() => parse({ ...tokens, space: { "1": "4px" } })).toThrow("space.1 is not a number");
  });
});
