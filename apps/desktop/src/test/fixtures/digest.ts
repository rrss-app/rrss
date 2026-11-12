import type { DigestView } from "../../bindings";
import type { Block, Document } from "../../lib/blockIr";
import { PREVIEW_DIGESTS } from "../../lib/preview";

const p = (text: string): Block => ({ t: "p", inlines: [{ t: "t", text }] });
const h = (text: string): Block => ({ t: "h", level: 3, inlines: [{ t: "t", text }] });
const li = (marker: string, text: string): Block => ({ t: "li", indent: 0, marker, inlines: [{ t: "t", text }] });
const document: Document = { v: 1, blocks: [
  h("本期要点"), p("导读段落"), li("7.", "保留已有条目编号"), li("•", "无序条目按出现顺序编号"),
  p("两组列表之间的普通段落"), h("更多要点"), li("12.", "另一组已有编号"), li("•", "最后一个条目"),
  p("收尾第一段"), p("收尾第二段"),
] };

/** 同一正文用于日报 / 周报，另有无列表、无收尾和非段落打断收尾的边界。 */
export function digestSignatureFixtures(): DigestView[] {
  const base = PREVIEW_DIGESTS[0];
  const daily: DigestView = {
    ...base, id: 101, revision: "signature-daily", date_label: "10月3日",
    // UTC 仍是星期五；Asia/Shanghai 已是星期六，验证本地时区而非 UTC。
    period_start: Date.parse("2026-10-02T16:00:00Z"), body: "本期要点", document,
    source_state: "verified", source_note: "",
    items: [{ block_id: 2, sources: base.source_index }],
  };
  const weekly: DigestView = {
    ...daily, id: 201, revision: "signature-weekly", period: "weekly", period_label: "周报",
    period_start: Date.parse("2026-09-27T16:00:00Z"), date_label: "9月28日 – 10月4日",
  };
  const withoutSources = { items: [], source_index: [], unlocated_sources: [] };
  return [
    daily,
    { ...daily, ...withoutSources, id: 100, revision: "signature-no-closing", date_label: "10月2日", period_start: daily.period_start - 86_400_000,
      document: { v: 1, blocks: [h("无收尾"), p("导读"), li("•", "最后就是列表")] } },
    { ...daily, ...withoutSources, id: 99, revision: "signature-no-list", date_label: "10月1日", period_start: daily.period_start - 172_800_000,
      document: { v: 1, blocks: [h("无列表"), p("只有普通段落")] } },
    weekly,
    { ...weekly, ...withoutSources, id: 200, revision: "signature-interrupted", date_label: "9月21日 – 9月27日", period_start: weekly.period_start - 604_800_000,
      document: { v: 1, blocks: [h("非收尾段落"), li("•", "要点"), p("后面仍有正文"), h("另一节"), p("另一节的普通段落")] } },
  ];
}
