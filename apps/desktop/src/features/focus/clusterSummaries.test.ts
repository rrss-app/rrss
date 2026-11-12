import { afterEach, expect, it, vi } from "vitest";
import { api, type ClusterSummaryView } from "../../lib/api";
import { deferred } from "../../test/deferred";
import { boldSegments, createClusterSummaries } from "./clusterSummaries";

afterEach(() => { vi.restoreAllMocks(); });

const view = (state: string, overview = ""): ClusterSummaryView => ({
  cluster_id: 7, state, model_ready: true, estimate_tokens: 2000, overview, additions: [], conflicts: [],
  basis: 0, generated_at: null, fresh: [], members: [],
});

it("a read that returns after a newer generate does not overwrite the generated summary", async () => {
  const slowRead = deferred<ClusterSummaryView>();
  vi.spyOn(api, "clusterSummary").mockImplementationOnce(() => slowRead.promise);
  vi.spyOn(api, "generateClusterSummary").mockResolvedValue(view("ready", "综述"));
  const generated = vi.fn();
  const store = createClusterSummaries(() => undefined, generated);
  const reading = store.load(7);
  await store.generate(7);
  slowRead.resolve(view("none"));
  await reading;
  expect(store.get(7).view?.state).toBe("ready");
  expect(store.get(7).generating).toBe(false);
  expect(generated).toHaveBeenCalledWith(7);
});

it("summarize generates only when nothing exists yet, and a scope change drops results in flight", async () => {
  vi.spyOn(api, "clusterSummary").mockResolvedValue(view("ready", "已有"));
  const generate = vi.spyOn(api, "generateClusterSummary");
  const store = createClusterSummaries(() => "folder:3");
  await store.summarize(7);
  expect(generate).not.toHaveBeenCalled();
  expect(api.clusterSummary).toHaveBeenCalledWith(7, "folder:3");

  const pending = deferred<ClusterSummaryView>();
  generate.mockImplementationOnce(() => pending.promise);
  const running = store.generate(7);
  store.reset();
  pending.resolve(view("ready", "旧范围的结果"));
  await running;
  expect(store.get(7).view).toBeNull();
});

it("only double asterisks become bold", () => {
  expect(boldSegments("**新规**公布，*单星*照原样")).toEqual([
    { text: "新规", bold: true }, { text: "公布，*单星*照原样", bold: false },
  ]);
});
