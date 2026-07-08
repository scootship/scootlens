import { describe, it, expect } from "vitest";
import { listProfiles, rememberProfile, forgetProfile, type ProfileStore } from "./profiles";

function fakeStore(): ProfileStore & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return {
    map,
    getItem: (k) => (map.has(k) ? (map.get(k) as string) : null),
    setItem: (k, v) => void map.set(k, v),
  };
}

describe("profiles registry", () => {
  it("starts empty", () => {
    expect(listProfiles(fakeStore())).toEqual([]);
  });

  it("remembers and lists a profile", () => {
    const s = fakeStore();
    rememberProfile("github", s);
    expect(listProfiles(s)).toEqual(["github"]);
  });

  it("dedupes and keeps names sorted", () => {
    const s = fakeStore();
    rememberProfile("gitlab", s);
    rememberProfile("github", s);
    rememberProfile("github", s);
    expect(listProfiles(s)).toEqual(["github", "gitlab"]);
  });

  it("trims whitespace and ignores blank names", () => {
    const s = fakeStore();
    rememberProfile("  reuse  ", s);
    rememberProfile("   ", s);
    expect(listProfiles(s)).toEqual(["reuse"]);
  });

  it("forgets a profile", () => {
    const s = fakeStore();
    rememberProfile("a", s);
    rememberProfile("b", s);
    expect(forgetProfile("a", s)).toEqual(["b"]);
    expect(listProfiles(s)).toEqual(["b"]);
  });

  it("survives corrupt storage payloads", () => {
    const s = fakeStore();
    s.map.set("scootlens.profiles", "{not-json");
    expect(listProfiles(s)).toEqual([]);
    expect(rememberProfile("fresh", s)).toEqual(["fresh"]);
  });

  it("ignores non-array / non-string entries", () => {
    const s = fakeStore();
    s.map.set("scootlens.profiles", JSON.stringify({ a: 1 }));
    expect(listProfiles(s)).toEqual([]);
    s.map.set("scootlens.profiles", JSON.stringify(["ok", 42, "", null]));
    expect(listProfiles(s)).toEqual(["ok"]);
  });

  it("returns empty when no store is available", () => {
    expect(listProfiles()).toEqual([]);
  });
});
