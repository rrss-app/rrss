#!/usr/bin/env node
// Smoke-test the real MCP stdio server against an already running debug RRSS.
// No extra Node dependencies: the configured MCP server is launched through npx.
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { finalizeHostReport, hostFacts, perfConsoleLine, perfProbeScript } from "./perf-metrics.mjs";

function mediaProbeScript(expect) {
  return `(async () => {
    const expect = ${JSON.stringify(expect)};
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    async function waitFor(label, read, budgetMs) {
      const deadline = performance.now() + budgetMs;
      let last = null;
      while (performance.now() < deadline) {
        last = read();
        if (last) return last;
        await sleep(50);
      }
      throw new Error(label);
    }
    const all = [...document.querySelectorAll('button.tab')].find((node) => node.textContent.trim() === '全部条目');
    if (!all) throw new Error('Focus filter tab missing');
    all.click();
    const card = await waitFor('Media fixture card did not appear', () => document.querySelector('article[aria-label="媒体夹具"]'), 8000);
    card.click();
    const img = await waitFor('Reader did not render a cached image', () => {
      const node = document.querySelector('.drawer.reader img.reader-img');
      return node instanceof HTMLImageElement && node.getAttribute('src') ? node : null;
    }, 8000);
    const src = img.currentSrc || img.getAttribute('src') || '';
    const prefix = 'rrss://localhost/media/' + expect.mediaId;
    if (!src.startsWith(prefix)) throw new Error('img src is not the rrss media protocol: ' + src);
    if (/^https?:/i.test(src)) throw new Error('img uses a network URL: ' + src);
    await img.decode();
    if (img.naturalWidth !== expect.width || img.naturalHeight !== expect.height) {
      throw new Error('decoded ' + img.naturalWidth + 'x' + img.naturalHeight + ', expected ' + expect.width + 'x' + expect.height);
    }
    const remote = performance.getEntriesByType('resource')
      .map((entry) => entry.name)
      .filter((name) => /^https?:/i.test(name) && (name.includes('/media/' + expect.mediaId) || name.includes(expect.src)));
    if (remote.length) throw new Error('image was fetched over HTTP: ' + remote[0]);
    return 'rrss-media-smoke-ok ' + src + ' ' + img.naturalWidth + 'x' + img.naturalHeight;
  })()`;
}

