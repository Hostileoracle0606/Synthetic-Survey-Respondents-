import { describe, expect, it } from "vitest";
import { divergingColours } from "./charts";

describe("divergingColours", () => {
  it("has a grey midpoint for odd scales and none for even ones", () => {
    const five = divergingColours(5);
    expect(five).toHaveLength(5);
    expect(five[2]).toBe("#e4e2dc");
    const four = divergingColours(4);
    expect(four).toHaveLength(4);
    expect(four).not.toContain("#e4e2dc");
  });

  it("darkens toward both ends", () => {
    const seven = divergingColours(7);
    const lum = (h: string) => parseInt(h.slice(1, 3), 16) + parseInt(h.slice(3, 5), 16) + parseInt(h.slice(5, 7), 16);
    expect(lum(seven[0])).toBeLessThan(lum(seven[2]));
    expect(lum(seven[6])).toBeLessThan(lum(seven[4]));
    expect(divergingColours(2)).toEqual(["#c9302f", "#1c5cab"]);
  });
});
