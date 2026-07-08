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
});
