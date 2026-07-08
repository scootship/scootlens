import { describe, it, expect } from "vitest";
import { buildStateBundle } from "./cookies";

// Cookie-Editor「Export」的典型输出：对象数组，含 httpOnly。
const cookieEditorJson = JSON.stringify([
  {
    name: "session",
    value: "SECRET-httponly",
    domain: ".github.com",
    path: "/",
    secure: true,
    httpOnly: true,
  },
  {
    name: "pref",
    value: "dark",
    domain: "github.com",
    path: "/",
    secure: false,
    httpOnly: false,
  },
]);

describe("buildStateBundle", () => {
  it("maps a Cookie-Editor array into cookie:* entries incl. httpOnly", () => {
    const r = buildStateBundle(cookieEditorJson);
    expect(r.cookies).toBe(2);
    expect(r.httpOnly).toBe(1);
    expect(r.storage).toBe(0);
    expect(r.bundle.entries["cookie:session"]).toEqual({
      value: "SECRET-httponly",
      domain: ".github.com",
      path: "/",
      secure: true,
      httpOnly: true,
    });
  });

  it("defaults path to / and treats missing flags as false", () => {
    const r = buildStateBundle(JSON.stringify([{ name: "a", value: "1", domain: "x.test" }]));
    const e = r.bundle.entries["cookie:a"] as Record<string, unknown>;
    expect(e.path).toBe("/");
    expect(e.secure).toBe(false);
    expect(e.httpOnly).toBe(false);
  });

  it("accepts http_only snake_case variant", () => {
    const r = buildStateBundle(
      JSON.stringify([{ name: "s", value: "1", domain: "x.test", http_only: true }]),
    );
    expect(r.httpOnly).toBe(1);
    expect((r.bundle.entries["cookie:s"] as Record<string, unknown>).httpOnly).toBe(true);
  });

  it("unwraps a { cookies: [...] } wrapper", () => {
    const r = buildStateBundle(JSON.stringify({ cookies: JSON.parse(cookieEditorJson) }));
    expect(r.cookies).toBe(2);
  });

  it("skips entries without a name", () => {
    const r = buildStateBundle(JSON.stringify([{ value: "x" }, { name: "ok", value: "1" }]));
    expect(r.cookies).toBe(1);
    expect(r.bundle.entries["cookie:ok"]).toBeDefined();
  });

  it("adds localStorage from an object", () => {
    const r = buildStateBundle(cookieEditorJson, JSON.stringify({ auth: "jwt", n: 5 }));
    expect(r.storage).toBe(2);
    expect(r.bundle.entries["storage:auth"]).toBe("jwt");
    // 非字符串值被序列化保存
    expect(r.bundle.entries["storage:n"]).toBe("5");
  });

  it("adds localStorage from an Object.entries array", () => {
    const r = buildStateBundle(cookieEditorJson, JSON.stringify([["k", "v"]]));
    expect(r.storage).toBe(1);
    expect(r.bundle.entries["storage:k"]).toBe("v");
  });

  it("throws on invalid cookie JSON", () => {
    expect(() => buildStateBundle("{not json")).toThrow(/解析失败/);
  });

  it("throws when no cookies parsed", () => {
    expect(() => buildStateBundle("[]")).toThrow(/没有解析到/);
  });

  it("throws when cookie JSON is not an array/wrapper", () => {
    expect(() => buildStateBundle('"just a string"')).toThrow(/应是一个数组/);
  });

  it("throws on invalid localStorage JSON", () => {
    expect(() => buildStateBundle(cookieEditorJson, "{bad")).toThrow(/localStorage JSON/);
  });
});
