import type { CountryOption } from "../types/gen/CountryOption";
import type { QuotaGroup } from "../types/gen/QuotaGroup";

/**
 * Largest-remainder apportionment. Mirrors `survey_core::sampling::apportion`; the Rust
 * version is authoritative, this one only drives the live preview.
 */
export function apportion(percents: number[], n: number): number[] | null {
  const total = percents.reduce((a, b) => a + b, 0);
  if (total !== 100) return null;
  const exact = percents.map((p) => p * n);
  const counts = exact.map((x) => Math.floor(x / 100));
  let left = n - counts.reduce((a, b) => a + b, 0);
  const order = percents.map((_, i) => i).sort((a, b) => (exact[b] % 100) - (exact[a] % 100) || a - b);
  for (const i of order) {
    if (left === 0) break;
    counts[i] += 1;
    left -= 1;
  }
  return counts;
}

function evenSplit(labels: string[]): { label: string; percent: number }[] {
  const base = Math.floor(100 / labels.length);
  return labels.map((label, i) => ({ label, percent: base + (i === 0 ? 100 - base * labels.length : 0) }));
}

const AGE = [
  { label: "18–29", percent: 25 },
  { label: "30–44", percent: 30 },
  { label: "45–59", percent: 25 },
  { label: "60+", percent: 20 },
];
const INCOME = [
  { label: "Under $50k", percent: 30 },
  { label: "$50k–$100k", percent: 40 },
  { label: "Over $100k", percent: 30 },
];

/** Default quota groups for a country selection (docs/DATA_FLOW.md, Step 1). */
export function defaultQuotaGroups(selected: CountryOption[]): QuotaGroup[] {
  const groups: QuotaGroup[] = [];
  if (selected.length > 1) {
    groups.push({ key: "country", label: "Country", rows: evenSplit(selected.map((c) => c.name)) });
  }
  groups.push({ key: "age", label: "Age", rows: AGE });
  if (selected.length === 1 && selected[0].regions.length > 0) {
    groups.push({ key: "region", label: "Region", rows: evenSplit(selected[0].regions) });
  }
  groups.push({ key: "income", label: "Household income", rows: INCOME });
  return groups;
}
