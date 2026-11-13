import { useEffect, useRef, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { feedQueryOptions } from "../lib/feedQuery";
import { memoryQueryOptions } from "./memoryQuery";
import { aiQueryOptions } from "./aiQuery";
import type { FeedTree, RankingSettings } from "../lib/api";
import type { RankingToggle } from "../components/FocusPane";
import { Icon, type IconName } from "../components/Icon";
import { useTranslation } from "../i18n";
import { DiagPane } from "./DiagPane";
import { MemoryPane } from "./MemoryPane";
import { ModelPane } from "./ModelPane";
import { AdvancedPane } from "./AdvancedPane";
import { AssistantPane } from "./AssistantPane";
import { RulesPane } from "./RulesPane";
import { StoragePane } from "./StoragePane";
import { storageQueryOptions } from "./storageQuery";
import { SubscribePane } from "./SubscribePane";
import { WebReadingPane } from "./WebReadingPane";
import { TranslatePane } from "./TranslatePane";
import type { SettingsEditState } from "./editState";

/**
 * 确认框：`okLabel` / `cancelLabel` 换按钮文字；`onCancel` 在点取消时调用（Esc 关闭不保证调用）；
 * `secondary` 是主按钮与取消之间的第三个选择。
 */
export type AskOptions = { okLabel?: string; cancelLabel?: string; onCancel?: () => void; secondary?: { label: string; onClick: () => void } };
export type Ask = (title: string, message: string, onOk: () => void, options?: AskOptions) => void;

export type SettingsTabId = "web" | "subscribe" | "rules" | "memory" | "model" | "translate" | "assistant" | "diagnostics" | "storage" | "advanced";
/** 切换分类；`anchor` 是目标页里分组的元素 id，切过去后滚到那里。 */
export type Navigate = (tab: SettingsTabId, anchor?: string) => void;

type SettingsPanelProps = {
  siteRuleSeed?: import("./SiteRuleDialog").SiteRuleSeed;
  onSiteRuleSeedUsed?: () => void;
  navigationHead?: ReactNode;
  navigationFoot?: ReactNode;
  tab: SettingsTabId;
  onTab: Navigate;
  ranking: RankingSettings;
  onRanking: (key: RankingToggle, value: boolean) => void | Promise<void>;
  /** 订阅、规则、兴趣或索引改了：首页列表需要重读；订阅改动带上新的订阅树。 */
  onChanged?: (tree?: FeedTree) => void;
  onAsk: Ask;
  onOpenArticle: (id: number) => void;
  onEditStateChange?: (state: SettingsEditState | null) => void;
};

/** 分类注册项：壳层只遍历注册表，各分类自行上报通用离开状态。 */
type SettingsTab = {
  id: SettingsTabId;
  icon: IconName;
  render: (props: SettingsPanelProps) => ReactNode;
};

const TABS: ReadonlyArray<SettingsTab> = [
  { id: "subscribe", icon: "rss",
    render: (p) => <SubscribePane onAsk={p.onAsk} onChanged={p.onChanged} onEditStateChange={p.onEditStateChange} /> },
  { id: "web", icon: "external",
    render: p => <WebReadingPane onAsk={p.onAsk} onEditStateChange={p.onEditStateChange} siteRuleSeed={p.siteRuleSeed} onSiteRuleSeedUsed={p.onSiteRuleSeedUsed} /> },
  { id: "rules", icon: "filter",
    render: (p) => <RulesPane onOpenArticle={p.onOpenArticle} onAsk={p.onAsk} onChanged={p.onChanged} onEditStateChange={p.onEditStateChange} /> },
  { id: "memory", icon: "memory",
    render: (p) => <MemoryPane ranking={p.ranking} onRanking={p.onRanking} onAsk={p.onAsk} onOpenArticle={p.onOpenArticle}
      onChanged={p.onChanged} onEditStateChange={p.onEditStateChange} /> },
  { id: "model", icon: "cpu",
    render: (p) => <ModelPane onAsk={p.onAsk} onNavigate={p.onTab} onEditStateChange={p.onEditStateChange} /> },
  { id: "assistant", icon: "chat",
    render: (p) => <AssistantPane onAsk={p.onAsk} onNavigate={p.onTab} onEditStateChange={p.onEditStateChange} /> },
  { id: "translate", icon: "translate",
    render: (p) => <TranslatePane onEditStateChange={p.onEditStateChange} onOpenModels={() => p.onTab("model")} /> },
  { id: "diagnostics", icon: "activity",
    render: (p) => <DiagPane onAsk={p.onAsk} onEditStateChange={p.onEditStateChange} onChanged={p.onChanged} /> },
  { id: "storage", icon: "database",
    render: (p) => <StoragePane onAsk={p.onAsk} onEditStateChange={p.onEditStateChange} onChanged={p.onChanged} /> },
  { id: "advanced", icon: "layers",
    render: (p) => <AdvancedPane onAsk={p.onAsk} onEditStateChange={p.onEditStateChange} /> },
];

export function SettingsPanel(props: SettingsPanelProps) {
  const { t } = useTranslation("settingsSystem");
  const { tab, onTab } = props;
  const feeds = useQuery(feedQueryOptions()).data;
  const memories = useQuery(memoryQueryOptions()).data;
  // 升级后自动摘要关闭的一次性提示还没看过：分类栏「模型」带一个点。
  const autoNotice = !!useQuery(aiQueryOptions()).data?.auto_summary.notice;
  const sources = feeds ? [...feeds.uncategorized, ...feeds.folders.flatMap((f) => f.feeds)] : [];
  const counts: Partial<Record<SettingsTabId, number>> = {
    subscribe: sources.filter((f) => f.health).length,
    memory: memories?.memories.filter((m) => m.pending).length ?? 0,
  };
  const body = useRef<HTMLDivElement>(null);
  const anchor = useRef<string | null>(null);
  // 离开守卫可能拦下切换，锚点只在真正切到目标页时使用。
  const goto: Navigate = (next, target) => { anchor.current = target ? `${next}:${target}` : null; onTab(next); };
  useEffect(() => {
    const [target, id] = anchor.current?.split(":") ?? [];
    anchor.current = null;
    const node = target === tab && id ? document.getElementById(id) : null;
    if (node) node.scrollIntoView({ block: "start" });
    else if (body.current) body.current.scrollTop = 0;
  }, [tab]);
  const current = TABS.find((item) => item.id === tab) ?? TABS[0];
  const client = useQueryClient();
  // 占用统计要扫库，不进启动预载；指到「存储」时就开始统计，点进去多半已经有结果，首屏不闪。
  const warm = (id: SettingsTabId) => { if (id === "storage" && id !== tab) void client.prefetchQuery(storageQueryOptions()); };
  return (
    <div className="settings-shell" data-scroll-boundary>
      <aside className="settings-sidebar">
        {props.navigationHead}
        <nav className="settings-nav" aria-label={t("panel.navAria")}>
          {TABS.map(({ id, icon }) => (
            <button
              key={id}
              type="button"
              title={t(`panel.tabs.${id}.title`)}
              aria-current={current.id === id ? "page" : undefined}
              data-on={current.id === id ? "1" : "0"}
              // 测试与冒烟脚本按分类 id 定位，改导航文字不必跟着改用例。
              data-settings-tab={id}
              className={id === "diagnostics" ? "settings-nav-second" : undefined}
              onPointerEnter={() => warm(id)}
              onFocus={() => warm(id)}
              onClick={() => goto(id)}
            >
              <Icon name={icon} size={16} />{t(`panel.tabs.${id}.label`)}
              {!!counts[id] && <span className={`settings-nav-count${id === "subscribe" ? " warn" : ""}`}>{counts[id]}</span>}
              {id === "model" && autoNotice && <span className="settings-nav-dot" aria-label={t("panel.newHint")} />}
            </button>
          ))}
        </nav>
        <footer className="settings-nav-foot">
          {props.navigationFoot}
          <p>{t("panel.dataLocal")}</p>
        </footer>
      </aside>
      <div className="settings-main">
        {/* 自绘标题栏：设置铺满窗口、没有固定顶栏，这条带子给三键让位，也能拖动窗口。 */}
        <div className="titlebar-band" data-tauri-drag-region="deep" />
        <div className="drawer-body settings-body" ref={body}>
          <section className="settings-panel-content" aria-labelledby="settings-page-title">
            <header className="settings-page-heading">
              <div>
                <h3 id="settings-page-title">{t(`panel.tabs.${current.id}.title`)}</h3>
                <p>{t(`panel.tabs.${current.id}.desc`)}</p>
              </div>
              {tab === "subscribe" && feeds && <span className="settings-page-summary">{t("panel.subscribeSummary", { sources: sources.length, folders: feeds.folders.length })}</span>}
            </header>
            {current.render({ ...props, onTab: goto })}
          </section>
        </div>
      </div>
    </div>
  );
}
