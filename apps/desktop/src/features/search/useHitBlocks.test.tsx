import { act, useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import type { SearchHitBlocksView } from "../../lib/api";
import { deferred } from "../../test/deferred";
import { createReactHarness } from "../../test/react";
import { useHitBlocks, type HitBlocks } from "./useHitBlocks";

const view = (id: number): SearchHitBlocksView => ({
  source_hash: `h${id}`, total: 8, title_match: false,
  hits: [{ block_id: id, ordinal: 2, section: "", text: `第 ${id} 篇的命中段` }],
});

describe("useHitBlocks", () => {
  const harness = createReactHarness();
  afterEach(() => harness.cleanup());

  it("waits for the selection to settle, caches per article and ignores late answers", async () => {
    const clock = harness.useClock();
    const calls: number[] = [];
    const gates = new Map<number, ReturnType<typeof deferred<SearchHitBlocksView>>>();
    const load = (id: number) => {
      calls.push(id);
      const gate = deferred<SearchHitBlocksView>();
      gates.set(id, gate);
      return gate.promise;
    };
    let state!: HitBlocks;
    let select!: (id: number | null) => void;
    let setOpen!: (open: boolean) => void;
    function Probe() {
      const [id, setId] = useState<number | null>(1);
      const [open, setOpenState] = useState(true);
      select = setId;
      setOpen = setOpenState;
      state = useHitBlocks({ active: open, articleId: id, query: "sqlite", load });
      return null;
    }
    harness.mount(<Probe />);

    // 悬停扫过 1 → 2 → 3：只有停稳的 3 发请求。
    expect(state.status).toBe("loading");
    await clock.advance(60);
    act(() => select(2));
    await clock.advance(60);
    act(() => select(3));
    await clock.advance(200);
    expect(calls).toEqual([3]);

    // 3 未返回时换到 4；3 迟到返回只进缓存，不改 4 的状态。
    act(() => select(4));
    await clock.advance(200);
    gates.get(3)!.resolve(view(3));
    await harness.flush();
    expect(state.status).toBe("loading");
    gates.get(4)!.resolve(view(4));
    await harness.flush();
    expect(state.data?.source_hash).toBe("h4");

    // 回到 3：直接用缓存，不再请求。
    act(() => select(3));
    expect(state.status).toBe("ready");
    expect(state.data?.source_hash).toBe("h3");
    await clock.advance(200);
    expect(calls).toEqual([3, 4]);

    // 失败只影响这一篇。
    act(() => select(5));
    await clock.advance(200);
    gates.get(5)!.reject(new Error("db busy"));
    await harness.flush();
    expect(state).toMatchObject({ status: "error", error: expect.stringContaining("db busy") });

    // 关闭浮层清空缓存；在途请求返回也不复活。
    act(() => select(6));
    await clock.advance(200);
    act(() => setOpen(false));
    gates.get(6)!.resolve(view(6));
    await harness.flush();
    act(() => setOpen(true));
    act(() => select(3));
    expect(state.status).toBe("loading");
    await clock.advance(200);
    expect(calls).toEqual([3, 4, 5, 6, 3]);
  });
});
