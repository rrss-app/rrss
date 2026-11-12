import { expect, it } from "vitest";
import { localArticleId } from "./ChatMessages";

it("recognizes only explicit positive safe-integer local article links", () => {
  expect(localArticleId("rrss://article/72")).toBe(72);
  for (const href of [
    "javascript:alert(1)", "https://article/72", "rrss://article/0", "rrss://article/-1",
    "rrss://article/1.5", "rrss://article/1?other=2", "rrss://article/9007199254740992",
    "rrss://article.evil/72", "rrss://article/72/extra",
  ]) expect(localArticleId(href), href).toBeNull();
});
