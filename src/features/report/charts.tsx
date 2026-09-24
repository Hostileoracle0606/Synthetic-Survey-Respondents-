/**
 * Report charts, drawn with plain SVG/CSS. Colours follow the validated reference palette
 * (dataviz skill): categorical slots in fixed order for pies, one hue for single-series
 * bars, blue↔red with a grey midpoint for scales. Every chart shows its numbers as text
 * (legend or labels), so colour never carries meaning alone, and marks have hover titles.
 */
import { useState } from "react";
import type { ReportRow } from "../../types/gen/ReportRow";
import type { ThemeSummary } from "../../types/gen/ThemeSummary";

export const CATEGORICAL = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300"];
const SERIES = "#2a78d6";
const NEUTRAL = "#e4e2dc";

const fmt = (p: number) => `${p % 1 === 0 ? p.toFixed(0) : p.toFixed(1)}%`;

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

function Legend({ rows, colours }: { rows: ReportRow[]; colours: string[] }) {
  return (
    <ul className="m-0 flex list-none flex-col gap-1.5 p-0 text-sm">
      {rows.map((r, i) => (
        <li key={r.key} className="grid grid-cols-[12px_1fr_auto_auto] items-center gap-2">
          <span className="h-3 w-3 rounded-[3px]" style={{ background: colours[i] }} aria-hidden />
          <span className="truncate" title={r.label}>{r.label}</span>
          <span className="font-mono text-xs text-muted">{r.count}</span>
          <span className="w-12 text-right font-medium">{fmt(r.percent)}</span>
        </li>
      ))}
    </ul>
  );
}

/** Horizontal bars, one hue; the % sits at the end of each bar. */
export function Bars({ rows, note }: { rows: ReportRow[]; note?: string }) {
  const top = Math.max(1, ...rows.map((r) => r.percent));
  return (
    <div className="flex flex-col gap-1.5">
      {rows.map((r) => (
        <div key={r.key} className="grid grid-cols-[minmax(120px,34%)_1fr_52px] items-center gap-3 text-sm" title={`${r.label}: ${r.count} (${fmt(r.percent)})`}>
          <span className="truncate" title={r.label}>{r.label}</span>
          <span className="h-3.5">
            <span className="block h-3.5 rounded-r-[4px]" style={{ width: `${(r.percent / top) * 100}%`, minWidth: r.count ? 2 : 0, background: SERIES }} />
          </span>
          <span className="text-right font-medium">{fmt(r.percent)}</span>
        </div>
      ))}
      {note && <p className="m-0 mt-1 text-xs text-muted">{note}</p>}
    </div>
  );
}

/** Pie for single choice with up to 6 options; the reader can switch to bars. */
export function Pie({ rows }: { rows: ReportRow[] }) {
  const [asBars, setAsBars] = useState(false);
  const total = rows.reduce((a, r) => a + r.count, 0);
  let angle = -Math.PI / 2;
  const arcs = rows.map((r, i) => {
    const a0 = angle;
    const sweep = total ? (r.count / total) * Math.PI * 2 : 0;
    angle += sweep;
    const p = (a: number) => `${60 + 56 * Math.cos(a)} ${60 + 56 * Math.sin(a)}`;
    const d = sweep >= Math.PI * 2 - 1e-9
      ? "M 60 4 A 56 56 0 1 1 59.99 4 Z"
      : `M 60 60 L ${p(a0)} A 56 56 0 ${sweep > Math.PI ? 1 : 0} 1 ${p(a0 + sweep)} Z`;
    return { d, r, colour: CATEGORICAL[i % CATEGORICAL.length], show: sweep > 0 };
  });
  return (
    <div className="flex flex-col gap-2">
      <div className="flex justify-end">
        <button type="button" className="text-xs text-muted underline" onClick={() => setAsBars(!asBars)}>{asBars ? "Show as pie" : "Show as bars"}</button>
      </div>
      {asBars ? (
        <Bars rows={rows} />
      ) : (
        <div className="grid grid-cols-[132px_1fr] items-center gap-6">
          <svg viewBox="0 0 120 120" width="132" height="132" role="img" aria-label="Pie chart">
            {arcs.filter((a) => a.show).map((a) => (
              <path key={a.r.key} d={a.d} fill={a.colour} stroke="#fff" strokeWidth="2" strokeLinejoin="round">
                <title>{`${a.r.label}: ${a.r.count} (${fmt(a.r.percent)})`}</title>
              </path>
            ))}
            {total === 0 && <circle cx="60" cy="60" r="56" fill={NEUTRAL} />}
          </svg>
          <Legend rows={rows} colours={rows.map((_, i) => CATEGORICAL[i % CATEGORICAL.length])} />
        </div>
      )}
    </div>
  );
}

