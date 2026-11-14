import { i18n } from "../i18n";
import { act, StrictMode, useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import type { ApiStub } from "../test/api";
import { createApiStub } from "../test/api";
import { deferred } from "../test/deferred";
import { createReactHarness } from "../test/react";
import { mcpFixture } from "../test/fixtures/settings";
import { McpSection } from "./McpSection";
import { forgetServer, startLogin } from "./mcpRuntime";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub) };
});
const harness = createReactHarness();
afterEach(() => {
  harness.cleanup();
  forgetServer(7); forgetServer(8);
  box.stub!.assertNoUnexpectedCalls();
});

it("retains completed logins during another write, then enables each service in order", async () => {
  const stub = box.stub = createApiStub();
  const first = stub.deferNext("loginMcp");
  const second = stub.deferNext("loginMcp");
  stub.handle("probeMcp", () => ({ ok: true, tools: [], elapsed_ms: 1, error: "", hint: "", output: "" }));
  const servers = [7, 8].map((id) => mcpFixture({ id, name: `server-${id}`, auth: "oauth", transport: "http" }));
  const enabled: number[] = [];
  const enabling = deferred<boolean>();
  let unlock: () => void;
  function Screen() {
    const [writing, setWriting] = useState(true);
    unlock = () => setWriting(false);
    return <McpSection servers={servers} writing={writing} onAdd={vi.fn()} onImport={vi.fn()} onEdit={vi.fn()}
      onSave={async () => true} onLoginFailed={vi.fn()} onToggle={(server) => {
        enabled.push(server.id);
        return server.id === 7 ? enabling.promise : Promise.resolve(true);
      }} />;
  }
  harness.mount(<StrictMode><Screen /></StrictMode>);
  act(() => { startLogin(7); startLogin(8); });
  await harness.flush();
  first.resolve({ ok: true }); second.resolve({ ok: true });
  await harness.flush();
  expect(enabled).toEqual([]);
  act(() => unlock());
  await harness.flush();
  expect(enabled).toEqual([7]);
  enabling.resolve(true);
  await harness.flush();
  expect(enabled).toEqual([7, 8]);
});

it("keeps a successful login when the write channel becomes busy before the effect runs", async () => {
  const stub = box.stub = createApiStub();
  const login = stub.deferNext("loginMcp");
  const probed: number[] = [];
  stub.handle("probeMcp", (input) => {
    probed.push(input.id);
    return { ok: true, tools: [], elapsed_ms: 1, error: "", hint: "", output: "" };
  });
  const servers = [mcpFixture({ id: 7, auth: "oauth", transport: "http" })];
  const enabled: number[] = [];
  let occupied = true;
  let unlock: () => void;
  function Screen() {
    const [writing, setWriting] = useState(false);
    unlock = () => { occupied = false; setWriting(false); };
    return <McpSection servers={servers} writing={writing} onAdd={vi.fn()} onImport={vi.fn()} onEdit={vi.fn()}
      onSave={async () => true} onLoginFailed={vi.fn()} onToggle={async (server) => {
        if (occupied) { setWriting(true); return "busy" as const; }
        enabled.push(server.id);
        return true;
      }} />;
  }
  harness.mount(<StrictMode><Screen /></StrictMode>);
  act(() => startLogin(7));
  await harness.flush();
  login.resolve({ ok: true });
  await harness.flush();
  expect(enabled).toEqual([]);
  expect(probed).toEqual([]);
  act(() => unlock());
  await harness.flush();
  expect(enabled).toEqual([7]);
  expect(probed).toEqual([7]);
});

it("consumes a real enable failure and allows the next login to proceed", async () => {
  const stub = box.stub = createApiStub();
  stub.handle("loginMcp", () => ({ ok: true }));
  const probed: number[] = [];
  stub.handle("probeMcp", (input) => {
    probed.push(input.id);
    return { ok: true, tools: [], elapsed_ms: 1, error: "", hint: "", output: "" };
  });
  const enabled: number[] = [];
  const attempted: number[] = [];
  const servers = [7, 8].map((id) => mcpFixture({ id, auth: "oauth", transport: "http" }));
  harness.mount(<McpSection servers={servers} writing={false} onAdd={vi.fn()} onImport={vi.fn()} onEdit={vi.fn()}
    onSave={async () => true} onLoginFailed={vi.fn()} onToggle={async (server) => {
      attempted.push(server.id);
      if (server.id === 7) return false; // 写入层已给出失败提示；不自动重试。
      enabled.push(server.id);
      return true;
    }} />);
  act(() => { startLogin(7); startLogin(8); });
  await harness.flush();
  expect(attempted).toEqual([7, 8]);
  expect(enabled).toEqual([8]);
  expect(probed).toEqual([8]);
});

it("opens details for a probe failure supplied by code", async () => {
  const stub = box.stub = createApiStub();
  stub.handle("probeMcp", () => ({ ok: false, tools: [], elapsed_ms: 1, error: "", hint: "", output: "", failure: { code: "mcp_http_status" as const, params: { status: 503 }, detail: "fixture" }, hint_code: "remote" as const }));
  const server = mcpFixture({ id: 7, name: "fixture" });
  const { host } = harness.mount(<McpSection servers={[server]} writing={false} onAdd={vi.fn()} onImport={vi.fn()} onEdit={vi.fn()} onSave={async () => true} onLoginFailed={vi.fn()} onToggle={async () => true} />);
  const label = i18n.t("mcp.probeAria", { ns: "settingsAssistant", name: server.name });
  act(() => host.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!.click());
  await harness.flush();
  act(() => host.querySelector<HTMLButtonElement>("button[aria-expanded]")!.click());
  expect(host.querySelector(".mcp-error")?.getAttribute("data-error-code")).toBe("mcp_http_status");
});
