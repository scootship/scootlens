import { describe, it, expect } from "vitest";
import {
  parseSnapshotText,
  interactive,
  acceptsText,
  screencastInterval,
  takeoverView,
} from "./session";

const SNAP = [
  '- document "Login"',
  '  - heading "Login"',
  '  - textbox "Username" [s2e1]',
  '  - textbox "Password" = "secret" [s2e2]',
  '  - button "Sign in" [s2e3]',
  "… (truncated)",
].join("\n");

describe("parseSnapshotText", () => {
  it("parses roles, names, values, refs and depth", () => {
    const els = parseSnapshotText(SNAP);
    expect(els).toHaveLength(5);
    expect(els[0]).toEqual({ depth: 0, role: "document", name: "Login" });
    expect(els[2]).toEqual({ depth: 1, role: "textbox", name: "Username", ref: "s2e1" });
    expect(els[3].value).toBe("secret");
    expect(els[4].ref).toBe("s2e3");
  });

  it("skips malformed lines and empty input", () => {
    expect(parseSnapshotText("")).toEqual([]);
    expect(parseSnapshotText("garbage\nmore garbage")).toEqual([]);
  });

  it("interactive keeps only ref-bearing elements", () => {
    const refs = interactive(parseSnapshotText(SNAP));
    expect(refs.map((e) => e.ref)).toEqual(["s2e1", "s2e2", "s2e3"]);
  });
});

describe("acceptsText", () => {
  it("recognizes text-input roles", () => {
    expect(acceptsText("textbox")).toBe(true);
    expect(acceptsText("SearchBox")).toBe(true);
    expect(acceptsText("button")).toBe(false);
    expect(acceptsText("link")).toBe(false);
  });
});

describe("screencastInterval", () => {
  it("polls running procs and stops otherwise", () => {
    expect(screencastInterval("running")).toBeGreaterThan(0);
    expect(screencastInterval("Running")).toBeGreaterThan(0);
    expect(screencastInterval("suspended")).toBe(0);
    expect(screencastInterval("terminated")).toBe(0);
    expect(screencastInterval(null)).toBe(0);
    expect(screencastInterval(undefined)).toBe(0);
  });
});

describe("takeoverView", () => {
  it("maps holder to view state", () => {
    expect(takeoverView(null, "user:admin")).toEqual({ kind: "idle" });
    expect(takeoverView("user:admin", "user:admin")).toEqual({ kind: "held-by-me" });
    expect(takeoverView("user:alice", "user:admin")).toEqual({
      kind: "held-by-other",
      holder: "user:alice",
    });
  });
});
