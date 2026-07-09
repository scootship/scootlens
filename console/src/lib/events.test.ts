import { describe, expect, it } from "vitest";
import { summarizeEvent, topicTone } from "./events";

describe("topicTone", () => {
  it("maps topic families", () => {
    expect(topicTone("proc.lifecycle")).toBe("ok");
    expect(topicTone("act.takeover")).toBe("warn");
    expect(topicTone("net.request")).toBe("danger");
    expect(topicTone("cap.request")).toBe("danger");
    expect(topicTone("nav.loaded")).toBe("info");
    expect(topicTone("something.else")).toBe("muted");
  });
});

describe("summarizeEvent", () => {
  it("extracts pid and compacts remaining fields", () => {
    const { pid, fields } = summarizeEvent(
      JSON.stringify({ pid: "p-1", seq: 90, topic: "proc.lifecycle", state: "running" }),
    );
    expect(pid).toBe("p-1");
    expect(fields).toEqual(["state=running"]);
  });

  it("truncates long values and limits field count", () => {
    const { fields } = summarizeEvent(
      JSON.stringify({
        a: "x".repeat(100),
        b: 1,
        c: true,
        d: { nested: 1 },
        e: "over-limit",
      }),
    );
    expect(fields).toHaveLength(4);
    expect(fields[0].length).toBeLessThan(60);
    expect(fields[0].endsWith("…")).toBe(true);
    expect(fields[1]).toBe("b=1");
    expect(fields[3]).toBe('d={"nested":1}');
  });

  it("handles non-JSON and non-object payloads", () => {
    expect(summarizeEvent("plain text").fields).toEqual(["plain text"]);
    expect(summarizeEvent("[1,2]").fields).toEqual(["[1,2]"]);
    expect(summarizeEvent("null").fields).toEqual(["—"]);
  });
});
