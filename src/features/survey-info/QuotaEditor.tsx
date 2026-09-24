import { apportion } from "../../lib/quota";
import type { QuotaGroup } from "../../types/gen/QuotaGroup";

type Props = { size: number; groups: QuotaGroup[]; onChange: (groups: QuotaGroup[]) => void };

export function QuotaEditor({ size, groups, onChange }: Props) {
  const setPercent = (gi: number, ri: number, percent: number) =>
    onChange(groups.map((g, i) => (i !== gi ? g : { ...g, rows: g.rows.map((r, j) => (j === ri ? { ...r, percent } : r)) })));

  return (
    <div className="grid grid-cols-3 gap-5">
      {groups.map((g, gi) => {
        const counts = apportion(g.rows.map((r) => r.percent), size);
        const total = g.rows.reduce((a, r) => a + r.percent, 0);
        return (
          <fieldset key={g.key} className="flex flex-col gap-2.5 rounded-[22px] border border-[#e2e2dc] bg-white px-5 py-4">
            <legend className="px-2 font-display text-[15px] font-medium">{g.label}</legend>
            {g.rows.map((r, ri) => (
              <div key={r.label} className="flex items-center gap-2.5">
                <label htmlFor={`${g.key}-${ri}`} className="flex-1 text-[15px]">{r.label}</label>
                <input
                  id={`${g.key}-${ri}`}
                  type="number"
                  min={0}
                  max={100}
                  value={r.percent}
                  onChange={(e) => setPercent(gi, ri, Number(e.target.value) || 0)}
                  className="h-11 w-20 rounded-full border border-line text-center"
                />
                <span className="w-12 text-right font-mono text-sm">{counts ? counts[ri] : "—"}</span>
              </div>
            ))}
            <div className={`border-t border-[#efefea] pt-2 text-[13px] font-semibold ${counts ? "text-ok" : "text-bad"}`}>
              {counts ? `Total 100% · ${size} people` : `Total ${total}% · must be 100%`}
            </div>
          </fieldset>
        );
      })}
    </div>
  );
}
