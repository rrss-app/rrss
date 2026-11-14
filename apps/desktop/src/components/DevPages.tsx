import { formatError } from "../lib/errors";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { BlockView, LIBRARY_FIXTURE } from "./BlockView";

export function ProbePage() {
  const [info, setInfo] = useState<string>("");
  const [ime, setIme] = useState("");
  useEffect(() => {
    api.probe().then((v) => setInfo(JSON.stringify(v, null, 2))).catch((e) => setInfo(formatError(e)));
  }, []);
  return (
    <div className="scroll reader" style={{ padding: 24 }}>
      <h1>WebView 探针</h1>
      <p>中文排版：新图书馆在市中心开放，开放时间 9–21 点。周末有故事会和时刻表。儿童区有矮书架 📚。</p>
      <p style={{ maxWidth: 280 }}>
        这段故意写得很长用来看中文换行是否按字边界切开而不是把「图书馆」从中间拆开到下一行。
      </p>
      <label>
        中文输入法
        <input value={ime} onChange={(e) => setIme(e.target.value)} placeholder="用拼音/五笔输入" />
      </label>
      <p>已输入：{ime || "（空）"}</p>
      <p className="embed">自定义协议图片（无缓存时显示占位）：</p>
      <img alt="protocol" src="rrss://localhost/media/1" style={{ maxWidth: 120 }} />
      <pre>{info}</pre>
    </div>
  );
}

export function RenderFixture() {
  return (
    <div style={{ padding: 24, maxWidth: 720 }}>
      <BlockView document={LIBRARY_FIXTURE} />
    </div>
  );
}
