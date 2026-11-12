import { describe, expect, it } from "vitest";
import { formatPublished, formatPublishedFull } from "./time";

// 用本地日历构造时刻，断言与运行机器的时区无关。
const at = (y: number, m: number, d: number, h = 0, min = 0, s = 0) => new Date(y, m - 1, d, h, min, s).getTime();

describe("formatPublished", () => {
  const now = at(2026, 9, 27, 15, 30);

  it("uses relative wording only for today", () => {
    expect(formatPublished(now - 20_000, now)).toBe("刚刚");
    expect(formatPublished(now + 20_000, now)).toBe("刚刚");
    expect(formatPublished(at(2026, 9, 27, 15, 25), now)).toBe("5 分钟前");
    expect(formatPublished(at(2026, 9, 27, 12, 10), now)).toBe("3 小时前");
    expect(formatPublished(at(2026, 9, 27, 0, 10), now)).toBe("15 小时前");
  });

  it("switches to yesterday with a local clock time, then to a local date", () => {
    expect(formatPublished(at(2026, 9, 26, 21, 40), now)).toBe("昨天 21:40");
    expect(formatPublished(at(2026, 9, 26, 0, 5), now)).toBe("昨天 00:05");
    expect(formatPublished(at(2026, 9, 25, 23, 59), now)).toBe("9月25日");
    expect(formatPublished(at(2025, 12, 30, 8), now)).toBe("2025年12月30日");
  });

  it("follows the local calendar across a new year and ignores missing or skewed times", () => {
    const newYear = at(2026, 1, 1, 0, 30);
    expect(formatPublished(at(2025, 12, 31, 23), newYear)).toBe("昨天 23:00");
    expect(formatPublished(at(2025, 12, 30, 23), newYear)).toBe("2025年12月30日");
    expect(formatPublished(null, now)).toBe("");
    expect(formatPublished(at(2026, 9, 27, 18), now)).toBe("9月27日");
  });

  it("gives a full local time for tooltips", () => {
    expect(formatPublishedFull(at(2026, 9, 5, 7, 3))).toBe("2026年9月5日 07:03");
    expect(formatPublishedFull(null)).toBe("");
  });
});
