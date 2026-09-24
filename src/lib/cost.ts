/** Dollar amounts for the cost estimate (Step 3) and the live cost card (Step 4). */
export function usd(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "$—";
  if (n > 0 && n < 0.01) return "< $0.01";
  return `$${n.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

/** Token counts like "1.2M" or "85k". */
export function tokens(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${Math.round(n / 1e3)}k`;
  return String(n);
}
