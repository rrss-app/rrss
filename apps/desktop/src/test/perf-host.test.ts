import { afterEach, describe, expect, it } from "vitest";
import { generateScene } from "../../perf/generate";
import { createPerfHostFactory, sceneToHostPayload, summarizeIpc } from "../../perf/host";
import type { ArticleView, ChatContextEvent, ThreadView, ChatStreamEvent, MessagesPage } from "../bindings";
import { installFakeClock } from "./clock";

describe("strict perf host", () => {
  const scene = generateScene("minimal");
  const payload = sceneToHostPayload(scene);
  let host = createPerfHostFactory(payload);

  afterEach(() => {
    host.dispose();
    host = createPerfHostFactory(sceneToHostPayload(scene));
  });

  it("records thread thinking and publishes context before done", async () => {
    const clock = installFakeClock();
    const events: Array<string> = [];
    const contexts: ChatContextEvent[] = [];
    try {
      const handler = host.transformCallback((raw: unknown) => {
        const e = raw as {event:string;payload:unknown};
        events.push(e.event);
        if(e.event === "chat:context") contexts.push(e.payload as ChatContextEvent);
      });
      await host.invoke("plugin:event|listen",{event:"chat:context",handler});
      await host.invoke("plugin:event|listen",{event:"chat:done",handler});
      const id = await host.invoke("send_message",{args:{threadId:0,text:"test",citations:[],thinking:"high"}}) as number;
      expect((await host.invoke("threads" ) as ThreadView[]).find(t=>t.id===id)?.chat_thinking).toBe("high");
      for(let i=0;i<scene.spec.streamSnapshotCount;i++) await clock.advance(scene.spec.streamIntervalMs);
      expect(events).toEqual(["chat:context","chat:done"]);
      expect(contexts[0]).toMatchObject({thread_id:id,context:{window:32000}});
      expect(contexts[0]!.context.used).toBeGreaterThan(0);
      expect((await host.invoke("threads") as ThreadView[]).find(t=>t.id===id)?.context).toEqual(contexts[0]!.context);
      await host.invoke("send_message",{args:{threadId:id,text:"keep",citations:[]}});
      expect((await host.invoke("threads") as ThreadView[]).find(t=>t.id===id)?.chat_thinking).toBe("high");
      for(let i=0;i<scene.spec.streamSnapshotCount;i++) await clock.advance(scene.spec.streamIntervalMs);
      await host.invoke("send_message",{args:{threadId:id,text:"clear",citations:[],thinking:""}});
      expect((await host.invoke("threads") as ThreadView[]).find(t=>t.id===id)?.chat_thinking).toBeNull();
    } finally {host.dispose();clock.restore();}
  });

  it("eval(toString()) still constructs a host", async () => {
    const source = createPerfHostFactory.toString();
    expect(source).toContain("focus_stream");
    expect(source).not.toMatch(/sk-[a-z0-9]+/i);
    const factory = eval(`(${source})`) as typeof createPerfHostFactory;
    const isolated = factory(sceneToHostPayload(scene));
    const page = (await isolated.invoke("focus_stream", {})) as { articles: ArticleView[]; has_more: boolean };
    expect(page.articles).toHaveLength(scene.spec.pageSize);
    expect(page.has_more).toBe(true);
    isolated.dispose();
  });

  it("paginates with excludeIds and rejects unknown commands without leaking payloads", async () => {
    const first = (await host.invoke("focus_stream", {
      more: false,
      excludeIds: [],
    })) as { articles: ArticleView[]; has_more: boolean };
    expect(first.articles).toHaveLength(scene.spec.pageSize);
    const excludeIds = first.articles.map((row) => row.id);
    const second = (await host.invoke("focus_stream", {
      more: true,
      excludeIds,
    })) as { articles: ArticleView[] };
    expect(second.articles[0]?.id).not.toBe(first.articles[0]?.id);
    expect(host.listedCount()).toBeGreaterThan(scene.spec.pageSize);

    await expect(host.invoke("not_a_command", { apiKey: "sk-secret-key", text: "secret-question" })).rejects.toThrow(
      /unexpected command not_a_command/,
    );
    const audit = host.audit();
    expect(audit.unexpected).toEqual(["not_a_command"]);
    expect(JSON.stringify(audit)).not.toContain("sk-secret-key");
    expect(JSON.stringify(audit)).not.toContain("secret-question");
    const ipc = summarizeIpc(audit.ipc);
    expect(ipc.some((row) => row.method === "not_a_command" && row.count === 1)).toBe(true);
  });

  it("marks read on open_article; article_reader is a pure snapshot", async () => {
    const id = scene.articles.find((row) => !row.read)?.id ?? 2;
    const before = (await host.invoke("focus_stream", { excludeIds: [] })) as { articles: ArticleView[] };
    expect(before.articles.find((row) => row.id === id)?.read).toBe(false);
    await host.invoke("article_reader", { id });
    const afterSnapshot = (await host.invoke("focus_stream", { excludeIds: [] })) as { articles: ArticleView[] };
    expect(afterSnapshot.articles.find((row) => row.id === id)?.read).toBe(false);
    await host.invoke("open_article", { id });
    const afterRead = (await host.invoke("focus_stream", { excludeIds: [] })) as { articles: ArticleView[] };
    expect(afterRead.articles.find((row) => row.id === id)?.read).toBe(true);

    await host.invoke("star_article", { id, starred: true });
    const afterStar = (await host.invoke("focus_stream", { excludeIds: [] })) as { articles: ArticleView[] };
    expect(afterStar.articles.find((row) => row.id === id)?.starred).toBe(true);
    const reader = (await host.invoke("article_reader", { id })) as { starred: boolean };
    expect(reader.starred).toBe(true);
  });

  it("returns cloned snapshots so callers cannot write through", async () => {
    const page = (await host.invoke("focus_stream", {})) as { articles: ArticleView[] };
    page.articles[0]!.title = "mutated";
    const again = (await host.invoke("focus_stream", {})) as { articles: ArticleView[] };
    expect(again.articles[0]?.title).not.toBe("mutated");
  });

  it("counts full JSON args UTF-8 length and does not store payload text", async () => {
    const shortArgs = { args: { threadId: scene.longThreadId, text: "短", citations: [], digestId: null } };
    const long = `合成载荷-${"文".repeat(4000)}`;
    const longArgs = { args: { threadId: scene.longThreadId, text: long, citations: [], digestId: null } };
    await host.invoke("send_message", shortArgs);
    await host.invoke("send_message", longArgs);
    const sends = host.audit().ipc.filter((row) => row.method === "send_message");
    expect(sends).toHaveLength(2);
    const expectedLong = new TextEncoder().encode(JSON.stringify(longArgs)).length;
    expect(sends[1]?.requestBytes).toBe(expectedLong);
    expect(sends[1]!.requestBytes).toBeGreaterThan(sends[0]!.requestBytes);
    expect(sends[0]?.requestBytes).toBe(new TextEncoder().encode(JSON.stringify(shortArgs)).length);
    const logged = JSON.stringify(host.audit());
    expect(logged).not.toContain(long);
    expect(logged).not.toContain("合成载荷");
  });

  it("emits start/delta/done snapshots under fake clock and persists +2 messages", async () => {
    const clock = installFakeClock();
    let isolated: ReturnType<typeof createPerfHostFactory> | undefined;
    try {
      isolated = createPerfHostFactory(sceneToHostPayload(scene));
      const starts: number[] = [];
      const deltas: ChatStreamEvent[] = [];
      const dones: number[] = [];
      const listen = async (event: "chat:start" | "chat:delta" | "chat:done") => {
        const handler = isolated!.transformCallback((raw: unknown) => {
          const row = raw as { event?: string; payload?: unknown };
          if (row.event !== event) return;
          if (event === "chat:start") starts.push(row.payload as number);
          else if (event === "chat:delta") deltas.push(row.payload as ChatStreamEvent);
          else dones.push(row.payload as number);
        });
        await isolated!.invoke("plugin:event|listen", { event, handler });
      };
      await listen("chat:start");
      await listen("chat:delta");
      await listen("chat:done");

      const before = ((await isolated.invoke("messages", { threadId: scene.longThreadId, limit: 200 })) as MessagesPage).messages;
      const threadId = (await isolated.invoke("send_message", {
        args: { threadId: scene.longThreadId, text: "hello-not-in-audit", citations: [], digestId: null },
      })) as number;
      expect(threadId).toBe(scene.longThreadId);
      expect(starts).toEqual([threadId]);
      expect(deltas).toHaveLength(0);

      await clock.advance(0);
      expect(deltas).toHaveLength(1);
      expect(deltas[0]?.thread_id).toBe(threadId);
      expect(deltas[0]?.document).toBeTruthy();
      expect((deltas[0]?.text.length ?? 0)).toBeGreaterThan(0);

      for (let i = 1; i < scene.spec.streamSnapshotCount; i++) {
        await clock.advance(scene.spec.streamIntervalMs);
      }
      expect(deltas).toHaveLength(scene.spec.streamSnapshotCount);
      expect(dones).toEqual([threadId]);
      expect(isolated.streamDone()).toBe(true);
      expect(isolated.streamLog()).toHaveLength(scene.spec.streamSnapshotCount);
      expect(isolated.streamLog()[0]).toEqual(
        expect.objectContaining({ seq: 1, textLength: expect.any(Number) }),
      );
      expect(JSON.stringify(isolated.streamLog())).not.toContain("hello-not-in-audit");

      const last = deltas[deltas.length - 1];
      const first = deltas[0];
      expect((last?.text.length ?? 0)).toBeGreaterThan(first?.text.length ?? 0);
      expect(last?.document).toBeTruthy();

      const rows = ((await isolated.invoke("messages", { threadId, limit: 200 })) as MessagesPage).messages;
      expect(rows).toHaveLength(before.length + 2);
      expect(rows.at(-2)?.role).toBe("user");
      expect(rows.at(-2)?.text).toBe("hello-not-in-audit");
      expect(rows.at(-1)?.role).toBe("assistant");
      expect((rows.at(-1)?.text.length ?? 0)).toBeGreaterThan(0);
      expect(rows.at(-1)?.streaming).toBe(false);

      const audit = isolated.audit();
      expect(audit.streamEmits).toBe(scene.spec.streamSnapshotCount);
      expect(JSON.stringify(audit)).not.toContain("hello-not-in-audit");
    } finally {
      isolated?.dispose();
      clock.restore();
    }
  });
});
