import assert from "node:assert/strict";
import test from "node:test";
import { convertRules, xpathToCss } from "./convert-site-rules.mjs";

test("converts exact attributes, substring class, id, paths and union without broadening", () => {
  assert.equal(xpathToCss('//div[@class="x"]'), 'div[class="x" s]');
  assert.equal(xpathToCss('//input[@type="TEXT"]'), 'input[type="TEXT" s]');
  assert.equal(xpathToCss('//div[@class="x\ty"]'), 'div[class="x\\9 y" s]');
  assert.equal(xpathToCss("//div[contains(@class,'x')]"), 'div[class*="x" s]');
  assert.equal(xpathToCss('//*[@id="body"]//div/p | //article'), '*[id="body" s] div > p, article');
  assert.equal(xpathToCss('//div[@class="x y"][@id="z"]'), 'div[class="x y" s][id="z" s]');
  assert.equal(xpathToCss('//a[contains(@href,"a/b|c")]'), 'a[href*="a/b|c" s]');
  assert.equal(xpathToCss('//div[@id="a" or @id="b"]'), 'div:is(*[id="a" s], *[id="b" s])');
  assert.equal(xpathToCss('//div[@id="a" and @class="b"]'), 'div[id="a" s][class="b" s]');
});

test("rejects XPath with nonrepresentable semantics, including an unsupported union branch", () => {
  for (const xpath of ['//div[1]', '//div/text()', '//div | //p[.="x"]', 'id("body")',
    '//div/..', '//div[contains(@class,"")]', '//div[@id="x"']) {
    assert.throws(() => xpathToCss(xpath), undefined, xpath);
  }
});

test("counts skipped and ignored directives, normalizes domains and fixes field order", () => {
  const { data, stats } = convertRules([
    ["www.Example.com.txt", 'body: //article\ntitle: //h1\nstrip: //aside\nstrip_id_or_class: advert\nauthor: //span[1]\nnext_page_link: //a\nlogin_uri: /login\nhttp_header(User-Agent): ignored\nreplace_string(a:b): ignored'],
    [".example.com.txt", 'date: //time\nbody: id("content")\nstrip: //div/text()'],
    ["empty.test.txt", 'body: //div[1]'],
    ["global.txt", 'body: //div'],
  ], "abc123");
  assert.deepEqual(data.rules.map((r) => r.domain), ["*.example.com", "example.com"]);
  assert.deepEqual(Object.keys(data.rules[0]), ["domain", "body", "title", "author", "date", "strip"]);
  assert.deepEqual(data.rules[1].strip, ['aside', '[id*="advert"], [class*="advert"]']);
  assert.deepEqual(stats, {
    rules: 2, converted: 5, emptyRules: 1,
    skipped: { unsupported_root: 1, unsupported_node_or_axis: 1, unsupported_predicate: 2, non_domain_filename: 1 },
    ignored: { next_page_link: 1, login_uri: 1, http_header: 1, replace_string: 1 },
  });
  assert.equal(data.source, "fivefilters/ftr-site-config");
  assert.equal(data.commit, "abc123");
  assert.equal(data.rules[0].body, null);
});

test("conversion is deterministic with duplicate normalized domains", () => {
  const files = [["www.example.com.txt", "body: //main"], ["example.com.txt", "body: //article"]];
  const a = convertRules(files, "commit"), b = convertRules(files.toReversed(), "commit");
  assert.deepEqual(a, b);
  assert.equal(a.stats.skipped.duplicate_domain, 1);
  assert.equal(a.data.rules[0].body, "article");
});
