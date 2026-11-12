import { describe, expect, it } from "vitest";
import { deferred } from "./deferred";

describe("deferred", () => {
  it("resolves the waiting caller", async () => {
    const pending = deferred<number>();
    expect(pending.settled).toBe(false);
    pending.resolve(7);
    await expect(pending.promise).resolves.toBe(7);
    expect(pending.settled).toBe(true);
  });

  it("rejects the waiting caller", async () => {
    const pending = deferred<string>();
    pending.reject(new Error("spawn failed"));
    await expect(pending.promise).rejects.toThrow("spawn failed");
    expect(pending.settled).toBe(true);
  });

  it("keeps the first settle and ignores later resolve or reject", async () => {
    const pending = deferred<number>();
    pending.resolve(1);
    pending.resolve(2);
    pending.reject(new Error("too late"));
    await expect(pending.promise).resolves.toBe(1);
    expect(pending.settled).toBe(true);
  });

  it("completes two deferreds of the same command out of order without sleep", async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const seen: string[] = [];
    const p1 = first.promise.then((value) => {
      seen.push(`first:${value}`);
    });
    const p2 = second.promise.then((value) => {
      seen.push(`second:${value}`);
    });
    second.resolve("b");
    await p2;
    expect(seen).toEqual(["second:b"]);
    first.resolve("a");
    await p1;
    expect(seen).toEqual(["second:b", "first:a"]);
  });
});
