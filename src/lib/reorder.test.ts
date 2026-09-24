import { describe, expect, it } from "vitest";
import { moveBefore, sameOrder } from "./reorder";

describe("moveBefore", () => {
  const ids = [1, 2, 3, 4];
  it("moves an item up to before another", () => {
    expect(moveBefore(ids, 4, 2)).toEqual([1, 4, 2, 3]);
  });
  it("moves an item down to before another", () => {
    expect(moveBefore(ids, 1, 4)).toEqual([2, 3, 1, 4]);
  });
  it("moves an item to the end", () => {
    expect(moveBefore(ids, 2, null)).toEqual([1, 3, 4, 2]);
  });
  it("leaves the order alone for a drop on itself or its own slot", () => {
    expect(moveBefore(ids, 3, 3)).toBe(ids);
    expect(sameOrder(moveBefore(ids, 2, 3), ids)).toBe(true);
    expect(sameOrder(moveBefore(ids, 4, null), ids)).toBe(true);
  });
  it("ignores unknown ids", () => {
    expect(moveBefore(ids, 9, 1)).toBe(ids);
  });
});
