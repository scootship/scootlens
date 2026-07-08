import { describe, it, expect, vi } from "vitest";
import {
  RpcClient,
  RpcError,
  handshakeUrl,
  type SocketLike,
} from "./rpc";

class FakeSocket implements SocketLike {
  onopen: ((ev?: unknown) => unknown) | null = null;
  onclose: ((ev?: unknown) => unknown) | null = null;
  onerror: ((ev?: unknown) => unknown) | null = null;
  onmessage: ((ev: { data: unknown }) => unknown) | null = null;
  sent: string[] = [];
  closed = false;

  send(data: string): void {
    this.sent.push(data);
  }
  close(): void {
    this.closed = true;
    this.onclose?.();
  }
  // 测试触发器
  open(): void {
    this.onopen?.();
  }
  message(obj: unknown): void {
    this.onmessage?.({ data: JSON.stringify(obj) });
  }
  raw(data: unknown): void {
    this.onmessage?.({ data });
  }
  error(e: unknown): void {
    this.onerror?.(e);
  }
  lastReq(): { id: number; method: string; params?: unknown } {
    const s = this.sent.at(-1);
    if (!s) throw new Error("no sent frame");
    return JSON.parse(s);
  }
}

function connected(): { client: RpcClient; sock: FakeSocket } {
  const sock = new FakeSocket();
  const client = new RpcClient("http://x/", () => sock);
  const p = client.connect();
  sock.open();
  // connect promise resolves synchronously after open()
  void p;
  return { client, sock };
}

describe("handshakeUrl", () => {
  it("appends /ws and encodes token", () => {
    expect(handshakeUrl("ws://h:9", "slt1.a.b")).toBe("ws://h:9/ws?token=slt1.a.b");
  });
  it("does not double slash", () => {
    expect(handshakeUrl("ws://h/", "a b")).toBe("ws://h/ws?token=a%20b");
  });
});

describe("RpcClient lifecycle", () => {
  it("connect resolves on open and reports state", async () => {
    const sock = new FakeSocket();
    const client = new RpcClient("u", () => sock);
    const states: string[] = [];
    client.onState((s) => states.push(s));
    const p = client.connect();
    expect(client.state).toBe("connecting");
    sock.open();
    await p;
    expect(client.state).toBe("open");
    expect(states).toEqual(["connecting", "open"]);
  });

  it("connect is idempotent when already open", async () => {
    const { client } = connected();
    await client.connect();
    expect(client.state).toBe("open");
  });

  it("connect rejects on error while connecting", async () => {
    const sock = new FakeSocket();
    const client = new RpcClient("u", () => sock);
    const p = client.connect();
    sock.error(new Error("boom"));
    await expect(p).rejects.toBeInstanceOf(Error);
  });

  it("call sends a well-formed JSON-RPC request and resolves result", async () => {
    const { client, sock } = connected();
    const call = client.call("sys.info");
    const req = sock.lastReq();
    expect(req).toMatchObject({ jsonrpc: "2.0", method: "sys.info" });
    expect(req.params).toBeUndefined();
    sock.message({ jsonrpc: "2.0", id: req.id, result: { engine: "mock" } });
    await expect(call).resolves.toEqual({ engine: "mock" });
  });

  it("call forwards params", async () => {
    const { client, sock } = connected();
    const call = client.call("obs.journal", { limit: 5 });
    const req = sock.lastReq();
    expect(req.params).toEqual({ limit: 5 });
    sock.message({ jsonrpc: "2.0", id: req.id, result: { entries: [] } });
    await call;
  });

  it("call rejects with RpcError carrying abiCode", async () => {
    const { client, sock } = connected();
    const call = client.call("js.exec");
    const req = sock.lastReq();
    sock.message({
      jsonrpc: "2.0",
      id: req.id,
      error: { code: -32000, message: "denied", data: { code: "E_CAP_DENIED" } },
    });
    await expect(call).rejects.toMatchObject({
      name: "RpcError",
      code: -32000,
    });
    await call.catch((e: RpcError) => {
      expect(e.abiCode).toBe("E_CAP_DENIED");
    });
  });

  it("RpcError.abiCode is undefined without structured data", () => {
    const e = new RpcError({ code: 1, message: "x" });
    expect(e.abiCode).toBeUndefined();
    const e2 = new RpcError({ code: 1, message: "x", data: { code: 42 } });
    expect(e2.abiCode).toBeUndefined();
  });

  it("routes notifications and supports unsubscribe", () => {
    const { client, sock } = connected();
    const seen: Array<[string, unknown]> = [];
    const off = client.onEvent((m, p) => seen.push([m, p]));
    sock.message({ jsonrpc: "2.0", method: "evt.event", params: { topic: "cap.request" } });
    expect(seen).toEqual([["evt.event", { topic: "cap.request" }]]);
    off();
    sock.message({ jsonrpc: "2.0", method: "evt.event", params: { topic: "x" } });
    expect(seen).toHaveLength(1);
  });

  it("ignores malformed and unknown-id frames", async () => {
    const { client, sock } = connected();
    sock.raw("not json");
    sock.raw(123);
    sock.message({ jsonrpc: "2.0", id: 999, result: {} });
    // client still usable
    const call = client.call("sys.info");
    const req = sock.lastReq();
    sock.message({ jsonrpc: "2.0", id: req.id, result: { ok: true } });
    await expect(call).resolves.toEqual({ ok: true });
  });

  it("rejects call when not connected", async () => {
    const client = new RpcClient("u", () => new FakeSocket());
    await expect(client.call("sys.info")).rejects.toThrow(/not connected/);
  });

  it("close transitions to closed and rejects pending calls", async () => {
    const { client, sock } = connected();
    const call = client.call("proc.list");
    client.close();
    expect(client.state).toBe("closed");
    await expect(call).rejects.toBeInstanceOf(Error);
    expect(sock.closed).toBe(true);
  });

  it("rejects pending when socket closes underneath", async () => {
    const { client, sock } = connected();
    const call = client.call("proc.list");
    sock.onclose?.();
    await expect(call).rejects.toThrow(/closed/);
  });

  it("send failure rejects the call and clears pending", async () => {
    const sock = new FakeSocket();
    sock.send = vi.fn(() => {
      throw new Error("send fail");
    });
    const client = new RpcClient("u", () => sock);
    const p = client.connect();
    sock.open();
    await p;
    await expect(client.call("x")).rejects.toThrow(/send fail/);
  });
});
