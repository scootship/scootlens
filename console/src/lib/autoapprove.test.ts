import { describe, expect, it } from "vitest";
import {
  AUTO_APPROVE_RULES,
  listAutoApprove,
  matchesAutoApprove,
  parseCapRequest,
  toggleAutoApprove,
} from "./autoapprove";
import type { ProfileStore } from "./profiles";

function memStore(initial?: Record<string, string>): ProfileStore {
  const m = new Map(Object.entries(initial ?? {}));
  return {
    getItem: (k) => m.get(k) ?? null,
    setItem: (k, v) => void m.set(k, v),
  };
}

describe("toggle/list roundtrip", () => {
  it("persists checked rule ids", () => {
    const store = memStore();
    expect(listAutoApprove(store).size).toBe(0);
    let set = toggleAutoApprove("js:exec", true, store);
    set = toggleAutoApprove("state:import", true, store);
    expect([...set].sort()).toEqual(["js:exec", "state:import"]);
    expect([...listAutoApprove(store)].sort()).toEqual(["js:exec", "state:import"]);
    set = toggleAutoApprove("js:exec", false, store);
    expect([...set]).toEqual(["state:import"]);
  });

  it("drops unknown ids and survives corrupt storage", () => {
    const store = memStore({ "scootlens.autoapprove": '["js:exec","bogus:rule"]' });
    expect([...listAutoApprove(store)]).toEqual(["js:exec"]);
    const bad = memStore({ "scootlens.autoapprove": "not json" });
    expect(listAutoApprove(bad).size).toBe(0);
  });
});

describe("matchesAutoApprove", () => {
  it("matches by segment prefix ignoring origin", () => {
    const enabled = new Set(["js:exec", "state:import"]);
    expect(matchesAutoApprove("js:exec@fixture.test", enabled)).toBe("js:exec");
    expect(matchesAutoApprove("js:exec", enabled)).toBe("js:exec");
    expect(matchesAutoApprove("state:import", enabled)).toBe("state:import");
    expect(matchesAutoApprove("state:export", enabled)).toBeNull();
    expect(matchesAutoApprove("act:upload@x.test", enabled)).toBeNull();
  });

  it("does not partial-match segment text", () => {
    // "state:import" 勾选不能命中 "state" 单段作用域（前缀比较按整段）
    expect(matchesAutoApprove("state", new Set(["state:import"]))).toBeNull();
  });

  it("rule table ids are unique and well-formed", () => {
    const ids = AUTO_APPROVE_RULES.map((r) => r.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of ids) expect(id).toMatch(/^[a-z]+(:[a-z]+)+$/);
  });
});

describe("parseCapRequest", () => {
  it("parses cap.request events", () => {
    const ev = parseCapRequest(
      JSON.stringify({
        topic: "cap.request",
        approval_id: "apr-3",
        method: "js.exec",
        scope: "js:exec@fixture.test",
        seq: 42,
      }),
    );
    expect(ev).toEqual({ approvalId: "apr-3", method: "js.exec", scope: "js:exec@fixture.test" });
  });

  it("returns null for other topics / malformed payloads", () => {
    expect(parseCapRequest(JSON.stringify({ topic: "nav", url: "x" }))).toBeNull();
    expect(parseCapRequest(JSON.stringify({ topic: "cap.request" }))).toBeNull();
    expect(parseCapRequest("not json")).toBeNull();
  });
});
