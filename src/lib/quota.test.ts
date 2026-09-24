import { describe, expect, it } from "vitest";
import { apportion, defaultQuotaGroups } from "./quota";
import type { CountryOption } from "../types/gen/CountryOption";

const ca: CountryOption = {
  code: "CA",
  name: "Canada",
  hasCensusTable: true,
  regions: ["Atlantic", "Quebec", "Ontario", "Prairies", "British Columbia", "Territories"],
};
const us: CountryOption = { code: "US", name: "United States", hasCensusTable: false, regions: [] };

describe("apportion", () => {
  it("matches the Rust fixed cases", () => {
    expect(apportion([25, 30, 25, 20], 200)).toEqual([50, 60, 50, 40]);
    expect(apportion([33, 33, 34], 7)).toEqual([2, 2, 3]);
    expect(apportion([50, 50], 1)).toEqual([1, 0]);
  });

  it("returns null when the group does not total 100", () => {
    expect(apportion([40, 40], 100)).toBeNull();
  });
});

describe("defaultQuotaGroups", () => {
  it("uses regions for one census country", () => {
    expect(defaultQuotaGroups([ca]).map((g) => g.key)).toEqual(["age", "region", "income"]);
  });

  it("adds a country group and drops regions for several countries", () => {
    const groups = defaultQuotaGroups([ca, us]);
    expect(groups.map((g) => g.key)).toEqual(["country", "age", "income"]);
    expect(groups[0].rows.reduce((a, r) => a + r.percent, 0)).toBe(100);
  });

  it("every default group totals 100", () => {
    for (const g of defaultQuotaGroups([ca])) {
      expect(g.rows.reduce((a, r) => a + r.percent, 0)).toBe(100);
    }
  });
});
