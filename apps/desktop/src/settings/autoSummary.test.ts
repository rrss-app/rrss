import { describe, expect, it } from "vitest";
import { aiPageFixture } from "../test/fixtures/settings";
import { articlesFit, fitText, tokensToWanText, unlimitedText, wanTextToTokens } from "./autoSummary";

// 中档：单篇约 4,000、自动简报约 3.5 万；自动汇总每天 20 组 × 2,000。
const auto = aiPageFixture().auto_summary;

describe("自动摘要估算", () => {
  it("先扣自动简报，再按单篇算，10 篇以上取整到 5", () => {
    expect(articlesFit(auto, 200_000, false)).toBe(40);
    expect(articlesFit(auto, 70_000, false)).toBe(8);
    expect(fitText(auto, 200_000, false)).toBe("扣除自动简报约 3.5 万后约够 40 篇 / 天");
  });

  it("开着自动汇总时一并扣掉汇总的上限", () => {
    expect(articlesFit(auto, 200_000, true)).toBe(30);
    expect(fitText(auto, 200_000, true)).toBe("扣除自动简报约 3.5 万、自动汇总约 4 万后约够 30 篇 / 天");
  });

  it("预算不够简报时直说，够到上限时说预算不是瓶颈", () => {
    expect(fitText(auto, 30_000, false)).toBe("只够自动简报（约 3.5 万 / 天），自动摘要基本不会运行");
    expect(fitText(auto, 60_000, true)).toBe("只够自动简报和自动汇总（约 7.5 万 / 天），自动摘要基本不会运行");
    expect(fitText(auto, 1_000_000, false)).toBe("扣除自动简报约 3.5 万后超过每天 100 篇的上限，预算不是瓶颈");
  });

  it("不限额时写开启后每天最多约多少", () => {
    expect(unlimitedText(auto, false)).toBe("自动摘要每天最多约 43.5 万 token（100 篇 × 约 4,000，加自动简报约 3.5 万，估算）");
  });

  it("预算按万 token 输入，存 token；非法输入原样留给预检", () => {
    expect(tokensToWanText("200000")).toBe("20");
    expect(tokensToWanText("15000")).toBe("1.5");
    expect(wanTextToTokens("1.5")).toBe("15000");
    expect(wanTextToTokens(" 0 ")).toBe("0");
    expect(wanTextToTokens("abc")).toBe("abc");
    expect(wanTextToTokens("-1")).toBe("-1");
  });
});
