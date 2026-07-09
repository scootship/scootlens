import { describe, it, expect } from "vitest";
import {
  forgetCredential,
  listCredentials,
  matchingCredentials,
  normalizeOriginPattern,
  originFromUrl,
  originMatches,
  saveCredential,
  type CredentialStore,
} from "./credentials";

function fakeStore(): CredentialStore & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return {
    map,
    getItem: (k) => (map.has(k) ? (map.get(k) as string) : null),
    setItem: (k, v) => void map.set(k, v),
  };
}

describe("credential registry", () => {
  it("normalizes origin patterns", () => {
    expect(normalizeOriginPattern("https://Github.com/login")).toBe("github.com");
    expect(normalizeOriginPattern(" *.corp.test ")).toBe("*.corp.test");
    expect(normalizeOriginPattern("@localhost:5173")).toBe("localhost:5173");
  });

  it("rejects broad or malformed origin patterns", () => {
    expect(() => normalizeOriginPattern("*")).toThrow("不允许");
    expect(() => normalizeOriginPattern("foo/bar")).toThrow("只能是");
    expect(() => normalizeOriginPattern("foo.*.test")).toThrow("只支持");
  });

  it("saves, lists, and forgets credential bindings without secrets", () => {
    const s = fakeStore();
    const saved = saveCredential(
      {
        label: "GitHub",
        origin: "github.com",
        usernameRef: "gh-user",
        passwordRef: "gh-pass",
        loginUrl: "https://github.com/login",
      },
      s,
    );
    expect(saved).toHaveLength(1);
    expect(saved[0]).toMatchObject({
      label: "GitHub",
      origin: "github.com",
      usernameRef: "gh-user",
      passwordRef: "gh-pass",
    });
    expect(JSON.stringify(saved)).not.toContain("s3cret");
    expect(listCredentials(s)).toEqual(saved);
    expect(forgetCredential(saved[0].id, s)).toEqual([]);
  });

  it("validates required refs and login URL", () => {
    const s = fakeStore();
    expect(() =>
      saveCredential({ origin: "a.test", usernameRef: "", passwordRef: "pw" }, s),
    ).toThrow("用户名");
    expect(() =>
      saveCredential({ origin: "a.test", usernameRef: "u", passwordRef: "pw", loginUrl: "bad" }, s),
    ).toThrow("登录页");
  });

  it("matches exact and wildcard origins", () => {
    expect(originFromUrl("http://app.test:8080/login")).toBe("app.test:8080");
    expect(originMatches("app.test:8080", "http://app.test:8080/login")).toBe(true);
    expect(originMatches("app.test", "http://sub.app.test/login")).toBe(false);
    expect(originMatches("*.app.test", "http://sub.app.test/login")).toBe(true);
    expect(originMatches("*.app.test", "http://app.test/login")).toBe(false);
  });

  it("filters matching credentials", () => {
    const s = fakeStore();
    const all = saveCredential({ origin: "a.test", usernameRef: "u", passwordRef: "p" }, s);
    const withSecond = saveCredential({ origin: "*.b.test", usernameRef: "u2", passwordRef: "p2" }, s);
    expect(withSecond).toHaveLength(2);
    expect(matchingCredentials("http://a.test/login", listCredentials(s)).map((c) => c.origin)).toEqual([
      "a.test",
    ]);
    expect(matchingCredentials("http://x.b.test/login", listCredentials(s)).map((c) => c.origin)).toEqual([
      "*.b.test",
    ]);
    expect(all[0].origin).toBe("a.test");
  });

  it("survives corrupt storage payloads", () => {
    const s = fakeStore();
    s.map.set("scootlens.credentials", "{not-json");
    expect(listCredentials(s)).toEqual([]);
    s.map.set("scootlens.credentials", JSON.stringify([{ origin: "*", usernameRef: "u", passwordRef: "p" }]));
    expect(listCredentials(s)).toEqual([]);
  });
});
