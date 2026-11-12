import { describe, expect, it } from "vitest";
import { contextUsageFixture, digestThreadFixture, threadFixture } from "./chat";
import { poolEntryFixture } from "./settings";

describe("chat view factories", () => {
  it("supplies compatible defaults and independent snapshots", () => {
    expect(threadFixture()).toMatchObject({chat_model:null,chat_thinking:null,context:null});
    expect(digestThreadFixture()).toMatchObject({thread_id:0,chat_thinking:null,context:null,messages:[]});
    expect(contextUsageFixture()).toEqual({used:0,window:null,at:0});
    expect(poolEntryFixture()).toMatchObject({provider_id:1,thinking:"off"});
    const first = digestThreadFixture();
    first.messages.push({id:1,role:"user",text:"测试",streaming:false,cite_article_id:0,cite_title:"",citations:[],document:null});
    expect(digestThreadFixture().messages).toEqual([]);
    const context = contextUsageFixture({used:123,window:32000,at:456});
    expect(threadFixture({context}).context).toEqual(context);
  });
});
