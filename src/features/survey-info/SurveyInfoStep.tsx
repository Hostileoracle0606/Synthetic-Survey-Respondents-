import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { useWizard } from "../../store/wizard";
import type { CountryOption } from "../../types/gen/CountryOption";
import type { ResearchType } from "../../types/gen/ResearchType";
import { AppShell } from "../../components/AppShell";
import { Arrow, Help, Label, areaField, pillButton, pillField, primaryButton } from "../../components/fields";
import { CountrySelect } from "./CountrySelect";
import { QuotaEditor } from "./QuotaEditor";

const RESEARCH_TYPES: { value: ResearchType; label: string }[] = [
  { value: "brand", label: "Brand Survey" },
  { value: "market_response", label: "Market Response Survey" },
  { value: "concept", label: "Concept Survey" },
];
const CATEGORIES = [
  { value: "mobile_phone", label: "Mobile phone" },
  { value: "tv", label: "TV" },
  { value: "refrigerator", label: "Refrigerator" },
  { value: "washing_machine", label: "Washing Machine" },
];

/** Step 1. Gate and fields follow docs/DATA_FLOW.md §3 (Step 1). */
export function SurveyInfoStep() {
  const { info, cohort, projectId, setInfo, setCohort, setProjectId, setCurrentCohort, setProgress, reach } = useWizard();
  const [countries, setCountries] = useState<CountryOption[]>([]);
  const [status, setStatus] = useState<string>("");

  useEffect(() => {
    api.listCountries().then(setCountries).catch((e) => setStatus(errorMessage(e)));
  }, []);

  const selectionKey = info.countries.join(",");
  useEffect(() => {
    // Quota groups follow the country selection; census shares where the country has a table.
    let live = true;
    api.defaultQuotas(info.countries).then((quotas) => live && setCohort({ quotas })).catch((e) => setStatus(errorMessage(e)));
    return () => {
      live = false;
    };
    // selectionKey (not info.countries) on purpose: it's a stable string, so this only refetches
    // when the actual country selection changes, not on every render that touches info.
  }, [selectionKey, setCohort]); // eslint-disable-line react-hooks/exhaustive-deps
  const censusNote = useMemo(() => {
    const selected = countries.filter((c) => info.countries.includes(c.code));
    if (selected.length === 1 && selected[0].hasCensusTable) return "Defaults match the adult population of this country (census data). Edit them to target a different audience.";
    if (selected.some((c) => !c.hasCensusTable)) return "No census table for some countries: people are drawn from these quotas only.";
    return "";
    // selectionKey stands in for info.countries here too, for the same reason as above.
  }, [countries, selectionKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const groupsOk = cohort.quotas.every((g) => g.rows.reduce((a, r) => a + r.percent, 0) === 100);
  const missing = [!info.researchType && "research type", info.countries.length === 0 && "a country", !info.title.trim() && "a project title"].filter(Boolean);
  const canGenerate = missing.length === 0 && groupsOk && cohort.size >= 1 && cohort.size <= 1000;
  const hint = missing.length ? `Choose ${missing.join(", ")} to continue` : groupsOk ? `≈ ${Math.ceil(cohort.size / 8)} Gemini calls · runs in the background` : "Each quota group must total 100%";

  async function generate() {
    setStatus("Saving…");
    try {
      const project = await api.saveSurveyInfo(projectId, info);
      setProjectId(project.id);
      setProgress({ done: 0, total: cohort.size, replaced: 0 });
      const started = await api.generateCohort(project.id, cohort, setProgress);
      setCurrentCohort(started);
      setStatus("");
      reach(1);
    } catch (e) {
      setStatus(errorMessage(e));
    }
  }

  return (
    <AppShell
      title="Create New"
      heading="Research Design"
      subtitle="Describe your survey objective. We’ll generate a tailored survey plan and questionnaire."
      footer={
        <>
          <button type="button" className={pillButton}>Cancel</button>
          <div className="flex items-center gap-4">
            <span role="status" className="text-sm text-muted">{status || hint}</span>
            <button type="button" className={primaryButton} disabled={!canGenerate} onClick={generate}>
              Generate Cohort <Arrow />
            </button>
          </div>
        </>
      }
    >
      <div className="grid grid-cols-2 gap-x-10 gap-y-7">
        <div className="flex flex-col gap-3">
          <Label htmlFor="research-type">Research Type</Label>
          <select
            id="research-type"
            className={pillField}
            value={info.researchType ?? ""}
            onChange={(e) => setInfo({ researchType: (e.target.value || null) as ResearchType | null })}
          >
            <option value="">Select</option>
            {RESEARCH_TYPES.map((t) => <option key={t.value} value={t.value}>{t.label}</option>)}
          </select>
        </div>
        <div className="flex flex-col gap-3">
          <Label htmlFor="category">Product Category</Label>
          <select id="category" className={pillField} value={info.productCategory ?? ""} onChange={(e) => setInfo({ productCategory: e.target.value })}>
            {CATEGORIES.map((c) => <option key={c.value} value={c.value}>{c.label}</option>)}
          </select>
        </div>
        <div className="col-span-2 flex flex-col gap-3">
          <Label htmlFor="title">Project Title</Label>
          <input id="title" className={pillField} value={info.title} onChange={(e) => setInfo({ title: e.target.value })} placeholder="e.g., Canada Mobile Phone Purchase & Usage Study" />
        </div>
        <div className="col-span-2 flex flex-col gap-2">
          <Label id="lbl-country">Target Country</Label>
          <Help>Select the countries where your target respondents live.</Help>
          <CountrySelect options={countries} value={info.countries} onChange={(c) => setInfo({ countries: c })} />
        </div>
        <div className="col-span-2 flex flex-col gap-2">
          <Label htmlFor="objective">Research Objective &amp; Requirements</Label>
          <Help>The more detail you give on research objectives, requirements and target respondent criteria, the better the Agent can propose a research design and questionnaire.</Help>
          <textarea id="objective" rows={4} className={areaField} value={info.researchGoal} onChange={(e) => setInfo({ researchGoal: e.target.value })} placeholder="e.g., Objective: To analyze purchase behavior, brand choice drivers, and usage patterns among Canadian smartphone owners." />
        </div>
      </div>

      <div className="flex flex-col gap-2 border-t border-divider pt-5">
        <h2 className="m-0 font-display text-2xl font-semibold">Audience</h2>
        <p className="m-0 text-muted">Who the synthetic respondents are. Head counts are exact and always add up to the sample size.</p>
      </div>
      <div className="flex items-center gap-5">
        <Label htmlFor="size">Number of respondents</Label>
        <input id="size" type="range" min={50} max={1000} step={50} value={cohort.size} onChange={(e) => setCohort({ size: Number(e.target.value) })} className="flex-1 accent-accent" />
        <label htmlFor="size-exact" className="sr-only">Exact number of respondents</label>
        <input id="size-exact" type="number" min={1} max={1000} value={cohort.size} onChange={(e) => setCohort({ size: Math.max(1, Math.min(1000, Number(e.target.value) || 1)) })} className="h-11 w-28 rounded-full border border-line text-center" />
      </div>
      {censusNote && <Help>{censusNote}</Help>}
      <QuotaEditor size={cohort.size} groups={cohort.quotas} onChange={(quotas) => setCohort({ quotas })} />
      <div className="flex flex-col gap-2">
        <Label htmlFor="screening">Screening Criteria</Label>
        <Help>Respondents who don’t meet these are replaced from the same quota group.</Help>
        <textarea id="screening" rows={2} className={areaField} value={cohort.screening} onChange={(e) => setCohort({ screening: e.target.value })} placeholder="e.g., Owns a smartphone. Plans to buy a new phone in the next 12 months." />
      </div>
    </AppShell>
  );
}
