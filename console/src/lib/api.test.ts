import { describe, it, expect } from "vitest";
import { ConsoleApi } from "./api";
import type { RpcClient } from "./rpc";

interface Recorded {
  method: string;
  params?: unknown;
}

function stub(results: Record<string, unknown>): { api: ConsoleApi; calls: Recorded[] } {
  const calls: Recorded[] = [];
  const client = {
    call: (method: string, params?: unknown) => {
      calls.push({ method, params });
      return Promise.resolve(results[method]);
    },
  } as unknown as RpcClient;
  return { api: new ConsoleApi(client), calls };
}

describe("ConsoleApi", () => {
  it("sysInfo delegates to sys.info", async () => {
    const { api, calls } = stub({ "sys.info": { engine: "mock", max_procs: 4 } });
    const info = await api.sysInfo();
    expect(info.engine).toBe("mock");
    expect(calls[0]).toEqual({ method: "sys.info", params: undefined });
  });

  it("procList extracts procs array", async () => {
    const { api } = stub({ "proc.list": { procs: [{ pid: "p-1", state: "running" }] } });
    const procs = await api.procList();
    expect(procs).toHaveLength(1);
    expect(procs[0].pid).toBe("p-1");
  });

  it("procList tolerates missing array", async () => {
    const { api } = stub({ "proc.list": {} });
    expect(await api.procList()).toEqual([]);
  });

  it("pending extracts pending array", async () => {
    const { api } = stub({ "cap.pending": { pending: [{ id: "apr-1" }] } });
    const p = await api.pending();
    expect(p[0].id).toBe("apr-1");
  });

  it("approve sends decision params with remember default", async () => {
    const { api, calls } = stub({ "cap.approve": {} });
    await api.approve("apr-9", "allow");
    expect(calls[0]).toEqual({
      method: "cap.approve",
      params: { approval_id: "apr-9", decision: "allow", remember: false },
    });
    await api.approve("apr-9", "deny", true);
    expect(calls[1].params).toMatchObject({ decision: "deny", remember: true });
  });

  it("journal parses entries and forwards limit/pid", async () => {
    const { api, calls } = stub({
      "obs.journal": { entries: [{ seq: 1, kind: "call", subject: "s", method: "m", hash: "h1" }] },
    });
    const rows = await api.journal(50, "p-1");
    expect(rows).toHaveLength(1);
    expect(calls[0].params).toEqual({ limit: 50, pid: "p-1" });
  });

  it("journal omits pid when absent", async () => {
    const { api, calls } = stub({ "obs.journal": { entries: [] } });
    await api.journal();
    expect(calls[0].params).toEqual({ limit: 100 });
  });

  // ---------- P4 ----------

  it("subscribe returns sub_id and forwards pid/topics", async () => {
    const { api, calls } = stub({ "evt.subscribe": { sub_id: "sub-1" } });
    expect(await api.subscribe()).toBe("sub-1");
    expect(calls[0].params).toEqual({ topics: [] });
    await api.subscribe("p-1", ["nav", "act.takeover"]);
    expect(calls[1].params).toEqual({ pid: "p-1", topics: ["nav", "act.takeover"] });
  });

  it("procSpawn / procKill / navGoto delegate", async () => {
    const { api, calls } = stub({
      "proc.spawn": { pid: "p-9" },
      "proc.kill": { ok: true },
      "nav.goto": { url: "http://a.test/" },
    });
    expect(await api.procSpawn()).toBe("p-9");
    expect(calls[0].params).toEqual({});
    await api.procSpawn("work");
    expect(calls[1].params).toEqual({ profile: "work" });
    await api.procKill("p-9");
    await api.navGoto("p-9", "http://a.test/");
    expect(calls[2]).toEqual({ method: "proc.kill", params: { pid: "p-9" } });
    expect(calls[3].params).toEqual({ pid: "p-9", url: "http://a.test/" });
  });

  it("screenshot builds a data url; snapshotText unwraps text", async () => {
    const { api } = stub({
      "view.screenshot": { format: "png", data_base64: "AA==" },
      "view.snapshot": { text: '- document "Home"\n' },
    });
    expect(await api.screenshot("p-1")).toBe("data:image/png;base64,AA==");
    expect(await api.snapshotText("p-1")).toContain("document");
  });

  it("act helpers forward pid/ref payloads", async () => {
    const { api, calls } = stub({ "act.click": {}, "act.type": {}, "act.press": {} });
    await api.actClick("p-1", "s1e2");
    await api.actType("p-1", "s1e2", "hi");
    await api.actTypeVault("p-1", "s1e3", "gh-password");
    await api.actPress("p-1", "Enter");
    expect(calls.map((c) => c.method)).toEqual(["act.click", "act.type", "act.type", "act.press"]);
    expect(calls[1].params).toEqual({ pid: "p-1", ref: "s1e2", text: "hi" });
    expect(calls[2].params).toEqual({ pid: "p-1", ref: "s1e3", vault_ref: "gh-password" });
    expect(calls[3].params).toEqual({ pid: "p-1", keys: "Enter" });
  });

  it("actClickAt forwards normalized ratio payload", async () => {
    const { api, calls } = stub({ "act.point.click": {} });
    await api.actClickAt("p-1", 0.25, 0.75);
    expect(calls[0]).toEqual({
      method: "act.point.click",
      params: { pid: "p-1", x_ratio: 0.25, y_ratio: 0.75 },
    });
  });

  it("takeover start/end delegate to act.takeover.*", async () => {
    const { api, calls } = stub({
      "act.takeover.start": { ok: true, holder: "user:admin" },
      "act.takeover.end": { ok: true },
    });
    await api.takeoverStart("p-1");
    await api.takeoverEnd("p-1");
    expect(calls.map((c) => c.method)).toEqual(["act.takeover.start", "act.takeover.end"]);
    expect(calls[0].params).toEqual({ pid: "p-1" });
  });

  it("netLog extracts entries; replayExport unwraps bundle", async () => {
    const { api, calls } = stub({
      "net.log": { entries: [{ url: "http://a.test/", allowed: true }] },
      "obs.replay.export": { bundle: { format_version: 1, pid: "p-1", journal: [] } },
    });
    const log = await api.netLog("p-1", 10);
    expect(log).toHaveLength(1);
    expect(calls[0].params).toEqual({ pid: "p-1", limit: 10 });
    const bundle = await api.replayExport("p-1", 64);
    expect(bundle.pid).toBe("p-1");
    expect(calls[1].params).toEqual({ pid: "p-1", journal_limit: 64 });
  });

  it("cap/vault/net settings calls carry exact params", async () => {
    const { api, calls } = stub({
      "cap.list": { subject: "user:admin", scopes: ["*"] },
      "cap.grant": {},
      "cap.revoke": {},
      "state.write": { ok: true },
      "state.list": { names: ["gh-password", "gh-user"] },
      "net.rules.get": { rules: { default: "allow", rules: [] } },
      "net.rules.set": { ok: true },
    });
    expect((await api.capList()).subject).toBe("user:admin");
    await api.capGrant("agent:a", "nav@a.test");
    await api.capRevoke("agent:a", "nav@a.test");
    await api.vaultWrite("gh-password", "s3cret");
    expect(await api.vaultList()).toEqual(["gh-password", "gh-user"]);
    await api.netRulesGet();
    await api.netRulesSet({ default: "deny", rules: [] });
    expect(calls[1].params).toEqual({ subject: "agent:a", scope: "nav@a.test" });
    expect(calls[3].params).toEqual({
      namespace: "vault",
      key: "gh-password",
      value: "s3cret",
    });
    expect(calls[4].params).toEqual({ namespace: "vault" });
    expect(calls[5].params).toEqual({});
    expect(calls[6].params).toEqual({ default: "deny", rules: [] });
    await api.netRulesGet("p-2");
    expect(calls[7].params).toEqual({ pid: "p-2" });
  });

  it("profile list/digest/delete carry exact params (values never requested back)", async () => {
    const { api, calls } = stub({
      "state.list": { names: ["shop", "mail"] },
      "state.read": {
        profile: "shop",
        entries: [
          {
            key: "cookie:sessionid",
            kind: "cookie",
            value_bytes: 24,
            domain: ".shop.test",
            httpOnly: true,
            secure: true,
          },
        ],
      },
      "state.delete": { ok: true, deleted: 1 },
    });
    expect(await api.profileList()).toEqual(["shop", "mail"]);
    expect(calls[0]).toEqual({ method: "state.list", params: { namespace: "profiles" } });

    const digest = await api.profileDigest("shop");
    expect(digest).toHaveLength(1);
    expect(digest[0].key).toBe("cookie:sessionid");
    expect(digest[0].value_bytes).toBe(24);
    expect(calls[1]).toEqual({
      method: "state.read",
      params: { namespace: "profiles", key: "shop" },
    });

    await api.profileDelete("shop");
    expect(calls[2]).toEqual({
      method: "state.delete",
      params: { namespace: "profiles", key: "shop" },
    });
    await api.profileDelete("shop", "cookie:sessionid");
    expect(calls[3]).toEqual({
      method: "state.delete",
      params: { namespace: "profiles", key: "shop", entry: "cookie:sessionid" },
    });
  });

  it("profileList/profileDigest tolerate missing arrays", async () => {
    const { api } = stub({ "state.list": {}, "state.read": {} });
    expect(await api.profileList()).toEqual([]);
    expect(await api.profileDigest("ghost")).toEqual([]);
  });

  it("vaultDelete targets the vault namespace by name", async () => {
    const { api, calls } = stub({ "state.delete": { ok: true, deleted: 1 } });
    await api.vaultDelete("gh-password");
    expect(calls[0]).toEqual({
      method: "state.delete",
      params: { namespace: "vault", key: "gh-password" },
    });
  });
});
