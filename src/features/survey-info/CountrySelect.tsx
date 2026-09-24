import { useState } from "react";
import type { CountryOption } from "../../types/gen/CountryOption";

type Props = { options: CountryOption[]; value: string[]; onChange: (codes: string[]) => void };

/** Multi-select with search, as in the prototype's Target Country field. */
export function CountrySelect({ options, value, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const shown = options.filter((o) => o.name.toLowerCase().includes(query.toLowerCase()));
  const selected = options.filter((o) => value.includes(o.code));
  const toggle = (code: string) => onChange(value.includes(code) ? value.filter((c) => c !== code) : [...value, code]);

  return (
    <div className="relative">
      <button
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-labelledby="lbl-country"
        onClick={() => setOpen(!open)}
        className={`flex min-h-14 w-full items-center justify-between gap-3 rounded-full border bg-white py-2.5 pl-6 pr-5 text-left ${open ? "border-accent ring-4 ring-accent/15" : "border-line"}`}
      >
        <span className="flex flex-wrap items-center gap-2">
          {selected.length === 0 && <span className="text-muted">Select</span>}
          {selected.map((c) => (
            <span key={c.code} className="rounded-full bg-[#eef1fd] px-3 py-0.5 text-[13px] font-medium text-accent-dark">
              {c.name}
            </span>
          ))}
        </span>
        <span aria-hidden>{open ? "▴" : "▾"}</span>
      </button>
      {open && (
        <div className="absolute inset-x-0 top-[calc(100%+8px)] z-20 rounded-[22px] bg-white pb-2 shadow-xl">
          <div className="border-b border-divider px-5 py-3.5">
            <label htmlFor="country-search" className="sr-only">Search countries</label>
            <input id="country-search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search..." className="w-full bg-transparent text-base outline-none" />
          </div>
          <ul role="listbox" aria-multiselectable className="max-h-64 overflow-y-auto px-2.5 pt-2">
            {shown.map((o) => (
              <li key={o.code}>
                <label className="flex min-h-11 cursor-pointer items-center gap-3 rounded-xl px-3.5 hover:bg-[#f1f1ee]">
                  <input type="checkbox" checked={value.includes(o.code)} onChange={() => toggle(o.code)} className="h-[18px] w-[18px] accent-accent" />
                  <span className="flex-1">{o.name}</span>
                  {!o.hasCensusTable && <span className="text-xs text-muted">Quotas only, no census table</span>}
                </label>
              </li>
            ))}
            {shown.length === 0 && <li className="px-3.5 py-3 text-sm text-muted">No countries match.</li>}
          </ul>
        </div>
      )}
    </div>
  );
}
