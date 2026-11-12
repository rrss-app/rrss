import { act, StrictMode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { McpInput, McpProbeView, McpView } from "../bindings";
import { createApiStub, type ApiStub } from "../test/api";
import { createReactHarness } from "../test/react";
import { mcpFixture } from "../test/fixtures/settings";
import { McpDialog } from "./McpDialogs";
import type { PiAction } from "./piQuery";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub) };
});
const harness = createReactHarness();
let client: QueryClient;
let saved: McpInput | undefined;
beforeEach(() => { client = new QueryClient(); box.stub = createApiStub(); saved = undefined; });
afterEach(() => { harness.cleanup(); client.clear(); box.stub!.assertNoUnexpectedCalls(); });

function mount(server?: McpView) {
  const run = async (action: PiAction) => {
    if (action.type !== "mcpSave") throw new Error("unexpected action");
    saved = action.input;
    return true;
  };
  return harness.mount(<QueryClientProvider client={client}><StrictMode>
    <McpDialog server={server} run={run} onAsk={vi.fn()} onClose={vi.fn()} onEditStateChange={vi.fn()} onPaste={vi.fn()} />
  </StrictMode></QueryClientProvider>);
}
function input(host: HTMLElement, label: string) {
  const id = [...host.querySelectorAll("label")].find((node) => node.textContent === label)!.htmlFor;
  return host.querySelector<HTMLInputElement>(`[id="${id}"]`)!;
}
function fill(node: HTMLInputElement, value: string) {
  act(() => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(node, value);
    node.dispatchEvent(new Event("input", { bubbles: true }));
  });
}
function button(host: HTMLElement, name: string) {
  return [...host.querySelectorAll("button")].find((node) => node.textContent === name)!;
}
const click = (node: HTMLButtonElement) => act(() => node.click());
const result: McpProbeView = { ok: true, elapsed_ms: 1, tools: [
  { name: "read_old", description: "", writes: false }, { name: "write_old", description: "", writes: true },
], error: "", hint: "", output: "" };

it.each(["pending", "complete"])("discards %s probe results after the command changes", async (phase) => {
  const pending = box.stub!.deferNext("probeMcp");
  const { host } = mount();
  fill(input(host, "启动命令"), "runner-a");
  click(button(host, "检测连接"));
  if (phase === "complete") { pending.resolve(result); await harness.flush(); }
  fill(input(host, "启动命令"), "runner-b");
  if (phase === "pending") { pending.resolve(result); await harness.flush(); }
  click(button(host, "保存"));
  await harness.flush();
  expect(saved).toMatchObject({ command: "runner-b", include_tools: [] });
});

it("preserves a valid probe selection when only the display name changes", async () => {
  const pending = box.stub!.deferNext("probeMcp");
  const { host } = mount();
  fill(input(host, "启动命令"), "runner");
  click(button(host, "检测连接"));
  pending.resolve(result);
  await harness.flush();
  fill(input(host, "名称"), "renamed");
  click(button(host, "保存"));
  await harness.flush();
  expect(saved).toMatchObject({ name: "renamed", include_tools: ["read_old"] });
});

it("preserves mixed quotes and Windows paths when editing only the service name", async () => {
  const server = mcpFixture({ command: String.raw`C:\Program Files\runner.exe`, args: ["-e", `console.log("it's ok")`, ""] });
  const { host } = mount(server);
  fill(input(host, "名称"), "renamed");
  click(button(host, "保存"));
  await harness.flush();
  expect(saved).toMatchObject({ command: server.command, args: server.args });
});

it.each(["地址", "令牌"])("invalidates a remote probe when %s changes", async (field) => {
  const server = mcpFixture({ transport: "http", url: "https://old.example.test/mcp", auth: "token",
    headers: [{ name: "Authorization", has_value: true }], include_tools: ["saved_tool"] });
  const pending = box.stub!.deferNext("probeMcp");
  const { host } = mount(server);
  click(button(host, "检测连接"));
  pending.resolve(result);
  await harness.flush();
  fill(input(host, field), field === "地址" ? "https://new.example.test/mcp" : "synthetic-new-token");
  click(button(host, "保存"));
  await harness.flush();
  expect(saved?.include_tools).toEqual(["saved_tool"]);
  expect(saved?.url).toBe(field === "地址" ? "https://new.example.test/mcp" : server.url);
});

it("blocks incomplete commands until the user closes the quote", async () => {
  const { host } = mount();
  fill(input(host, "启动命令"), 'runner "unfinished');
  click(button(host, "检测连接"));
  click(button(host, "保存"));
  await harness.flush();
  expect(saved).toBeUndefined();
  fill(input(host, "启动命令"), 'runner "unfinished"');
  click(button(host, "保存"));
  await harness.flush();
  expect(saved).toMatchObject({ command: "runner", args: ["unfinished"] });
});