function streamProbeScript(expect) {
  return `(async () => {
    const expect = ${JSON.stringify(expect)};
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const listen = window.__TAURI__.event.listen;
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const deltas = [];
    const errors = [];
    let done = false;
    const unDelta = await listen('chat:delta', (event) => deltas.push(event.payload));
    const unDone = await listen('chat:done', () => { done = true; });
    const unError = await listen('chat:error', (event) => errors.push(event.payload && event.payload.message || String(event.payload)));
    const observed = [];
    const note = () => {
      const bubble = [...document.querySelectorAll('.chat-pane .bubble.assistant')].at(-1);
      observed.push(bubble ? bubble.textContent || '' : '');
    };
    const observer = new MutationObserver(note);
    observer.observe(document.body, { subtree: true, childList: true, characterData: true });
    try {
      const newer = [...document.querySelectorAll('button')].find((node) => node.textContent.trim() === '新对话');
      if (!newer) throw new Error('New chat button missing');
      newer.click();
      const deadline = performance.now() + 8000;
      let box = null;
      while (performance.now() < deadline) {
        box = document.querySelector('textarea[aria-label="对话消息"]');
        if (box) break;
        await sleep(50);
      }
      if (!box) throw new Error('Chat composer did not open');
      const proto = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value');
      proto.set.call(box, '持续生成');
      box.dispatchEvent(new Event('input', { bubbles: true }));
      const sendDeadline = performance.now() + 4000;
      let send = null;
      while (performance.now() < sendDeadline) {
        const candidate = document.querySelector('.composer button.btn.primary');
        if (candidate instanceof HTMLElement && !candidate.disabled) {
          send = candidate;
          break;
        }
        await sleep(50);
      }
      if (!send) throw new Error('Send button is not ready');
      send.click();
      const until = performance.now() + 30000;
      while (!done && performance.now() < until) {
        note();
        await sleep(20);
      }
      note();
      if (!done) throw new Error('Stream did not settle' + (errors.length ? ': ' + errors[0] : ''));
      if (errors.length) throw new Error(errors[0]);
      const distinct = [];
      for (const row of deltas) {
        const text = row && row.text || '';
        if (!distinct.length || text.length > distinct[distinct.length - 1].length) distinct.push(text);
      }
      if (distinct.length !== expect.snapshots) {
        throw new Error('distinct snapshots ' + distinct.length + ' != ' + expect.snapshots + ' (emitted ' + deltas.length + ')');
      }
      const used = new Set();
      let matched = 0;
      for (const text of distinct) {
        let found = -1;
        for (let i = 0; i < observed.length; i++) {
          if (used.has(i) || observed[i] !== text) continue;
          found = i;
          break;
        }
        if (found >= 0) {
          used.add(found);
          matched += 1;
        }
      }
      const missed = distinct.length - matched;
      if (missed !== 0) throw new Error('missed ' + missed + ' of ' + distinct.length + ' distinct snapshots; emitted ' + deltas.length);
      return 'rrss-stream-smoke-ok emitted=' + deltas.length + ' distinct=' + distinct.length + ' matched=' + matched + ' missed=' + missed;
    } finally {
      observer.disconnect();
      unDelta();
      unDone();
      unError();
      await invoke('stop_pi', {}).catch(() => {});
    }
  })()`;
}

function switchProbeScript(expect) {
  return `(async () => {
    const expect = ${JSON.stringify(expect)};
    const nav = {
      focus: 'nav[aria-label="主视图"] button:nth-of-type(1)',
      digest: 'nav[aria-label="主视图"] button:nth-of-type(2)',
      chat: 'nav[aria-label="主视图"] button:nth-of-type(3)',
    };
    const p95 = (samples) => {
      const sorted = samples.slice().sort((a, b) => a - b);
      return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(0.95 * sorted.length) - 1))];
    };
    async function clickTo(sel, visible) {
      const el = document.querySelector(sel);
      if (!(el instanceof HTMLElement)) throw new Error('missing ' + sel);
      const t0 = performance.now();
      el.click();
      await new Promise((resolve, reject) => {
        const deadline = t0 + 20000;
        const tick = () => {
          if (document.querySelector(visible)) resolve();
          else if (performance.now() > deadline) reject(new Error('timeout ' + visible));
          else requestAnimationFrame(tick);
        };
        tick();
      });
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      return performance.now() - t0;
    }
    async function sample(prepare, sel, visible) {
      for (let i = 0; i < expect.warmup; i++) {
        await prepare();
        await clickTo(sel, visible);
      }
      const samples = [];
      for (let i = 0; i < expect.samples; i++) {
        await prepare();
        samples.push(await clickTo(sel, visible));
      }
      if (samples.length !== expect.samples) throw new Error('sample count ' + samples.length);
      return Math.round(p95(samples) * 10) / 10;
    }
    const legs = {
      focusToDigest: [() => clickTo(nav.focus, '.focus'), nav.digest, '.digest'],
      digestToChat: [() => clickTo(nav.digest, '.digest'), nav.chat, '.chat-pane'],
      chatToFocus: [() => clickTo(nav.chat, '.chat-pane'), nav.focus, '.focus'],
    };
    const chosen = legs[expect.leg];
    if (!chosen) throw new Error('unknown switch leg ' + expect.leg);
    const value = await sample(chosen[0], chosen[1], chosen[2]);
    return 'rrss-switch-smoke-ok ' + expect.leg + '=' + value + ' n=' + expect.samples + ' targetMs=' + expect.interactionP95Ms + ' jsWork=unmeasured heap=unmeasured';
  })()`;
}

