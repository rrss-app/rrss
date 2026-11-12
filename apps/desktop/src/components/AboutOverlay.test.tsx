import { afterEach, expect, it } from "vitest";
import { AboutOverlay } from "./AboutOverlay";
import { createReactHarness } from "../test/react";

const harness = createReactHarness();
afterEach(harness.cleanup);
it("describes the app without renderer jargon", () => {
  const { host } = harness.mount(<AboutOverlay onClose={() => {}} />);
  expect(host.textContent).toContain("留在本机");
  expect(host.textContent).not.toContain("Block IR");
  expect(host.textContent).not.toContain("Ammonia");
});
