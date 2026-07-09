import { describe, expect, it } from "vitest";
import type { ProcInfo } from "./api";
import { preferredPid, sortProcs, splitProcs, stateTone } from "./procs";

const proc = (pid: string, state: string): ProcInfo => ({ pid, state });

describe("sortProcs", () => {
  it("orders running > suspended > terminated, pid natural order", () => {
    const out = sortProcs([
      proc("p-10", "terminated"),
      proc("p-2", "running"),
      proc("p-3", "suspended"),
      proc("p-1", "terminated"),
      proc("p-11", "running"),
    ]);
    expect(out.map((p) => p.pid)).toEqual(["p-2", "p-11", "p-3", "p-1", "p-10"]);
  });

  it("does not mutate input", () => {
    const input = [proc("p-2", "terminated"), proc("p-1", "running")];
    sortProcs(input);
    expect(input[0].pid).toBe("p-2");
  });
});

describe("splitProcs / preferredPid", () => {
  it("splits active vs terminated", () => {
    const { active, terminated } = splitProcs([
      proc("p-1", "terminated"),
      proc("p-2", "running"),
      proc("p-3", "suspended"),
    ]);
    expect(active.map((p) => p.pid)).toEqual(["p-2", "p-3"]);
    expect(terminated.map((p) => p.pid)).toEqual(["p-1"]);
  });

  it("prefers first active pid, falls back to terminated, then empty", () => {
    expect(preferredPid([proc("p-1", "terminated"), proc("p-2", "running")])).toBe("p-2");
    expect(preferredPid([proc("p-1", "terminated")])).toBe("p-1");
    expect(preferredPid([])).toBe("");
  });
});

describe("stateTone", () => {
  it("maps states to tones", () => {
    expect(stateTone("running")).toBe("ok");
    expect(stateTone("suspended")).toBe("warn");
    expect(stateTone("terminated")).toBe("muted");
    expect(stateTone("odd")).toBe("info");
  });
});
