import { expect, it } from "vitest";
import { joinCommand, parseCommand } from "./mcpCommand";

it("preserves stored argv when commands are shown and edited on either platform", () => {
  for (const parts of [
    ["node", "-e", `console.log("it's ok")`],
    [String.raw`C:\Program Files\runner.exe`, "C:\\notes\\", "", `a'b\"c`],
    ["runner", "line\nbreak", "a\tb", "中文引号 ' 和 \"", "'", '"'],
  ]) {
    expect(parseCommand(joinCommand(parts))).toEqual({ parts, complete: true });
  }
});

it("flags incomplete quotes without treating Windows backslashes as escapes", () => {
  expect(parseCommand('runner "unfinished').complete).toBe(false);
  expect(parseCommand("runner 'unfinished").complete).toBe(false);
  expect(parseCommand('"C:\\Program Files\\runner.exe" "C:\\notes\\"')).toEqual({
    parts: [String.raw`C:\Program Files\runner.exe`, "C:\\notes\\"], complete: true,
  });
});