function walkProbeScript() {
  return `(async () => {
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const listen = window.__TAURI__.event.listen;
    const title = '步行检索文章';
    async function waitFor(label, read, budgetMs) {
      const deadline = performance.now() + budgetMs;
      let found = null;
      while (performance.now() < deadline) {
        found = read();
        if (found) return found;
        await sleep(40);
      }
      throw new Error(label);
    }
    const key = (name, extra) => window.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, ...extra }));
    const card = await waitFor('Walk article missing', () => document.querySelector('article[aria-label="' + title + '"]'), 8000);
    card.click();
    await waitFor('Reader did not open', () => document.querySelector('.drawer.reader'), 8000);
    key('k', { ctrlKey: true });
    const input = await waitFor('Search did not open', () => document.querySelector('.command-palette-input'), 4000);
    const proto = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value');
    proto.set.call(input, '步行检索');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await waitFor('Search missed the article', () => {
      const hit = document.querySelector('.command-palette-title');
      return hit && hit.textContent.includes(title) ? hit : null;
    }, 8000);
    key('Escape');
    await waitFor('Escape did not close search', () => document.querySelector('.command-palette') ? null : document.querySelector('.drawer.reader'), 4000);
    key('Escape');
    await waitFor('Escape did not close the reader', () => document.querySelector('.drawer.reader') ? null : document.querySelector('.focus'), 4000);
    const same = document.querySelector('article[aria-label="' + title + '"]');
    if (!same) throw new Error('Focus card missing after close');
    let mutations = 0;
    const observer = new MutationObserver(() => { mutations += 1; });
    observer.observe(document.querySelector('.focus'), { subtree: true, childList: true, characterData: true });
    let done = false;
    let error = '';
    const unDone = await listen('chat:done', () => { done = true; });
    const unError = await listen('chat:error', (event) => { error = (event.payload && event.payload.message) || String(event.payload); });
    try {
      await invoke('send_message', { args: { threadId: 0, text: '持续生成', citations: [] } });
      const deadline = performance.now() + 20000;
      while (!done && performance.now() < deadline) await sleep(40);
      if (!done) throw new Error(error || 'Stream did not settle');
      if (error) throw new Error(error);
      if (!same.isConnected) throw new Error('Focus card was replaced during stream');
      if (mutations !== 0) throw new Error('Focus mutated ' + mutations + ' times during stream');
      return 'rrss-walk-smoke-ok open search escape focusMutations=0';
    } finally {
      observer.disconnect();
      unDone();
      unError();
      await invoke('stop_pi', {}).catch(() => {});
    }
  })()`;
}

