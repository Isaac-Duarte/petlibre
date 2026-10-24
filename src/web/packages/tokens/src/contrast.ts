// WCAG 2.2 contrast between two sRGB colors, as the success criteria define
// it (1.4.3 text, 1.4.11 non-text). No dependency: it is two formulas.

function linear(channel: number): number {
  const value = channel / 255;
  return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

/** A `#RRGGBB` color's relative luminance. */
export function luminance(hex: string): number {
  if (!/^#[0-9a-f]{6}$/i.test(hex)) throw new Error(`not a #RRGGBB color: ${hex}`);

  const rgb = Number.parseInt(hex.slice(1), 16);
  return 0.2126 * linear((rgb >> 16) & 0xff) + 0.7152 * linear((rgb >> 8) & 0xff) + 0.0722 * linear(rgb & 0xff);
}

/** The contrast ratio of two colors, from 1 to 21, in either order. */
export function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}
