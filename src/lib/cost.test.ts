import { describe, expect, it } from "vitest";
import { tokens, usd } from "./cost";

describe("usd", () => {
  it("shows cents, thousands and tiny amounts", () => {
    expect(usd(0.4234)).toBe("$0.42");
    expect(usd(1234.5)).toBe("$1,234.50");
    expect(usd(0.004)).toBe("< $0.01");
    expect(usd(0)).toBe("$0.00");
  });
  it("shows a dash when there is no price", () => {
    expect(usd(null)).toBe("$—");
    expect(usd(undefined)).toBe("$—");
  });
});

describe("tokens", () => {
  it("abbreviates", () => {
    expect(tokens(1_234_567)).toBe("1.2M");
    expect(tokens(85_300)).toBe("85k");
    expect(tokens(640)).toBe("640");
  });
});
