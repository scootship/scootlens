import { describe, it, expect } from "vitest";
import { parseConnectParams, defaultBase } from "./connect";

describe("parseConnectParams", () => {
  it("extracts token and autoconnect flag", () => {
    expect(parseConnectParams("?token=slt1.a.b&connect=1")).toEqual({
      token: "slt1.a.b",
      auto: true,
      base: null,
    });
  });

  it("no autoconnect without token", () => {
    expect(parseConnectParams("?connect=1")).toEqual({ token: null, auto: false, base: null });
  });

  it("token without connect flag stays manual", () => {
    const p = parseConnectParams("token=slt1.x.y");
    expect(p.token).toBe("slt1.x.y");
    expect(p.auto).toBe(false);
  });

  it("blank token treated as absent; base override parsed", () => {
    expect(parseConnectParams("?token=%20%20").token).toBeNull();
    expect(parseConnectParams("?base=ws://10.0.0.2:9910").base).toBe("ws://10.0.0.2:9910");
    expect(parseConnectParams("")).toEqual({ token: null, auto: false, base: null });
  });
});

describe("defaultBase", () => {
  it("maps page protocol to ws/wss", () => {
    expect(defaultBase("http:", "localhost:9910")).toBe("ws://localhost:9910");
    expect(defaultBase("https:", "lens.example")).toBe("wss://lens.example");
    expect(defaultBase("http:", "")).toBe("ws://127.0.0.1:9910");
  });
});
