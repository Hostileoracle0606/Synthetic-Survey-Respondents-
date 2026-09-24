import { describe, expect, it } from "vitest";
import { cohortIsOutOfDate } from "./cohortDrift";
import type { Cohort } from "../types/gen/Cohort";
import type { CohortConfig } from "../types/gen/CohortConfig";

const config: CohortConfig = {
  size: 200,
  seed: 4821,
  quotas: [{ key: "age", label: "Age", rows: [{ label: "18–29", percent: 50 }, { label: "30–44", percent: 50 }] }],
  screening: "Owns a smartphone.",
  nonBinaryShare: 0,
  countries: ["CA"],
};
const cohort: Cohort = {
  id: 1,
  projectId: 1,
  name: "Cohort v1",
  status: "ready",
  config,
  error: null,
  createdAt: new Date().toISOString(),
};

describe("cohortIsOutOfDate", () => {
  it("is false when Step 1 still matches the saved cohort", () => {
    expect(cohortIsOutOfDate(["CA"], config, cohort)).toBe(false);
  });

  it("ignores country order", () => {
    expect(cohortIsOutOfDate(["US", "CA"], { ...config, countries: ["CA", "US"] }, {
      ...cohort,
      config: { ...config, countries: ["CA", "US"] },
    })).toBe(false);
  });

  it("flags a country change", () => {
    expect(cohortIsOutOfDate(["US"], config, cohort)).toBe(true);
  });

  it("flags a size change", () => {
    expect(cohortIsOutOfDate(["CA"], { ...config, size: 300 }, cohort)).toBe(true);
  });

  it("flags a screening change", () => {
    expect(cohortIsOutOfDate(["CA"], { ...config, screening: "Owns a smartphone. Plans to buy soon." }, cohort)).toBe(true);
  });

  it("flags a quota change", () => {
    const changed = { ...config, quotas: [{ key: "age", label: "Age", rows: [{ label: "18–29", percent: 60 }, { label: "30–44", percent: 40 }] }] };
    expect(cohortIsOutOfDate(["CA"], changed, cohort)).toBe(true);
  });

  it("flags a non-binary share change", () => {
    expect(cohortIsOutOfDate(["CA"], { ...config, nonBinaryShare: 5 }, cohort)).toBe(true);
  });
});
