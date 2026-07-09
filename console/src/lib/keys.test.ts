import { describe, expect, it } from "vitest";
import { pressKeyFor } from "./keys";

describe("pressKeyFor", () => {
  it("passes named control keys through", () => {
    for (const k of ["Enter", "Tab", "Escape", "Backspace", "Delete", "ArrowLeft", "PageDown"]) {
      expect(pressKeyFor({ key: k })).toBe(k);
    }
  });

  it("passes single printable characters (incl. space and CJK)", () => {
    expect(pressKeyFor({ key: "a" })).toBe("a");
    expect(pressKeyFor({ key: "Z" })).toBe("Z");
    expect(pressKeyFor({ key: "!" })).toBe("!");
    expect(pressKeyFor({ key: " " })).toBe(" ");
    expect(pressKeyFor({ key: "中" })).toBe("中");
  });

  it("ignores modifier combos (browser shortcuts stay local)", () => {
    expect(pressKeyFor({ key: "r", metaKey: true })).toBeNull();
    expect(pressKeyFor({ key: "c", ctrlKey: true })).toBeNull();
    expect(pressKeyFor({ key: "Tab", altKey: true })).toBeNull();
  });

  it("ignores bare modifiers and unsupported function keys", () => {
    expect(pressKeyFor({ key: "Shift" })).toBeNull();
    expect(pressKeyFor({ key: "CapsLock" })).toBeNull();
    expect(pressKeyFor({ key: "F5" })).toBeNull();
  });
});
