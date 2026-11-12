import { describe, expect, it } from "vitest";
import { createPiFixture, FIXTURE_AI_PAGE, FIXTURE_PI_PAGE, FIXTURE_PI_PROBE } from "./pi";

describe("createPiFixture", () => {
  it("returns independent snapshots and does not succeed unknown writes", async () => {
    const { stub } = createPiFixture();
    const page = await stub.api.pi();
    page.pi_path = "changed";
    await expect(stub.api.pi()).resolves.toEqual(FIXTURE_PI_PAGE);
    await expect(stub.api.piProbe()).resolves.toEqual(FIXTURE_PI_PROBE);
    await expect(stub.api.ai()).resolves.toEqual(FIXTURE_AI_PAGE);
    await stub.api.restartPi().catch(() => {});
    expect(() => stub.assertNoUnexpectedCalls()).toThrow(/restartPi/);
  });

  it("makes writes visible on the next read and blocks page vs probe independently", async () => {
    const { stub } = createPiFixture();
    const pageWait = stub.deferNext("pi");
    const probeWait = stub.deferNext("piProbe");
    const pagePromise = stub.api.pi();
    const probePromise = stub.api.piProbe();
    probeWait.resolve({ ...FIXTURE_PI_PROBE, version: "late-probe" });
    await expect(probePromise).resolves.toMatchObject({ version: "late-probe" });
    pageWait.resolve(FIXTURE_PI_PAGE);
    await pagePromise;

    const saved = await stub.api.savePiSettings({ pi_path: "/opt/rrss-fixture/pi" });
    expect(saved).toMatchObject({ pi_path: "/opt/rrss-fixture/pi" });
    await stub.api.savePiSettings({ pi_path: "  /opt/rrss-fixture/pi2  " });
    await expect(stub.api.pi()).resolves.toMatchObject({ pi_path: "/opt/rrss-fixture/pi2" });
  });
});
