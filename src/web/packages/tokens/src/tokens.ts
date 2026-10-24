// assets/brand/tokens.json, read, checked and written out as CSS custom
// properties. The JSON is the one source for the web and (later) the Flutter
// theme; this module is pure so the generator and the tests share it.

import { contrast } from "./contrast.ts";

export type Palette = Record<string, string>;
export type Theme = Record<string, string>;

/** A text and background role pair that must reach `min` in every theme. */
export interface Pair {
  text: string;
  on: string;
  min: number;
}

export interface Tokens {
  color: Palette;
  theme: { light: Theme; dark: Theme };
  contrast: Pair[];
  font: { family: string; mono: string; weights: Record<string, number> };
  size: Record<string, number>;
  space: Record<string, number>;
  radius: Record<string, number>;
  focus: { width: number; offset: number };
  elevation: Record<string, string>;
  motion: Record<string, number>;
}

export const THEMES = ["light", "dark"] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function record(value: unknown, where: string): Record<string, unknown> {
  if (!isRecord(value)) throw new Error(`${where} is not an object`);
  return value;
}

function strings(value: unknown, where: string): Record<string, string> {
  const entries = Object.entries(record(value, where)).map(([key, item]) => {
    if (typeof item !== "string") throw new Error(`${where}.${key} is not a string`);
    return [key, item] as const;
  });
  return Object.fromEntries(entries);
}

function numbers(value: unknown, where: string): Record<string, number> {
  const entries = Object.entries(record(value, where)).map(([key, item]) => {
    if (typeof item !== "number") throw new Error(`${where}.${key} is not a number`);
    return [key, item] as const;
  });
  return Object.fromEntries(entries);
}

function text(value: unknown, where: string): string {
  if (typeof value !== "string") throw new Error(`${where} is not a string`);
  return value;
}

function number(value: unknown, where: string): number {
  if (typeof value !== "number") throw new Error(`${where} is not a number`);
  return value;
}

/** The parsed file, or an error naming the first field out of shape. */
export function parse(json: unknown): Tokens {
  const root = record(json, "tokens");
  const theme = record(root.theme, "theme");
  const font = record(root.font, "font");
  const focus = record(root.focus, "focus");
  if (!Array.isArray(root.contrast)) throw new Error("contrast is not a list");

  return {
    color: strings(root.color, "color"),
    theme: { light: strings(theme.light, "theme.light"), dark: strings(theme.dark, "theme.dark") },
    contrast: root.contrast.map((item: unknown, i) => {
      const pair = record(item, `contrast[${i}]`);
      return {
        text: text(pair.text, `contrast[${i}].text`),
        on: text(pair.on, `contrast[${i}].on`),
        min: number(pair.min, `contrast[${i}].min`),
      };
    }),
    font: {
      family: text(font.family, "font.family"),
      mono: text(font.mono, "font.mono"),
      weights: numbers(font.weights, "font.weights"),
    },
    size: numbers(root.size, "size"),
    space: numbers(root.space, "space"),
    radius: numbers(root.radius, "radius"),
    focus: { width: number(focus.width, "focus.width"), offset: number(focus.offset, "focus.offset") },
    elevation: strings(root.elevation, "elevation"),
    motion: numbers(root.motion, "motion"),
  };
}

/** One measured pair, for the failure list and the evidence table. */
export interface Measured extends Pair {
  theme: (typeof THEMES)[number];
  ratio: number;
}

/** Every named pair measured in both themes. */
export function measure(tokens: Tokens): Measured[] {
  return THEMES.flatMap((theme) =>
    tokens.contrast.map((pair) => ({
      ...pair,
      theme,
      ratio: contrast(resolve(tokens, theme, pair.text), resolve(tokens, theme, pair.on)),
    })),
  );
}

