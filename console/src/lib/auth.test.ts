import { describe, expect, it } from "vitest";
import {
  fetchMe,
  fetchProviders,
  loginErrorMessage,
  loginPassword,
  logout,
  type FetchLike,
} from "./auth";

function fakeFetch(status: number, body?: unknown, fail = false): FetchLike {
  return async () => {
    if (fail) throw new Error("network down");
    return {
      ok: status >= 200 && status < 300,
      status,
      json: async () => body,
    } as Response;
  };
}

describe("fetchProviders", () => {
  it("parses enabled providers", async () => {
    const p = await fetchProviders(fakeFetch(200, { password: true, microsoft: false }));
    expect(p).toEqual({ password: true, microsoft: false });
  });

  it("treats old gateway (404) as all-disabled", async () => {
    expect(await fetchProviders(fakeFetch(404))).toEqual({ password: false, microsoft: false });
  });

  it("treats network failure as all-disabled", async () => {
    expect(await fetchProviders(fakeFetch(0, undefined, true))).toEqual({
      password: false,
      microsoft: false,
    });
  });

  it("ignores non-boolean fields", async () => {
    expect(await fetchProviders(fakeFetch(200, { password: "yes" }))).toEqual({
      password: false,
      microsoft: false,
    });
  });
});

describe("fetchMe", () => {
  it("returns subject when session exists", async () => {
    expect(await fetchMe(fakeFetch(200, { subject: "user:admin" }))).toBe("user:admin");
  });

  it("returns null on 401 / network failure", async () => {
    expect(await fetchMe(fakeFetch(401))).toBeNull();
    expect(await fetchMe(fakeFetch(0, undefined, true))).toBeNull();
  });
});

describe("loginPassword", () => {
  it("returns subject on success", async () => {
    await expect(
      loginPassword("admin", "pw", fakeFetch(200, { subject: "user:admin" })),
    ).resolves.toBe("user:admin");
  });

  it("maps 401 and 501 to readable errors", async () => {
    await expect(loginPassword("a", "b", fakeFetch(401))).rejects.toThrow("用户名或密码错误");
    await expect(loginPassword("a", "b", fakeFetch(501))).rejects.toThrow("未启用密码登录");
    await expect(loginPassword("a", "b", fakeFetch(500))).rejects.toThrow("HTTP 500");
  });
});

describe("logout", () => {
  it("swallows network failures", async () => {
    await expect(logout(fakeFetch(0, undefined, true))).resolves.toBeUndefined();
  });
});

describe("loginErrorMessage", () => {
  it("maps known codes", () => {
    expect(loginErrorMessage("?login_error=not_allowed")).toContain("允许名单");
    expect(loginErrorMessage("?login_error=state_mismatch")).toContain("重试");
  });

  it("falls back for unknown codes and returns null when absent", () => {
    expect(loginErrorMessage("?login_error=weird")).toContain("weird");
    expect(loginErrorMessage("?x=1")).toBeNull();
    expect(loginErrorMessage("")).toBeNull();
  });
});
