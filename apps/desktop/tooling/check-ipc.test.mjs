import assert from "node:assert/strict";
import { test } from "node:test";
import { checkChatEvents, checkIpc, commandRegistry, frontendCalls, mockedCommands, registered, rustCommands } from "./check-ipc.mjs";

test("chat context event has a listener and a host forwarder", () => {
  const api = 'listen<ChatContextEvent>("chat:context", cb)';
  const host = 'self.app.emit("chat:context", context)';
  assert.deepEqual(checkChatEvents(api,host), []);
  assert.equal(checkChatEvents(api,"").length,1);
  assert.equal(checkChatEvents("",host).length,1);
});

const RUST = `
#[tauri::command]
pub async fn save_rule(state: State<'_, AppState>, rule_id: i64, spec: NewRuleSpec, note: Option<String>) -> Result<(), String> {
    todo!()
}
db_command!(rules_page(filter: String) -> RulesPage => |conn| queries::rules_page(conn, &filter));
db_task!(
    /// 说明
    unsubscribe_feeds(ids: Vec<i64>) -> FeedTree =>
        |conn| actions::unsubscribe_many(conn, &ids)
);
`;
const MODULE = `${RUST}\ncommand_handler![save_rule, rules_page, unsubscribe_feeds,];`;

function errorsFor(api, mock = "") {
  return checkIpc({
    commands: rustCommands(RUST), handlers: new Set(registered(MODULE)), calls: frontendCalls(api),
    mocks: [{ file: "mock.ts", names: mockedCommands(mock) }],
  });
}

test("macro and handwritten commands parse with camelCase args, injected state skipped", () => {
  const commands = rustCommands(RUST);
  assert.deepEqual(commands.get("save_rule"), [
    { key: "ruleId", optional: false }, { key: "spec", optional: false }, { key: "note", optional: true },
  ]);
  assert.deepEqual(commands.get("unsubscribe_feeds"), [{ key: "ids", optional: false }]);
  assert.deepEqual(errorsFor(`invoke("save_rule", { ruleId, spec: { a: 1, b: 2 } }); invoke<RulesPage>("rules_page", { filter })`), []);
});

test("module registry connects only assembled modules and keeps argument contracts", () => {
  const registry = commandRegistry(new Map([
    ["mod.rs", "command_modules! { pub mod rules; mod feeds; }"],
    ["rules.rs", MODULE],
    ["feeds.rs", "db_task!(feed_tree() -> FeedTree => queries::feed_tree); command_handler![feed_tree];"],
  ]));
  assert.deepEqual(registry.errors, []);
  assert.deepEqual([...registry.handlers], ["save_rule", "rules_page", "unsubscribe_feeds", "feed_tree"]);
  assert.deepEqual(checkIpc({
    ...registry, calls: frontendCalls('invoke("save_rule", { ruleId, spec })'), mocks: [],
  }), []);
  assert.deepEqual(checkIpc({
    ...registry, calls: frontendCalls('invoke("save_rule", { rule_id, spec })'), mocks: [],
  }), [
    "api.ts:1 save_rule：多出参数 rule_id（Rust 参数：ruleId, spec, note）",
    "api.ts:1 save_rule：缺少必填参数 ruleId",
  ]);
});

test("missing modules, missing lists, wrong-module names, duplicates and detached commands fail", () => {
  const registry = commandRegistry(new Map([
    ["mod.rs", "command_modules! { pub mod rules; mod feeds; mod missing; mod unlisted; }"],
    ["rules.rs", MODULE.replace("unsubscribe_feeds,];", "save_rule, feed_tree,];")],
    ["feeds.rs", "db_task!(feed_tree() -> FeedTree => queries::feed_tree); command_handler![feed_tree];"],
    ["unlisted.rs", "db_command!(forgotten() -> () => queries::forgotten);"],
    ["detached.rs", "db_command!(detached() -> () => queries::detached); command_handler![detached];"],
  ]));
  assert.deepEqual(registry.errors, [
    "命令 save_rule 重复登记",
    "rules.rs 登记了 feed_tree，但本模块找不到定义",
    "rules.rs 定义的 unsubscribe_feeds 未登记",
    "命令 feed_tree 重复登记",
    "command_modules! 的 missing.rs 不存在",
    "unlisted.rs: command_handler! not found",
    "detached.rs 有命令定义，但未接入 command_modules!",
  ]);
  assert.throws(() => commandRegistry(new Map()), /command_modules! not found/);
});

test("deleting a module command makes both API calls and stale mocks fail", () => {
  const registry = commandRegistry(new Map([
    ["mod.rs", "command_modules! { mod rules; }"],
    ["rules.rs", "command_handler![];"],
  ]));
  assert.deepEqual(checkIpc({
    ...registry, calls: frontendCalls('invoke("save_rule", { ruleId, spec })'),
    mocks: [{ file: "mock.ts", names: mockedCommands('case "save_rule": case "plugin:dialog|open":') }],
  }), ["api.ts:1 save_rule：命令未注册", "mock.ts 模拟了不存在的命令 save_rule"]);
});

test("typos, missing args, unregistered commands and stale mocks fail", () => {
  const errors = errorsFor(
    `invoke("save_rule", { rule_id: 1, spec });\ninvoke("rules_page");\ninvoke("drop_rule", { id });`,
    `case "rules_page": case "input_tier":`,
  );
  assert.deepEqual(errors, [
    "api.ts:1 save_rule：多出参数 rule_id（Rust 参数：ruleId, spec, note）",
    "api.ts:1 save_rule：缺少必填参数 ruleId",
    "api.ts:2 rules_page：缺少必填参数 filter",
    "api.ts:3 drop_rule：命令未注册",
    "mock.ts 模拟了不存在的命令 input_tier",
  ]);
});
