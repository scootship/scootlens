import { describe, it, expect } from "vitest";
import {
  formatTs,
  scopeLabel,
  kindLabel,
  kindTone,
  shortId,
  abiCodeLabel,
} from "./format";

describe("formatTs", () => {
  it("formats unix ms as UTC date-time", () => {
    expect(formatTs(0)).toBe("1970-01-01 00:00:00");
    expect(formatTs(Date.UTC(2025, 1, 14, 8, 30, 5))).toBe("2025-02-14 08:30:05");
  });
  it("falls back for invalid input", () => {
    expect(formatTs(undefined)).toBe("—");
    expect(formatTs("x")).toBe("—");
    expect(formatTs(-1)).toBe("—");
    expect(formatTs(NaN)).toBe("—");
  });
});

describe("scopeLabel", () => {
  it("passes through strings", () => {
    expect(scopeLabel("js:exec@app.test")).toBe("js:exec@app.test");
  });
  it("renders segments+origin objects", () => {
    expect(scopeLabel({ segments: ["state", "write", "vault"], origin: "app.test" })).toBe(
      "state:write:vault@app.test",
    );
  });
  it("renders domain/action objects", () => {
    expect(scopeLabel({ domain: "nav", action: "goto" })).toBe("nav:goto");
  });
  it("handles empty and null", () => {
    expect(scopeLabel(null)).toBe("");
    expect(scopeLabel({})).toBe("{}");
  });
});

describe("kindLabel / kindTone", () => {
  it("labels known kinds", () => {
    expect(kindLabel("call")).toContain("调用");
    expect(kindLabel("result")).toContain("成功");
    expect(kindLabel("deny")).toContain("拒绝");
    expect(kindLabel("approval")).toContain("审批");
  });
  it("echoes unknown kinds", () => {
    expect(kindLabel("weird")).toBe("weird");
  });
  it("maps tones", () => {
    expect(kindTone("call")).toBe("info");
    expect(kindTone("result")).toBe("ok");
    expect(kindTone("deny")).toBe("danger");
    expect(kindTone("approval")).toBe("warn");
    expect(kindTone("other")).toBe("muted");
  });
});

describe("shortId", () => {
  it("truncates long ids", () => {
    expect(shortId("abcdefghijklmnop", 6)).toBe("abcdef…");
  });
  it("keeps short ids", () => {
    expect(shortId("abc", 6)).toBe("abc");
    expect(shortId(undefined)).toBe("");
  });
});

describe("abiCodeLabel", () => {
  it("humanizes known codes", () => {
    expect(abiCodeLabel("E_CAP_DENIED")).toContain("能力");
    expect(abiCodeLabel("E_QUOTA")).toContain("限速");
  });
  it("echoes unknown", () => {
    expect(abiCodeLabel("E_MYSTERY")).toBe("E_MYSTERY");
  });
});
