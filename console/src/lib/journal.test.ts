import { describe, it, expect } from "vitest";
import { parseEntries, checkIntegrity, type JournalEntry } from "./journal";

function entry(seq: number, hash: string | undefined = `h${seq}`): JournalEntry {
  return { seq, ts_ms: seq * 1000, kind: "call", subject: "s", method: "m", hash };
}

describe("parseEntries", () => {
  it("parses well-formed entries", () => {
    const rows = parseEntries([
      { seq: 2, ts_ms: 2, kind: "result", subject: "a", method: "sys.info", hash: "h2", pid: "p-1" },
      { seq: 1, ts_ms: 1, kind: "call", subject: "a", method: "sys.info", hash: "h1" },
    ]);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({ seq: 2, kind: "result", pid: "p-1" });
    expect(rows[1].pid).toBeNull();
  });
  it("skips malformed items and non-arrays", () => {
    expect(parseEntries(null)).toEqual([]);
    expect(parseEntries("nope")).toEqual([]);
    expect(parseEntries([{ nope: 1 }, { seq: "x" }, 5, null])).toEqual([]);
  });
  it("defaults missing optional fields", () => {
    const [row] = parseEntries([{ seq: 1 }]);
    expect(row).toMatchObject({ seq: 1, ts_ms: 0, kind: "", method: "", hash: undefined });
  });
});

describe("checkIntegrity", () => {
  it("passes a contiguous newest-first window", () => {
    const r = checkIntegrity([entry(5), entry(4), entry(3)]);
    expect(r.ok).toBe(true);
    expect(r.count).toBe(3);
    expect(r.gaps).toEqual([]);
    expect(r.missingHash).toEqual([]);
  });
  it("detects a seq gap", () => {
    const r = checkIntegrity([entry(5), entry(3)]);
    expect(r.ok).toBe(false);
    expect(r.gaps).toContain(4);
  });
  it("flags reordering / duplicates", () => {
    const r = checkIntegrity([entry(3), entry(3)]);
    expect(r.ok).toBe(false);
  });
  it("reports missing hashes", () => {
    const r = checkIntegrity([
      { seq: 2, ts_ms: 2000, kind: "call", subject: "s", method: "m" },
      entry(1),
    ]);
    expect(r.ok).toBe(false);
    expect(r.missingHash).toEqual([2]);
  });
  it("handles empty window", () => {
    expect(checkIntegrity([])).toMatchObject({ ok: true, count: 0 });
  });
});
