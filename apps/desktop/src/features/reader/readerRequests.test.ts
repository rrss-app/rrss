import { describe, expect, it } from "vitest";
import { createReaderRequestTracker } from "./readerRequests";

describe("reader request tracker", () => {
  it("a newer open invalidates the earlier ticket even for the same article", () => {
    const t = createReaderRequestTracker();
    const first = t.open(1);
    const second = t.open(2);
    const third = t.open(1);
    expect(t.owns(first)).toBe(false);
    expect(t.owns(second)).toBe(false);
    expect(t.owns(third)).toBe(true);
    expect(t.current()).toEqual({ articleId: 1, session: 3 });
  });

  it("close invalidates the open ticket and a later open starts a fresh session", () => {
    const t = createReaderRequestTracker();
    const ticket = t.open(1);
    t.close();
    expect(t.owns(ticket)).toBe(false);
    expect(t.current()).toBeNull();
    expect(t.beginOp("summary", ticket)).toBeNull();
    const again = t.open(1);
    expect(t.owns(again)).toBe(true);
    expect(again.session).not.toBe(ticket.session);
  });

  it("ops belong to their session and are superseded per kind", () => {
    const t = createReaderRequestTracker();
    const shownA = t.open(1);
    const summary1 = t.beginOp("summary", shownA)!;
    const translate1 = t.beginOp("translate", shownA)!;
    const summary2 = t.beginOp("summary", shownA)!;
    expect(t.ownsOp(summary1)).toBe(false);
    expect(t.ownsOp(summary2)).toBe(true);
    expect(t.ownsOp(translate1)).toBe(true);
    const pendingB = t.open(2);
    expect(t.ownsOp(summary2)).toBe(false);
    expect(t.ownsOp(translate1)).toBe(false);
    // B 还没显示时，对仍在屏幕上的 A 发起工具操作应被忽略，而不是替 B 发请求。
    expect(t.beginOp("summary", shownA)).toBeNull();
    const summary3 = t.beginOp("summary", pendingB)!;
    expect(t.ownsOp(summary3)).toBe(true);
    expect(summary3.articleId).toBe(2);
  });
});
