import { describe, expect, it } from "vitest";
import { formatRefreshTime } from "./refreshTime";

// Sunday 2026-11-22 11:20 local time.
const now = new Date(2026, 10, 22, 11, 20).getTime();
const at = (month: number, day: number, hour: number, minute: number, year = 2026) => new Date(year, month - 1, day, hour, minute).getTime();

describe("formatRefreshTime", () => {
  it("tiers by calendar distance and keeps the time of day", () => {
    const cases: [number, string][] = [
      [at(11, 22, 11, 32), "11:32"],
      [at(11, 23, 9, 10), "tomorrow 09:10"],
      [at(11, 21, 21, 10), "yesterday 21:10"],
      [at(11, 24, 9, 10), "Tue 09:10"],
      [at(11, 30, 8, 0), "Nov 30, 08:00"],
      [at(1, 3, 7, 0, 2027), "Jan 3, 2027, 07:00"],
    ];
    for (const [time, en] of cases) expect(formatRefreshTime(time, now, "en")).toBe(en);
  });
});