/** Likert: one 100% bar across the scale, then the legend with every point's share. */
export function Diverging({ rows, mean }: { rows: ReportRow[]; mean: number | null }) {
  const colours = divergingColours(rows.length);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex h-7 w-full gap-[2px] overflow-hidden rounded-[4px]" role="img" aria-label="Distribution across the scale">
        {rows.map((r, i) =>
          r.percent > 0 ? (
            <span key={r.key} className="flex h-7 items-center justify-center text-[11px] font-medium" title={`${r.label}: ${r.count} (${fmt(r.percent)})`}
              style={{ width: `${r.percent}%`, background: colours[i], color: i === (rows.length - 1) / 2 ? "#3a3a36" : "#fff" }}>
              {r.percent >= 8 ? fmt(r.percent) : ""}
            </span>
          ) : null,
        )}
      </div>
      <div className="grid grid-cols-[1fr_auto] items-start gap-6">
        <Legend rows={rows} colours={colours} />
        {mean != null && (
          <div className="text-right">
            <span className="block text-xs text-muted">Mean</span>
            <span className="font-display text-2xl font-semibold">{mean.toFixed(2)}</span>
          </div>
        )}
      </div>
    </div>
  );
}

/** Numeric: columns per range, with median and interquartile range. */
export function Histogram({ rows, median, q1, q3, unit }: { rows: ReportRow[]; median: number | null; q1: number | null; q3: number | null; unit: string | null }) {
  const top = Math.max(1, ...rows.map((r) => r.count));
  const u = unit ? ` ${unit}` : "";
  return (
    <div className="flex flex-col gap-3">
      <div className="flex gap-6 text-sm">
        <span><span className="text-muted">Median </span><strong className="font-display text-xl">{median ?? "—"}{median != null ? u : ""}</strong></span>
        <span><span className="text-muted">Middle half </span><strong>{q1 ?? "—"} – {q3 ?? "—"}{q1 != null ? u : ""}</strong></span>
      </div>
      <div className="flex h-36 items-end gap-[2px] border-b border-[#d8d8d2]" role="img" aria-label="Histogram">
        {rows.map((r) => (
          <span key={r.key} className="flex h-full flex-1 flex-col justify-end" title={`${r.label}${u}: ${r.count} (${fmt(r.percent)})`}>
            <span className="text-center text-[10px] text-muted">{r.count || ""}</span>
            <span className="block rounded-t-[4px]" style={{ height: `${(r.count / top) * 85}%`, background: SERIES }} />
          </span>
        ))}
      </div>
      <div className="flex gap-[2px] text-[10px] text-muted">
        {rows.map((r) => <span key={r.key} className="flex-1 truncate text-center" title={r.label}>{r.label}</span>)}
      </div>
    </div>
  );
}

/** Open answers: coded themes with counts and example quotes. */
export function Themes({ themes, samples, n }: { themes: ThemeSummary[]; samples: string[]; n: number }) {
  if (!themes.length) {
    return (
      <div className="flex flex-col gap-2 text-sm">
        <p className="m-0 text-muted">Themes appear once the answers are coded (this runs with the AI synthesis).</p>
        {samples.map((s, i) => <blockquote key={i} className="m-0 border-l-2 border-line pl-3 italic">“{s}”</blockquote>)}
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-3">
      <Bars rows={themes.map((t) => ({ key: String(t.id), label: t.label, count: t.count, percent: t.percent }))}
        note={`An answer can carry more than one theme, so shares can total more than 100% (n = ${n}).`} />
      <details className="text-sm">
        <summary className="cursor-pointer text-muted">Example answers</summary>
        <div className="mt-2 flex flex-col gap-3">
          {themes.map((t) => (
            <div key={t.id}>
              <p className="m-0 font-medium">{t.label} <span className="font-normal text-muted">— {t.description}</span></p>
              {t.quotes.map((q, i) => <blockquote key={i} className="m-0 mt-1 border-l-2 border-line pl-3 italic">“{q}”</blockquote>)}
            </div>
          ))}
        </div>
      </details>
    </div>
  );
}
