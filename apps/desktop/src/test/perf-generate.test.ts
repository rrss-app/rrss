import { describe, expect, it } from "vitest";
import {
  assertSceneInvariants,
  focusPageFrom,
  generateScene,
  paginateArticles,
  streamSnapshots,
} from "../../perf/generate";
import { looksLikeRemoteImageUrl } from "../../perf/media";
import { PERF_SEED, SCENES, type SceneName } from "../../perf/scenes";
import { percentileNearestRank, summarizeSamples } from "../../perf/stats";

const NAMES = Object.keys(SCENES) as SceneName[];

describe("perf scene generation", () => {
  it("is deterministic for a fixed seed and reports configured sizes", () => {
    for (const name of NAMES) {
      const a = generateScene(name);
      const b = generateScene(name);
      expect(a.spec.seed).toBe(PERF_SEED);
      expect(a.articles.map((row) => row.id)).toEqual(b.articles.map((row) => row.id));
      expect(a.articles.map((row) => row.title)).toEqual(b.articles.map((row) => row.title));
      expect(JSON.stringify(a.longReader.document)).toBe(JSON.stringify(b.longReader.document));
      expect(a.articles).toHaveLength(a.spec.articleCount);
      expect(a.messages[a.longThreadId]).toHaveLength(a.spec.historyMessageCount);
      assertSceneInvariants(a);
    }
  });

  it("uses legal ids, no remote img urls, and loopback media hrefs", () => {
    const scene = generateScene("minimal");
    expect(scene.articles.every((row) => Number.isInteger(row.id) && row.id > 0)).toBe(true);
    expect(new Set(scene.articles.map((row) => row.id)).size).toBe(scene.articles.length);
    const history = scene.messages[scene.longThreadId] ?? [];
    expect(history.every((row) => Number.isInteger(row.id) && row.id > 0)).toBe(true);
    const serialized = JSON.stringify({
      document: scene.longReader.document,
      media: scene.longReader.media,
    });
    expect(serialized).not.toMatch(/sk-[a-z0-9]+/i);
    expect(serialized).not.toContain("api_key");
    for (const item of scene.longReader.media) {
      expect(looksLikeRemoteImageUrl(item.href)).toBe(false);
      expect(item.href.startsWith("http://127.0.0.1:")).toBe(true);
      expect(looksLikeRemoteImageUrl(item.src)).toBe(false);
    }
  });

  it("paginates with excludeIds for a real second page", () => {
    const scene = generateScene("minimal");
    expect(scene.spec.articleCount).toBeGreaterThan(scene.spec.pageSize);
    const first = paginateArticles(
      scene.articles,
      { feedId: null, more: false, excludeIds: [], excludeClusterIds: [], filter: "all", listScope: "all" },
      scene.spec.pageSize,
    );
    expect(first.rows).toHaveLength(scene.spec.pageSize);
    expect(first.has_more).toBe(true);
    const excludeIds = first.rows.map((row) => row.id);
    const second = paginateArticles(
      scene.articles,
      { feedId: null, more: true, excludeIds, excludeClusterIds: [], filter: "all", listScope: "all" },
      scene.spec.pageSize,
    );
    expect(second.rows.length).toBeGreaterThan(0);
    const overlap = new Set(excludeIds);
    expect(second.rows.some((row) => overlap.has(row.id))).toBe(false);
    expect(first.rows[0]?.id).not.toBe(second.rows[0]?.id);
    expect(focusPageFrom(scene, {
      feedId: null,
      more: false,
      excludeIds: [],
      excludeClusterIds: [],
      filter: "all",
      listScope: "all",
    }).ranking.cluster_group).toBe(false);
  });

  it("does not alias returned page rows back into the scene corpus", () => {
    const scene = generateScene("minimal");
    const page = focusPageFrom(scene, {
      feedId: null,
      more: false,
      excludeIds: [],
      excludeClusterIds: [],
      filter: "all",
      listScope: "all",
    });
    page.articles[0]!.title = "mutated";
    expect(scene.articles[0]?.title).not.toBe("mutated");
  });
});

describe("perf stats and stream snapshots", () => {
  it("uses nearest-rank p95 and documents sample count", () => {
    const samples = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    expect(percentileNearestRank(samples, 0.95)).toBe(10);
    expect(percentileNearestRank([10, 20], 0.95)).toBe(20);
    const summary = summarizeSamples(samples);
    expect(summary.n).toBe(10);
    expect(summary.algorithm).toBe("nearest-rank");
    expect(summary.p50).toBe(5);
    expect(summary.p95).toBe(10);
    expect(percentileNearestRank([], 0.95)).toBeNull();
  });

  it("builds full snapshot streams rather than fragment deltas", () => {
    const snaps = streamSnapshots("abcdefghij", 5);
    expect(snaps).toHaveLength(5);
    expect(snaps.map((row) => row.text)).toEqual(["ab", "abcd", "abcdef", "abcdefgh", "abcdefghij"]);
    expect(snaps.every((row) => row.document.v === 1)).toBe(true);
    const lengths = snaps.map((row) => row.text.length);
    expect(new Set(lengths).size).toBe(lengths.length);
    expect(lengths.every((len, i) => i === 0 || len > lengths[i - 1]!)).toBe(true);
    for (const name of NAMES) {
      const scene = generateScene(name);
      expect(scene.spec.samples).toBeGreaterThanOrEqual(20);
      expect(scene.spec.streamSnapshotCount).toBeGreaterThanOrEqual(60);
      expect(scene.spec.streamIntervalMs).toBe(30);
      expect(streamSnapshots("abcdefghij", scene.spec.streamSnapshotCount)).toHaveLength(scene.spec.streamSnapshotCount);
    }
  });
});
