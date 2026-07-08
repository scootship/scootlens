import { describe, it, expect } from "vitest";
import {
  parseBundle,
  verifyChain,
  timeline,
  frameAt,
  frameUrl,
  type ReplayLine,
} from "./replay";

const GENESIS = "0".repeat(64);

async function sha256(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/** 构造与内核同构的合法链段。 */
async function chain(entries: Array<Record<string, unknown>>): Promise<ReplayLine[]> {
  const out: ReplayLine[] = [];
  let prev = GENESIS;
  for (const [i, e] of entries.entries()) {
    const raw = JSON.stringify({ seq: i + 1, ts_ms: 1000 + i, kind: "call", ...e });
    const hash = await sha256(prev + raw);
    out.push({ seq: i + 1, prev, hash, raw });
    prev = hash;
  }
  return out;
}

function bundleOf(journal: ReplayLine[], frames: unknown[] = []) {
  return {
    format_version: 1,
    pid: "p-a1",
    engine: "mock",
    exported_at_ms: 5000,
    journal,
    frames,
  };
}

describe("parseBundle", () => {
  it("parses a wire bundle from string or object", async () => {
    const lines = await chain([{ subject: "s", method: "nav.goto", pid: "p-a1" }]);
    const b = parseBundle(JSON.stringify(bundleOf(lines, [
      { ts_ms: 1, format: "png", data_base64: "AA==" },
    ])));
    expect(b.pid).toBe("p-a1");
    expect(b.journal).toHaveLength(1);
    expect(b.frames).toHaveLength(1);
  });

  it("rejects wrong version and malformed shells", () => {
    expect(() => parseBundle({ format_version: 2, pid: "p-1", journal: [] })).toThrow(
      /format_version/,
    );
    expect(() => parseBundle({ format_version: 1 })).toThrow(/pid|journal/);
    expect(() => parseBundle("not json")).toThrow();
  });

  it("skips malformed journal lines and frames", () => {
    const b = parseBundle(
      bundleOf([{ seq: 1, prev: GENESIS, hash: "h", raw: "{}" }, { bogus: true } as never], [
        { nope: 1 },
      ]),
    );
    expect(b.journal).toHaveLength(1);
    expect(b.frames).toHaveLength(0);
  });
});

describe("verifyChain", () => {
  it("accepts a well-formed chain", async () => {
    const lines = await chain([
      { subject: "a", method: "proc.spawn" },
      { subject: "a", method: "nav.goto", pid: "p-a1" },
      { subject: "b", method: "js.exec", pid: "p-a1" },
    ]);
    const report = await verifyChain(lines);
    expect(report).toEqual({ ok: true, checked: 3, brokenAt: null, reason: null });
  });

  it("detects a modified line (hash mismatch)", async () => {
    const lines = await chain([{ method: "a" }, { method: "b" }]);
    lines[1] = { ...lines[1], raw: lines[1].raw.replace('"b"', '"evil"') };
    const report = await verifyChain(lines);
    expect(report.ok).toBe(false);
    expect(report.brokenAt).toBe(2);
    expect(report.reason).toMatch(/hash/);
  });

  it("detects a broken link (prev mismatch)", async () => {
    const a = await chain([{ method: "a" }]);
    const other = await chain([{ method: "x" }, { method: "y" }]);
    const report = await verifyChain([a[0], other[1]]);
    expect(report.ok).toBe(false);
    expect(report.reason).toMatch(/链断裂/);
  });

  it("accepts an empty segment", async () => {
    expect((await verifyChain([])).ok).toBe(true);
  });
});

describe("timeline / frames", () => {
  it("parses entries and marks pid ownership", async () => {
    const lines = await chain([
      { subject: "a", method: "proc.spawn" },
      { subject: "a", method: "nav.goto", pid: "p-a1" },
      { subject: "a", method: "nav.goto", pid: "p-zz" },
    ]);
    const items = timeline(parseBundle(bundleOf(lines)));
    expect(items).toHaveLength(3);
    expect(items.map((i) => i.ofPid)).toEqual([false, true, false]);
  });

  it("frameAt picks latest frame at or before ts", () => {
    const frames = [
      { ts_ms: 100, format: "png", data_base64: "a" },
      { ts_ms: 200, format: "png", data_base64: "b" },
      { ts_ms: 300, format: "png", data_base64: "c" },
    ];
    expect(frameAt(frames, 50)).toBeNull();
    expect(frameAt(frames, 200)?.data_base64).toBe("b");
    expect(frameAt(frames, 999)?.data_base64).toBe("c");
    expect(frameAt([], 100)).toBeNull();
  });

  it("frameUrl builds a data url", () => {
    expect(frameUrl({ ts_ms: 1, format: "png", data_base64: "AA==" })).toBe(
      "data:image/png;base64,AA==",
    );
  });
});
