/**
 * Chart colours, split out from charts.tsx (which also exports React components) so Fast
 * Refresh can hot-reload the components without a full-page reload. Values follow the
 * validated reference palette (dataviz skill): see charts.tsx for how they're used.
 */
export const CATEGORICAL = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300"];
export const SERIES = "#2a78d6";
export const NEUTRAL = "#e4e2dc";

function mix(a: string, b: string, t: number): string {
  const c = (h: string, i: number) => parseInt(h.slice(1 + i * 2, 3 + i * 2), 16);
  return `#${[0, 1, 2].map((i) => Math.round(c(a, i) + (c(b, i) - c(a, i)) * t).toString(16).padStart(2, "0")).join("")}`;
}

/** Colours for a k-point scale: red arm (low), grey midpoint when k is odd, blue arm (high). */
export function divergingColours(k: number): string[] {
  const arm = Math.floor(k / 2);
  const shade = (light: string, dark: string, i: number) => (arm <= 1 ? dark : mix(light, dark, i / (arm - 1)));
  const low = Array.from({ length: arm }, (_, i) => shade("#f4b4b3", "#c9302f", arm - 1 - i));
  const high = Array.from({ length: arm }, (_, i) => shade("#a9ccf5", "#1c5cab", i));
  return [...low, ...(k % 2 ? [NEUTRAL] : []), ...high];
}
