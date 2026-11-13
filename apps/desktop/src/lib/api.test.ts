import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { api, isSafeHttpUrl } from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => 7) }));

it("sends digest selections with canonical block IDs and the required revision", async () => {
  await api.citeDigestSelection(42, "revision-2", [{ blockId: 3, excerpt: "所选段落" }]);
  expect(invoke).toHaveBeenLastCalledWith("cite_digest_selection", {
    digestId: 42, revision: "revision-2", selections: [{ block_id: 3, excerpt: "所选段落" }],
  });
});

it("sends the generated camelCase chat contract with an optional digest anchor", async () => {
  await api.sendMessage(0, "展开第二条", [], 42);
  expect(invoke).toHaveBeenLastCalledWith("send_message", {
    args: { threadId: 0, text: "展开第二条", citations: [], digestId: 42, model: null },
  });
  await api.sendMessage(7, "继续", [], null, { provider_id: 2, model: "qwen" });
  expect(invoke).toHaveBeenLastCalledWith("send_message", {
    args: { threadId: 7, text: "继续", citations: [], digestId: null, model: { provider_id: 2, model: "qwen" } },
  });
  await api.sendMessage(7, "思考", [], null, null, "high");
  expect(invoke).toHaveBeenLastCalledWith("send_message", { args: {threadId:7,text:"思考",citations:[],digestId:null,model:null,thinking:"high"} });
  await api.sendMessage(7, "跟随", [], null, null, "");
  expect(invoke).toHaveBeenLastCalledWith("send_message", { args: {threadId:7,text:"跟随",citations:[],digestId:null,model:null,thinking:""} });
  await api.digestThread(42);
  expect(invoke).toHaveBeenLastCalledWith("digest_thread", { digestId: 42 });
});

it("omits scope on focus reads unless the caller passes one", async () => {
  await api.focus(null);
  expect(invoke).toHaveBeenLastCalledWith("focus_stream", {
    expandedCluster: null, more: false, excludeIds: null, excludeClusterIds: null,
  });
  await api.focus(3, { more: true, excludeIds: [1], excludeClusterIds: [2], scope: "folder:4" });
  expect(invoke).toHaveBeenLastCalledWith("focus_stream", {
    expandedCluster: 3, more: true, excludeIds: [1], excludeClusterIds: [2], scope: "folder:4",
  });
  await api.focusClusterMembers(9);
  expect(invoke).toHaveBeenLastCalledWith("focus_cluster_members", { clusterId: 9 });
  await api.focusClusterMembers(9, "folder:4");
  expect(invoke).toHaveBeenLastCalledWith("focus_cluster_members", { clusterId: 9, scope: "folder:4" });
});

describe("isSafeHttpUrl", () => {
  it("accepts http(s) and rejects javascript", () => {
    expect(isSafeHttpUrl("https://example.com/a")).toBe(true);
    expect(isSafeHttpUrl("http://library.example/hours")).toBe(true);
    expect(isSafeHttpUrl("javascript:alert(1)")).toBe(false);
    expect(isSafeHttpUrl("not a url")).toBe(false);
    expect(isSafeHttpUrl("")).toBe(false);
  });
});
