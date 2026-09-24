import type { Cohort } from "../types/gen/Cohort";
import type { CohortConfig } from "../types/gen/CohortConfig";

/**
 * True when Step 1's live audience (countries, size, quotas or screening) no longer matches
 * what `cohort` was generated from (docs/DATA_FLOW.md §2: "Cohort out of date").
 */
export function cohortIsOutOfDate(liveCountries: string[], live: CohortConfig, saved: Cohort): boolean {
  const sortedEq = (a: string[], b: string[]) =>
    a.length === b.length && [...a].sort().every((v, i) => v === [...b].sort()[i]);
  return (
    !sortedEq(liveCountries, saved.config.countries) ||
    live.size !== saved.config.size ||
    live.screening.trim() !== saved.config.screening.trim() ||
    live.nonBinaryShare !== saved.config.nonBinaryShare ||
    JSON.stringify(live.quotas) !== JSON.stringify(saved.config.quotas)
  );
}
