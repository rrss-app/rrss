import { describe, expect, it } from "vitest";
import { createApiStub, UnexpectedApiCallError, type ApiStub } from "./api";
import { deferred } from "./deferred";
import { FIXTURE_AI_PAGE, FIXTURE_PI_PAGE } from "./fixtures/pi";
import type { UiError } from "../bindings";

function expectMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

describe("createApiStub", () => {
  it("resolves and rejects registered or deferred calls", async () => {
    const stub = createApiStub();
    stub.handle("pi", () => FIXTURE_PI_PAGE);
    await expect(stub.api.pi()).resolves.toEqual(FIXTURE_PI_PAGE);

    const hanging = stub.deferNext("piProbe");
    const pending = stub.api.piProbe();
    hanging.reject({ code: "internal", detail: "spawn failed" } satisfies UiError);
    await expect(pending).rejects.toMatchObject({ code: "internal" });
  });

  it("lets two calls of the same method finish out of order", async () => {
    const stub = createApiStub();
    const first = stub.deferNext("pi");
    const second = stub.deferNext("pi");
    const seen: string[] = [];
    const p1 = stub.api.pi().then((page) => {
      seen.push(page.manage_status);
    });
    const p2 = stub.api.pi().then((page) => {
      seen.push(page.manage_status);
    });
    second.resolve({ ...FIXTURE_PI_PAGE, manage_status: "second" });
    await p2;
    first.resolve({ ...FIXTURE_PI_PAGE, manage_status: "first" });
    await p1;
    expect(seen).toEqual(["second", "first"]);
    expect(stub.callCount("pi")).toBe(2);
  });

  it("audits unknown calls even when production code catches them", async () => {
    const stub = createApiStub();
    try {
      await stub.api.saveAiSettings({
        provider: { id: 1, name: "x", base_url: "http://127.0.0.1", api_key: "sk-secret-key", protocol: "chat" },
      });
    } catch (err) {
      expect(err).toBeInstanceOf(UnexpectedApiCallError);
      expect(expectMessage(err)).toBe("unexpected api.saveAiSettings call");
      expect(expectMessage(err)).not.toContain("sk-secret-key");
    }
    expect(stub.callCount("saveAiSettings")).toBe(1);
    expect(() => stub.assertNoUnexpectedCalls()).toThrow(/saveAiSettings/);
    try {
      stub.assertNoUnexpectedCalls();
    } catch (err) {
      expect(expectMessage(err)).not.toContain("sk-secret-key");
    }
  });

  it("does not treat a registered handler failure as an unexpected call", async () => {
    const stub = createApiStub();
    stub.handle("piProbe", () => {
      throw new Error("spawn failed");
    });
    await expect(stub.api.piProbe()).rejects.toThrow("spawn failed");
    stub.assertNoUnexpectedCalls();
  });

  it("keeps handler writes visible on the next read and isolates returned objects", async () => {
    const stub = createApiStub();
    let path = "";
    stub.handle("savePiSettings", (args) => {
      path = args.pi_path;
      return { ...FIXTURE_PI_PAGE, pi_path: path };
    });
    stub.handle("pi", () => ({ ...FIXTURE_PI_PAGE, pi_path: path }));
    await stub.api.savePiSettings({ pi_path: "/opt/rrss-fixture/pi" });
    const first = await stub.api.pi();
    first.pi_path = "mutated";
    const second = await stub.api.pi();
    expect(second.pi_path).toBe("/opt/rrss-fixture/pi");
  });

  it("isolates handlers across stub instances", async () => {
    const a = createApiStub();
    const b = createApiStub();
    a.handle("pi", () => ({ ...FIXTURE_PI_PAGE, manage_status: "a" }));
    b.handle("pi", () => ({ ...FIXTURE_PI_PAGE, manage_status: "b" }));
    await expect(a.api.pi()).resolves.toMatchObject({ manage_status: "a" });
    await expect(b.api.pi()).resolves.toMatchObject({ manage_status: "b" });
    expect(a.callCount("pi")).toBe(1);
    expect(b.callCount("pi")).toBe(1);
  });

  it("commits a write only after the handler gate resolves", async () => {
    const stub = createApiStub();
    let path = "";
    stub.handle("pi", () => ({ ...FIXTURE_PI_PAGE, pi_path: path }));
    const commit = deferred<void>();
    const save = (piPath: string) => stub.api.savePiSettings({ pi_path: piPath });
    stub.handle("savePiSettings", async (args) => {
      await commit.promise;
      path = args.pi_path;
      return { ...FIXTURE_PI_PAGE, pi_path: path };
    });
    const pending = save("/opt/rrss-fixture/pi");
    await expect(stub.api.pi()).resolves.toMatchObject({ pi_path: "" });
    commit.resolve();
    await pending;
    await expect(stub.api.pi()).resolves.toMatchObject({ pi_path: "/opt/rrss-fixture/pi" });

    const denied = deferred<void>();
    stub.handle("savePiSettings", async (args) => {
      await denied.promise;
      path = args.pi_path;
      return { ...FIXTURE_PI_PAGE, pi_path: path };
    });
    const rejected = save("/tmp/should-not-stick");
    await expect(stub.api.pi()).resolves.toMatchObject({ pi_path: "/opt/rrss-fixture/pi" });
    denied.reject(new Error("save failed"));
    await expect(rejected).rejects.toThrow("save failed");
    await expect(stub.api.pi()).resolves.toMatchObject({ pi_path: "/opt/rrss-fixture/pi" });
  });
});

function _apiTypeContract(stub: ApiStub) {
  // @ts-expect-error pi takes no arguments
  void stub.api.pi("/etc/passwd");
  // @ts-expect-error idle timeout is a number
  void stub.api.savePiSettings({ pi_path: "", idle_timeout: "1800", thinking: "medium" });
  // @ts-expect-error handler must return PiPage
  stub.handle("pi", () => 0);
}
void _apiTypeContract;