function resolve(tokens: Tokens, theme: (typeof THEMES)[number], role: string): string {
  const name = tokens.theme[theme][role];
  if (name === undefined) throw new Error(`theme.${theme} has no role ${role}`);
  const hex = tokens.color[name];
  if (hex === undefined) throw new Error(`theme.${theme}.${role} names ${name}, which is not in color`);
  return hex;
}

/**
 * Everything wrong with the tokens: themes with different roles, a role that
 * names no color, a pair under its minimum. Empty when the file is sound.
 */
export function problems(tokens: Tokens): string[] {
  const found: string[] = [];

  const light = Object.keys(tokens.theme.light).sort();
  const dark = Object.keys(tokens.theme.dark).sort();
  if (light.join() !== dark.join()) found.push(`the themes define different roles: ${light.join()} / ${dark.join()}`);

  for (const theme of THEMES) {
    for (const [role, name] of Object.entries(tokens.theme[theme])) {
      if (!(name in tokens.color)) found.push(`theme.${theme}.${role} names ${name}, which is not in color`);
    }
  }
  if (found.length > 0) return found;

  for (const m of measure(tokens)) {
    if (m.ratio < m.min) found.push(`${m.theme}: ${m.text} on ${m.on} is ${m.ratio.toFixed(2)}:1, under ${m.min}:1`);
  }
  return found;
}

const kebab = (name: string) => name.replaceAll("_", "-");

function stack(family: string, fallback: string): string {
  // Fontsource registers the variable cut as "<family> Variable".
  return `"${family} Variable", "${family}", ${fallback}`;
}

function themeBlock(selector: string, tokens: Tokens, theme: (typeof THEMES)[number], indent: string): string[] {
  const lines = [`${indent}${selector} {`, `${indent}  color-scheme: ${theme};`];
  for (const [role, name] of Object.entries(tokens.theme[theme])) {
    lines.push(`${indent}  --color-${kebab(role)}: ${tokens.color[name] ?? name};`);
  }
  lines.push(`${indent}}`);
  return lines;
}

/** The CSS custom properties for both themes, in a fixed order. */
export function render(tokens: Tokens): string {
  const px = (prefix: string, values: Record<string, number>) =>
    Object.entries(values).map(([key, value]) => `  --${prefix}-${kebab(key)}: ${value}px;`);

  const root = [
    `  --font: ${stack(tokens.font.family, 'system-ui, -apple-system, "Segoe UI", sans-serif')};`,
    `  --mono: ${stack(tokens.font.mono, "ui-monospace, Menlo, monospace")};`,
    ...Object.entries(tokens.font.weights).map(([key, value]) => `  --weight-${kebab(key)}: ${value};`),
    ...px("text", tokens.size),
    ...px("space", tokens.space),
    ...px("radius", tokens.radius),
    `  --focus-width: ${tokens.focus.width}px;`,
    `  --focus-offset: ${tokens.focus.offset}px;`,
    // A ring in the surface color first, so the focus color reads on any fill.
    "  --focus: 0 0 0 var(--focus-offset) var(--color-surface),",
    "    0 0 0 calc(var(--focus-offset) + var(--focus-width)) var(--color-focus);",
    ...Object.entries(tokens.elevation).map(([key, value]) => `  --shadow-${kebab(key)}: ${value};`),
    ...Object.entries(tokens.motion).map(([key, value]) => `  --motion-${kebab(key)}: ${value}ms;`),
  ];

  const light = themeBlock(":root", tokens, "light", "");
  return [
    "/* Generated from assets/brand/tokens.json by @petlibre/tokens. Do not edit: run `pnpm generate`. */",
    "",
    ...light.slice(0, -1),
    ...root,
    "}",
    "",
    "/* Dark follows the system unless the page sets data-theme; both blocks define the same names. */",
    "@media (prefers-color-scheme: dark) {",
    ...themeBlock(':root:not([data-theme="light"])', tokens, "dark", "  "),
    "}",
    "",
    ...themeBlock(':root[data-theme="dark"]', tokens, "dark", ""),
    "",
  ].join("\n");
}
