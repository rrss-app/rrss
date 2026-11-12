import type { FeedView } from "../../lib/api";
import { Icon } from "../../components/Icon";
import { MenuButton, MenuCheck, MenuSelect } from "../../components/MenuSelect";
import { SCOPE_OPTIONS, TIME_OPTIONS, type SearchFilters } from "./searchFilters";

export type SearchKind = "articles" | "threads";

/**
 * 输入行下的筛选。能搜会话时先是「文章 / 对话」切换（带各自的条数）；范围、来源（多选）、时间三个下拉胶囊
 * 只对文章生效，切到对话时收起。改动立即重查。
 */
export function SearchFilterBar({ filters, feeds, onChange, kind, counts, onKind }: {
  filters: SearchFilters;
  feeds: FeedView[];
  onChange: (next: SearchFilters) => void;
  kind?: SearchKind;
  /** 标签上的条数；还没有结果时为空。 */
  counts?: { articles: string; threads: string };
  onKind?: (kind: SearchKind) => void;
}) {
  const known = filters.feedIds.filter((id) => feeds.some((feed) => feed.id === id));
  const feedLabel = known.length === 0 ? "全部"
    : known.length === 1 ? feeds.find((feed) => feed.id === known[0])!.title || "1 个来源"
    : `${known.length} 个来源`;
  const timeLabel = TIME_OPTIONS.find((option) => option.value === filters.time)!.label;
  const toggleFeed = (id: number) => {
    const next = known.includes(id) ? known.filter((x) => x !== id) : [...known, id];
    onChange({ ...filters, feedIds: next });
  };

  return (
    <div className="search-filters">
      {kind && onKind ? <>
        <div className="search-seg search-kind" role="group" aria-label="搜索什么">
          {([["articles", "文章"], ["threads", "对话"]] as const).map(([value, label]) => (
            <button key={value} type="button" aria-pressed={kind === value} onClick={() => onKind(value)}>
              {label}{counts?.[value] ? <small>{counts[value]}</small> : null}
            </button>
          ))}
        </div>
        {kind === "threads" ? <span className="search-kind-note">只搜没归档的会话，含标题和消息</span> : <span className="search-kind-sep" aria-hidden="true" />}
      </> : null}
      {kind === "threads" ? null : <>
      <MenuSelect label="范围" className="search-pill" menuClassName="search-menu" prefix="范围：" active={filters.scope !== "all"} value={filters.scope}
        options={SCOPE_OPTIONS} onChange={(scope) => onChange({ ...filters, scope })} />
      <MenuButton label="按来源筛选" className="search-pill" menuClassName="search-menu" active={known.length > 0}
        face={<><span className="search-pill-value">来源：{feedLabel}</span><Icon name="chevronDown" size={12} /></>}>
        {() => <>
          <MenuCheck checked={known.length === 0} onSelect={() => onChange({ ...filters, feedIds: [] })}>全部来源</MenuCheck>
          {feeds.length ? <hr /> : null}
          {feeds.map((feed) => (
            <MenuCheck key={feed.id} checked={known.includes(feed.id)} onSelect={() => toggleFeed(feed.id)}>{feed.title || feed.url}</MenuCheck>
          ))}
        </>}
      </MenuButton>
      <MenuSelect label="按发布时间筛选" className="search-pill" menuClassName="search-menu" prefix="时间：" active={filters.time !== "any"} value={filters.time}
        options={TIME_OPTIONS} onChange={(time) => onChange({ ...filters, time })} />
      </>}
    </div>
  );
}
