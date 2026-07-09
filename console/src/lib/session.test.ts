import { describe, it, expect } from "vitest";
import {
  parseSnapshotText,
  interactive,
  acceptsText,
  screencastInterval,
  takeoverView,
  containRect,
  clickRatio,
  pickLoginFields,
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

describe("pickLoginFields", () => {
  it("picks username and password fields from common labels", () => {
    const els = interactive(parseSnapshotText(SNAP));
    const picked = pickLoginFields(els);
    expect(picked.username?.name).toBe("Username");
    expect(picked.password?.name).toBe("Password");
  });

  it("uses nearby preceding text input as username fallback", () => {
    const els = interactive(
      parseSnapshotText(
        [
          '- document "Login"',
          '  - textbox "Field A" [s2e1]',
          '  - textbox "Field B" [s2e2]',
          '  - textbox "Password" [s2e3]',
        ].join("\n"),
      ),
    );
    const picked = pickLoginFields(els);
    expect(picked.username?.ref).toBe("s2e2");
    expect(picked.password?.ref).toBe("s2e3");
  });

  it("returns empty fields when no password-like input exists", () => {
    const els = interactive(parseSnapshotText('- textbox "Search" [s2e1]'));
    const picked = pickLoginFields(els);
    expect(picked.password).toBeUndefined();
    expect(picked.username).toBeUndefined();
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

describe("containRect", () => {
  it("fills the box exactly when aspect ratios match", () => {
    expect(containRect(1280, 800, 640, 400)).toEqual({ x: 0, y: 0, width: 640, height: 400 });
  });

  it("letterboxes top/bottom when the box is relatively wider", () => {
    // 1280x800 图片塞进 1000x1000 方盒：按宽缩放到 1000x625，上下各留白 187.5
    expect(containRect(1280, 800, 1000, 1000)).toEqual({
      x: 0,
      y: 187.5,
      width: 1000,
      height: 625,
    });
  });

  it("letterboxes left/right when the box is relatively wider than the image (taller aspect)", () => {
    // 1280x800（宽高比 1.6）塞进 400x200（宽高比 2）：按高缩放，宽度留白
    // scale = min(400/1280, 200/800) = 0.25 → 320x200，左右各留白 40
    expect(containRect(1280, 800, 400, 200)).toEqual({ x: 40, y: 0, width: 320, height: 200 });
  });

  it("rejects non-positive dimensions", () => {
    expect(containRect(0, 800, 100, 100)).toBeNull();
    expect(containRect(1280, 0, 100, 100)).toBeNull();
    expect(containRect(1280, 800, 0, 100)).toBeNull();
    expect(containRect(1280, 800, 100, -1)).toBeNull();
  });
});

describe("clickRatio", () => {
  const rect = { x: 0, y: 0, width: 128, height: 80 };

  it("normalizes offset against the content rect", () => {
    expect(clickRatio(64, 40, rect)).toEqual({ xRatio: 0.5, yRatio: 0.5 });
    expect(clickRatio(0, 0, rect)).toEqual({ xRatio: 0, yRatio: 0 });
    expect(clickRatio(128, 80, rect)).toEqual({ xRatio: 1, yRatio: 1 });
  });

  it("subtracts the letterbox offset before normalizing", () => {
    // 内容矩形从 (0,20) 开始、100x40：偏移 50,40 落在内容矩形中点
    const letterboxed = { x: 0, y: 20, width: 100, height: 40 };
    expect(clickRatio(50, 40, letterboxed)).toEqual({ xRatio: 0.5, yRatio: 0.5 });
  });

  it("clamps offsets outside the content rect to the nearest edge", () => {
    expect(clickRatio(-10, 200, rect)).toEqual({ xRatio: 0, yRatio: 1 });
  });

  it("rejects a non-positive content rect", () => {
    expect(clickRatio(10, 10, { x: 0, y: 0, width: 0, height: 80 })).toBeNull();
    expect(clickRatio(10, 10, { x: 0, y: 0, width: 128, height: 0 })).toBeNull();
  });
});
