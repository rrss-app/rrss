import { expect, test, type Locator, type Page } from "./fixtures";
import { installMockHost } from "./mockHost";
import { readerReturnFixture } from "../src/test/fixtures/reader";

const fixture = readerReturnFixture(2);
const articleTitle = fixture.readers[2].title;
const question = "What did the fixture record?";

async function configureHost(page: Page, acceptanceMs: number, replyMs: number) {
  await page.evaluate(({ acceptanceMs, replyMs }) => {
    const hostWindow = window as unknown as {
      __mockHost: { chatAcceptanceDelayMs: number; chatReplyDelayMs: number };
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown) => Promise<unknown> };
    };
    hostWindow.__mockHost.chatAcceptanceDelayMs = acceptanceMs;
    hostWindow.__mockHost.chatReplyDelayMs = replyMs;
    const host = hostWindow.__TAURI_INTERNALS__;
    const invoke = host.invoke.bind(host);
    host.invoke = async (command, args) => {
      const result = await invoke(command, args);
      if (command === "send_message") window.dispatchEvent(new Event("fixture-send-accepted"));
      return result;
    };
  }, { acceptanceMs, replyMs });
}

async function cite(trigger: Locator, expectedChips: number) {
  return trigger.evaluate((node, count) => new Promise<number>((resolve, reject) => {
    const started = performance.now();
    (node as HTMLElement).click();
    const check = () => {
      const chips = document.querySelectorAll(".drawer.reader .reader-citation-chip");
      if (chips.length === count && chips[count - 1].checkVisibility({ visibilityProperty: true })) resolve(performance.now() - started);
      else if (performance.now() - started > 5_000) reject(new Error("Citation did not appear"));
      else requestAnimationFrame(check);
    };
    requestAnimationFrame(check);
  }), expectedChips);
}

async function sample(page: Page, kind: "article" | "spans") {
  const reader = page.locator(".drawer.reader");
  await page.getByRole("article", { name: articleTitle, exact: true }).click();
  await expect(reader.locator(".reader-title")).toHaveText(articleTitle);
  const citations: number[] = [];
  if (kind === "article") {
    await reader.locator('summary[aria-label="More"]').click();
    citations.push(await cite(reader.getByRole("button", { name: "Cite the full article in chat", exact: true }), 1));
  } else {
    for (const [index, blockId] of [8, 9, 10].entries()) {
      const block = reader.locator(`[data-reader-document] > [data-block-id="${blockId}"]`);
      await block.scrollIntoViewIfNeeded();
      await block.evaluate(node => {
        const range = document.createRange();
        range.selectNodeContents(node);
        const selection = window.getSelection()!;
        selection.removeAllRanges();
        selection.addRange(range);
        node.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, button: 0 }));
      });
      citations.push(await cite(page.getByRole("menuitem", { name: "Cite", exact: true }), index + 1));
    }
  }
  const tray = page.getByRole("region", { name: "Citation tray" });
  await tray.getByRole("textbox").fill(question);
  const send = tray.getByRole("button", { name: "Send", exact: true });
  await expect(send).toBeEnabled();
  const chat = await send.evaluate((node, question) => new Promise<{ accepted: number; visible: number; firstText: number }>((resolve, reject) => {
    let accepted: number | undefined;
    let visible: number | undefined;
    window.addEventListener("fixture-send-accepted", () => { accepted = performance.now() - started; }, { once: true });
    const started = performance.now();
    (node as HTMLElement).click();
    const check = () => {
      const elapsed = performance.now() - started;
      const reader = document.querySelector(".drawer.reader");
      const user = [...document.querySelectorAll(".chat-pane .bubble.user")].at(-1);
      const reply = [...document.querySelectorAll(".chat-pane .bubble.assistant")].at(-1);
      const shown = (element: Element | null | undefined) => element?.checkVisibility({ visibilityProperty: true });
      if (!shown(reader) && shown(user) && user?.textContent?.includes(question)) visible ??= elapsed;
      if (visible !== undefined && accepted !== undefined && shown(reply) && reply?.textContent?.trim()) {
        resolve({ accepted, visible, firstText: elapsed - accepted });
      } else if (elapsed > 5_000) reject(new Error("Send milestones did not all arrive"));
      else requestAnimationFrame(check);
    };
    requestAnimationFrame(check);
  }), question);
  return { citations, ...chat };
}

test("measure reader citation, send visibility, and first text separately with a fake host", async ({ page }) => {
  test.skip(process.env.RRSS_CITATION_LATENCY !== "1", "Opt-in probe; run with RRSS_CITATION_LATENCY=1");
  test.setTimeout(90_000);
  await installMockHost(page, { ...fixture, systemLanguage: "en", threads: [], messages: {} });
  for (const kind of ["article", "spans"] as const) {
    for (const acceptanceMs of [0, 240]) {
      const samples: Record<"cold" | "warm", Awaited<ReturnType<typeof sample>>[]> = { cold: [], warm: [] };
      for (let trial = 0; trial < 5; trial++) {
        await page.goto("/");
        await page.getByRole("article", { name: articleTitle, exact: true }).waitFor();
        await configureHost(page, acceptanceMs, 140);
        samples.cold.push(await sample(page, kind));
        await page.locator(".sidebar").getByRole("button", { name: "Today", exact: true }).click();
        samples.warm.push(await sample(page, kind));
      }
      const median = (values: number[]) => values.sort((a, b) => a - b)[Math.floor(values.length / 2)].toFixed(1);
      for (const [stage, measurements] of Object.entries(samples)) {
        const citation = measurements[0].citations.map((_, index) => median(measurements.map(row => row.citations[index])));
        console.log(`fake host ${kind} ${stage} delay=${acceptanceMs}ms n=${measurements.length} median ms: cite->chip [${citation.join(", ")}]; send->return ${median(measurements.map(row => row.accepted))}; send->visible ${median(measurements.map(row => row.visible))}; return->first text ${median(measurements.map(row => row.firstText))}`);
      }
    }
  }
});