const listOnly = process.argv.includes("--list-tools");
const child = spawn("npx", ["-y", "@hypothesi/tauri-mcp-server@0.13.0"], {
  stdio: ["pipe", "pipe", "pipe"],
  detached: process.platform !== "win32",
});
const pending = new Map();
let id = 0;
let stderr = "";
child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-8000); });
function failAll(error) {
  for (const { reject, timer } of pending.values()) {
    clearTimeout(timer);
    reject(error);
  }
  pending.clear();
}
child.on("error", failAll);
child.on("exit", (code) => failAll(new Error(`MCP server exited ${code}: ${stderr}`)));
const lines = createInterface({ input: child.stdout });
lines.on("line", (line) => {
  let message;
  try { message = JSON.parse(line); } catch { return; }
  const call = pending.get(message.id);
  if (!call) return;
  pending.delete(message.id);
  clearTimeout(call.timer);
  if (message.error) call.reject(new Error(JSON.stringify(message.error)));
  else call.resolve(message.result);
});
function request(method, params, timeoutMs = 90000) {
  const requestId = ++id;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(requestId);
      reject(new Error(`${method} timed out: ${stderr}`));
    }, timeoutMs);
    pending.set(requestId, { resolve, reject, timer });
    child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: requestId, method, params }) + "\n");
  });
}
async function tool(name, args, timeoutMs = 90000) {
  const result = await request("tools/call", { name, arguments: args }, timeoutMs);
  if (result.isError) throw new Error(`${name}: ${JSON.stringify(result.content)}`);
  console.log(`${name}: ${JSON.stringify(result.content).slice(0, 2500)}`);
  return result;
}
async function toolQuiet(name, args, timeoutMs = 90000) {
  const result = await request("tools/call", { name, arguments: args }, timeoutMs);
  if (result.isError) {
    const brief = JSON.stringify(result.content).replace(/sk-[A-Za-z0-9_-]+/g, "[redacted]").slice(0, 500);
    throw new Error(`${name}: ${brief}`);
  }
  return result;
}
try {
  const init = await request("initialize", {
    protocolVersion: "2024-11-05", capabilities: {},
    clientInfo: { name: "rrss-debug-probe", version: "0.1.0" },
  });
  console.log(`MCP initialized: ${JSON.stringify(init.serverInfo)}`);
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
  const { tools } = await request("tools/list", {});
  const wanted = ["driver_session", "ipc_get_backend_state", "webview_execute_js"];
  for (const name of wanted) {
    const descriptor = tools.find((item) => item.name === name);
    if (!descriptor) throw new Error(`Missing MCP tool: ${name}`);
    if (listOnly) console.log(JSON.stringify(descriptor));
  }
  if (!listOnly) {
    await tool("driver_session", { action: "start", host: "127.0.0.1", port: 9223 });
    await tool("ipc_get_backend_state", {});
    const expectedDb = process.env.RRSS_EXPECTED_DB || "";
    const inspected = await tool("webview_execute_js", { script: `(async () => {
      if (!window.__TAURI_INTERNALS__ || !document.querySelector('.shell')) throw new Error('RRSS UI missing');
      if (${process.env.RRSS_EXPECT_DEV === "1"} && !document.querySelector('script[src*="/@vite/client"]')) throw new Error('Vite development client missing');
      const diag = await window.__TAURI_INTERNALS__.invoke('diagnostics', {});
      const expected = ${JSON.stringify(expectedDb)};
      if (expected && !diag.db.includes(expected)) throw new Error('MCP connected to the wrong database');
      return 'rrss-mcp-probe-ok';
    })()` });
    if (!JSON.stringify(inspected.content).includes("rrss-mcp-probe-ok")) {
      throw new Error("MCP did not return the expected live UI result");
    }
    if (process.env.RRSS_TEST_MEDIA === "1") {
      const expect = JSON.parse(process.env.RRSS_MEDIA_EXPECT || "{}");
      for (const key of ["articleId", "mediaId", "src", "width", "height"]) {
        if (expect[key] == null || expect[key] === "") throw new Error(`Media fixture missing ${key}`);
      }
      const mediaCheck = await tool("webview_execute_js", { script: mediaProbeScript(expect) });
      if (!JSON.stringify(mediaCheck.content).includes("rrss-media-smoke-ok")) {
        throw new Error("Media protocol smoke did not return its success marker");
      }
      console.log("Media protocol + decode passed");
    }
    if (process.env.RRSS_TEST_STREAM === "1") {
      const expect = JSON.parse(process.env.RRSS_STREAM_EXPECT || "{}");
      if (!Number.isInteger(expect.snapshots) || expect.snapshots < 1) throw new Error("Stream fixture missing snapshots");
      const streamCheck = await tool("webview_execute_js", { script: streamProbeScript(expect) }, 150000);
      if (!JSON.stringify(streamCheck.content).includes("rrss-stream-smoke-ok")) {
        throw new Error("Stream smoke did not return its success marker");
      }
      console.log("Minimal stream snapshots passed");
    }
    if (process.env.RRSS_TEST_SWITCH === "1") {
      const expect = JSON.parse(process.env.RRSS_SWITCH_EXPECT || "{}");
      if (!Number.isInteger(expect.samples) || expect.samples < 1) throw new Error("Switch fixture missing samples");
      const legs = [];
      for (const leg of ["focusToDigest", "digestToChat", "chatToFocus"]) {
        const switchCheck = await tool("webview_execute_js", { script: switchProbeScript({ ...expect, leg }) }, 120000);
        const text = JSON.stringify(switchCheck.content);
        if (!text.includes("rrss-switch-smoke-ok")) throw new Error("Switch smoke did not return its success marker");
        const marker = text.match(new RegExp(leg + "=[0-9.]+"));
        if (!marker) throw new Error("Switch smoke missing " + leg);
        legs.push(marker[0]);
      }
      console.log(`Minimal hot view switch reported ${legs.join(" ")} n=${expect.samples} jsWork=unmeasured heap=unmeasured`);
    }
    if (process.env.RRSS_TEST_WALK === "1") {
      const walkCheck = await tool("webview_execute_js", { script: walkProbeScript() }, 90000);
      if (!JSON.stringify(walkCheck.content).includes("rrss-walk-smoke-ok")) {
        throw new Error("Walk smoke did not return its success marker");
      }
      console.log("R1/R2 real-host walk passed");
    }
    if (process.env.RRSS_TEST_READER === "1") {
      if (!expectedDb) throw new Error("Reader checks require an isolated database path");
      const readerCheck = await tool("webview_execute_js", { script: `(async () => {
        const invoke = window.__TAURI_INTERNALS__.invoke;
        await invoke('set_list_scope', { scope: 'all' });
        await invoke('set_focus_filter', { filter: 'all' });
        await invoke('set_setting', { id: 'cluster_group_enabled', value: '0' });
        async function assertRead(articleId, expectedRead) {
          const page = await invoke('focus_stream', {});
          const article = page.articles.find(row => row.id === articleId);
          if (!article || article.read !== expectedRead) {
            throw new Error('Unexpected read state for ' + articleId + ': expected ' + expectedRead + ', got ' + (article ? article.read : 'missing row'));
          }
        }
        await assertRead(81, false);
        const snapshot = await invoke('article_reader', { id: 81 });
        if (snapshot.id !== 81 || !snapshot.document) throw new Error('Missing reader snapshot');
        await assertRead(81, false);
        const opened = await invoke('open_article', { id: 81 });
        if (opened.id !== 81) throw new Error('Wrong opened article');
        await assertRead(81, true);
        await invoke('mark_read', { id: 81, read: false });
        await invoke('article_reader', { id: 81 });
        await assertRead(81, false);
        await invoke('article_reader', { id: 82 });
        let rejected = false;
        try { await invoke('open_article', { id: 82 }); }
        catch (error) { rejected = String(error).includes('reader-write-rejected'); }
        if (!rejected) throw new Error('Open swallowed a failed read write');
        await assertRead(82, false);
        for (const command of ['article_reader', 'open_article']) {
          let missingRejected = false;
          try { await invoke(command, { id: 999999 }); }
          catch { missingRejected = true; }
          if (!missingRejected) throw new Error(command + ' accepted a missing article');
        }
        return 'rrss-reader-smoke-ok';
      })()` });
      if (!JSON.stringify(readerCheck.content).includes("rrss-reader-smoke-ok")) {
        throw new Error("Reader IPC smoke did not return its success marker");
      }
      console.log("Reader query/action real IPC checks passed");
    }
    if (process.env.RRSS_TEST_SETTINGS === "1") {
      if (!expectedDb) throw new Error("Settings checks require an isolated database path");
      const settingsCheck = await tool("webview_execute_js", { script: `(async () => {
        const invoke = window.__TAURI_INTERNALS__.invoke;
        const provider = (page) => page.providers.find(row => row.id === 91);
        const before = await invoke('ai_page', { selectedId: 91 });
        if (!provider(before) || provider(before).key !== 'set' || 'api_key' in provider(before)) throw new Error('Seeded connection or secret projection is invalid');
        const saved = await invoke('save_ai_settings', { args: {
          provider: { id: 91, name: ' 改名 ', base_url: 'http://127.0.0.1:1/v2', api_key: '', protocol: 'chat' },
        } });
        if (saved.selected_id !== 91 || provider(saved).name !== '改名' || provider(saved).base_url !== 'http://127.0.0.1:1/v2') throw new Error('Returned snapshot does not reflect the save');
        if (provider(saved).key !== 'set' || provider(saved).key_hint !== provider(before).key_hint || 'api_key' in provider(saved)) throw new Error('Blank key input changed the saved secret projection');
        if (JSON.stringify(await invoke('ai_page', { selectedId: 91 })) !== JSON.stringify(saved)) throw new Error('Connection snapshot differs from an independent query');
        let rejected = false;
        try {
          await invoke('save_ai_settings', { args: {
            provider: { id: 91, name: '不该落库', base_url: 'http://127.0.0.1:1/v3', api_key: 'sk-new', protocol: 'chat', compat_json: '{}' },
          } });
        } catch (error) { rejected = String(error).includes('ai-write-rejected'); }
        if (!rejected) throw new Error('Rejected connection write did not surface as an IPC error');
        const after = await invoke('ai_page', { selectedId: 91 });
        if (JSON.stringify(after) !== JSON.stringify(saved)) throw new Error('Failed connection save left partial writes behind');
        let missing = false;
        try { await invoke('save_ai_settings', { args: { provider: { id: 9999, name: 'x', base_url: 'http://127.0.0.1:1', api_key: null, protocol: 'chat' } } }); }
        catch { missing = true; }
        if (!missing) throw new Error('Save accepted a missing connection');

        // 用途与普通选项各走自己的写入契约；不再依赖旧连接页的复合字段。
        await invoke('save_model_pool', { args: { add: [{ provider_id: 91, model: 'mini' }], remove: [], replace: null } });
        const bindings = await invoke('save_model_bindings', { args: { bindings: [{ task: 'focus_summary', provider_id: 91, model: 'mini' }] } });
        if (!bindings.tasks.some(row => row.task === 'focus_summary' && row.provider_id === 91 && row.model === 'mini')) throw new Error('Model binding was not saved');
        if (JSON.stringify(await invoke('model_bindings', {})) !== JSON.stringify(bindings)) throw new Error('Model bindings differ from an independent query');
        await invoke('set_setting', { id: 'ai_daily_budget_tokens', value: '1234' });
        await invoke('set_setting', { id: 'focus_summary_neutral_limit', value: '2' });
        const schema = await invoke('settings_schema', {});
        const value = id => schema.values.find(row => row.id === id)?.value;
        if (value('ai_daily_budget_tokens') !== '1234' || value('focus_summary_neutral_limit') !== '2') throw new Error('Registered settings did not persist');

        const feedOf = (tree, id) => [...tree.uncategorized, ...tree.folders.flatMap(f => f.feeds)].find(f => f.id === id);
        const moved = await invoke('save_feed', { args: { id: 92, title: ' 自定义名 ', url: 'https://example.invalid/settings-b', folder_id: 93 } });
        const row = feedOf(moved, 92);
        if (!row || row.title !== '自定义名' || row.url !== 'https://example.invalid/settings-b' || row.folder_id !== 93) throw new Error('Feed save did not apply all fields');
        if (JSON.stringify(await invoke('feed_tree', {})) !== JSON.stringify(moved)) throw new Error('Feed tree differs from an independent query');
        let duplicate = false;
        try { await invoke('save_feed', { args: { id: 92, title: '不该落库', url: 'https://example.invalid/settings-taken', folder_id: 0 } }); }
        catch (error) { duplicate = String(error).includes('already subscribed'); }
        if (!duplicate) throw new Error('Duplicate URL was not rejected');
        const unchanged = feedOf(await invoke('feed_tree', {}), 92);
        if (unchanged.title !== '自定义名' || unchanged.folder_id !== 93) throw new Error('Failed feed save left partial writes behind');

        // 复合保存只剩路径；普通选项走注册表，未知设置名必须拒绝。
        const pi = await invoke('save_pi_settings', { args: { pi_path: ' /bin/false ' } });
        if (pi.pi_path !== '/bin/false') throw new Error('Pi path save did not apply');
        let badLevel = false;
        try { await invoke('set_setting', { id: 'pi_thinking_level', value: 'ultra' }); }
        catch { badLevel = true; }
        if (!badLevel) throw new Error('Unknown thinking level was accepted');
        const piAfter = await invoke('pi_page', {});
        if (piAfter.pi_path !== '/bin/false') throw new Error('Pi path did not persist');

        const latest = await invoke('messages', { threadId: 95, beforeId: null, limit: null });
        if (latest.messages.length !== 40 || latest.has_more !== true) throw new Error('Latest message page is not the bounded window');
        if (latest.messages[0].id !== 6 || latest.messages[39].id !== 45) throw new Error('Latest page is not the newest ids');
        const older = await invoke('messages', { threadId: 95, beforeId: latest.messages[0].id, limit: null });
        if (older.has_more !== false || older.messages.map(row => row.id).join(',') !== '1,2,3,4,5') throw new Error('Older page is not contiguous');
        const titled = (await invoke('threads', {})).find(row => row.id === 95);
        if (!titled || titled.title !== 'm01') throw new Error('Thread title did not come from the first message');

        const summaries = await invoke('digest_list', { period: 'daily' });
        const summary = summaries.find(row => row.id === 96);
        if (!summary || 'body' in summary || 'document' in summary) throw new Error('Digest list still carries the document');
        const full = await invoke('digest', { id: 96 });
        if (full.body !== '冒烟简报正文' || !full.document) throw new Error('Current digest did not load its document');
        return 'rrss-settings-smoke-ok';
      })()` });
      if (!JSON.stringify(settingsCheck.content).includes("rrss-settings-smoke-ok")) {
        throw new Error("Settings IPC smoke did not return its success marker");
      }
      console.log("Connection / bindings / registered settings / feed / Pi real IPC checks passed");
    }
    if (process.env.RRSS_TEST_DIGEST === "1") {
      const digestCheck = await tool("webview_execute_js", { script: `(async () => {
        const invoke = window.__TAURI_INTERNALS__.invoke;
        const listen = window.__TAURI__.event.listen;
        const empty = await invoke('digest_thread', { digestId: 71 });
        if (empty.thread_id !== 0 || empty.messages.length) throw new Error('Reading digest created a thread');
        const digest = await invoke('digest', { id: 71 });
        if (!digest.revision || digest.items[1].block_id !== 1) throw new Error('Digest revision/block contract missing');
        const citations = await invoke('cite_digest_selection', { digestId: 71, revision: digest.revision, selections: [{ block_id: 1, excerpt: '第二条测试文章' }] });
        if (citations.length !== 1 || citations[0].article_id !== 72 || citations[0].kind !== 'digest' || citations[0].block_id !== -1) throw new Error('Digest selection source mismatch');
        let threadId = 0;
        const snapshots = [];
        const unDelta = await listen('chat:delta', (event) => snapshots.push(event.payload));
        async function send(text, citations = []) {
          let done = false;
          const unDone = await listen('chat:done', () => { done = true; });
          try {
            const id = await invoke('send_message', { args: { threadId, text, citations, digestId: 71 } });
            for (let i = 0; i < 2000 && !done; i++) await new Promise(resolve => setTimeout(resolve, 10));
            if (!done) throw new Error('Digest reply never settled');
            if (threadId && id !== threadId) throw new Error('Thread was not reused');
            threadId = id;
            return (await invoke('messages', { threadId })).messages;
          } finally { unDone(); }
        }
        try {
          let rows = await send('展开第二条', citations);
          if (rows[0].citations.length !== 1 || rows[0].citations[0].excerpt !== '第二条测试文章') throw new Error('Selection snapshot did not survive message storage');
          if (rows.length !== 2 || rows[0].text !== '展开第二条' || !rows[1].text.includes('rrss://article/72')) throw new Error('Digest/source/user-message mismatch');
          if (rows[1].text.includes('工具结果不是助手回复')) throw new Error('Tool message was persisted as assistant');
          rows = await send('继续解释第二条');
          if (rows.length !== 4 || !rows[3].text.includes('第 2 轮')) throw new Error('Pi session file was not restored');
          let rejected = false;
          try { await invoke('send_message', { args: { threadId, text: '拒绝发送', citations: [], digestId: 71 } }); }
          catch (error) { rejected = String(error).includes('fixture rejects'); }
          if (!rejected) throw new Error('Prompt rejection was swallowed');
          if ((await invoke('messages', { threadId })).messages.length !== 4) throw new Error('Failed send left a duplicate user message');
          rows = await send('失败后重试第二条');
          if (rows.length !== 6 || !rows[5].text.includes('第 3 轮')) throw new Error('Runtime did not recover after rejection');
          const restored = await invoke('digest_thread', { digestId: 71 });
          const other = await invoke('digest_thread', { digestId: 72 });
          if (restored.thread_id !== threadId || other.thread_id !== 0) throw new Error('Digest thread leaked across issues');
          if ((await invoke('threads', {})).length) throw new Error('Digest follow-up leaked into deep chat list');
          if (!snapshots.length || snapshots.some((s) => s.thread_id !== threadId || !s.document)) throw new Error('Stream lacks thread identity or block IR');
          const reader = await invoke('article_reader', { id: 72 });
          if (reader.title !== '第二条测试文章') throw new Error('Wrong source article');
          return 'rrss-digest-smoke-ok';
        } finally {
          unDelta();
          await invoke('stop_pi', {});
        }
      })()` });
      if (!JSON.stringify(digestCheck.content).includes("rrss-digest-smoke-ok")) {
        throw new Error("Digest IPC smoke did not return its success marker");
      }
      console.log("Digest IPC + isolated Pi RPC fixture passed");
    }
    if (process.env.RRSS_TEST_PERF === "1") {
      const expect = JSON.parse(process.env.RRSS_PERF_EXPECT || "{}");
      const toolTimeout = expect.articles >= 2000 ? 600000 : expect.articles >= 400 ? 300000 : 180000;
      const perfCheck = await toolQuiet("webview_execute_js", { script: perfProbeScript(expect) }, toolTimeout);
      const perfText = (perfCheck.content || []).map((item) => item.text || "").join("\n");
      const perfLine = perfText.split("\n").find((line) => line.startsWith("rrss-perf-report "));
      if (!perfLine) throw new Error("Perf IPC smoke did not return its report");
      const report = finalizeHostReport(JSON.parse(perfLine.slice("rrss-perf-report ".length)), hostFacts());
      const reportPath = process.env.RRSS_PERF_REPORT;
      if (reportPath) writeFileSync(reportPath, JSON.stringify(report, null, 2) + "\n");
      console.log(perfConsoleLine(report));
      console.log("rrss-perf-smoke-ok");
    }
    await tool("driver_session", { action: "stop" });
    console.log("MCP live probe passed");
  }
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
} finally {
  child.stdin.end();
  lines.close();
  failAll(new Error("MCP probe closed"));
  try {
    if (process.platform === "win32") child.kill("SIGTERM");
    else process.kill(-child.pid, "SIGTERM");
  } catch { /* process already exited */ }
}
