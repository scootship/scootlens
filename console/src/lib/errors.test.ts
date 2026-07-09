import { describe, expect, it } from "vitest";
import { friendlyError } from "./errors";
import { RpcError } from "./rpc";

describe("friendlyError", () => {
  it("maps method-not-found to an upgrade hint when method given", () => {
    const e = new RpcError({ code: -32601, message: "method not found: act.point.click" });
    const msg = friendlyError(e, "act.point.click");
    expect(msg).toContain("act.point.click");
    expect(msg).toContain("升级 scootlensd");
  });

  it("detects legacy dispatcher message without -32601 code", () => {
    const e = new RpcError({ code: -32000, message: "method not found: act.point.click" });
    expect(friendlyError(e, "act.point.click")).toContain("升级 scootlensd");
  });

  it("keeps ABI code prefix for other rpc errors", () => {
    const e = new RpcError({
      code: -32000,
      message: "denied",
      data: { code: "E_CAP_DENIED" },
    });
    expect(friendlyError(e)).toBe("E_CAP_DENIED: denied");
  });

  it("passes through plain errors and strings", () => {
    expect(friendlyError(new Error("boom"))).toBe("boom");
    expect(friendlyError("boom")).toBe("boom");
  });
});
