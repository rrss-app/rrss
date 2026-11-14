import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { i18n } from "../i18n";
import { isTauri } from "./preview";
import type { SelectedBlock } from "./selection";
import type {
  ClientLogLevel,
  LogStatusView,
  LogUnavailableReason,
  WebVerifyStateEvent,
  WebNavigated,
  WebClosed,
  WebEscape,
  WebDownloadBlocked,
  WebViewBounds,
  SiteRuleView,
  SiteRulesPage,
  SiteRuleTestView,
  OpenWebLinkView,
  LinkSafety,
  TranslationEvent,
  TranslationReply,
  TranslationStartMode,
  TranslationInitialViewport,
  TranslationHint,
  McpInput,
  McpProbeView,
  McpLoginResult,
  McpImportEntry,
  AddFeedResult,
  FeedUpdated,
  AiPage,
  GeneratedConfig,
  PiConfigView,
  SavePiConfigArgs,
  ArticleView,
  ClusterArticleView,
  ClusterNoteView,
  ClusterSummaryView,
  ChatMessageView,
  ChatStreamEvent,
  ChatErrorEvent,
  ChatStatusEvent,
  ChatStepView,
  ArticlePreviewView,
  ChatConfirmEvent,
  ChatContextEvent,
  ContextUsageView,
  SendMessageArgs,
  DigestThreadView,
  DigestSelection,
  CitationView,
  DiagnosticsPage,
  WindowChromeView,
  CloseChoice,
  TrayLabels,
  SettingsSchema,
  SettingSpec,
  SettingValue,
  StorageMessage,
  StoragePage,
  StorageGroupView,
  StorageItemView,
  RetentionMessage,
  RetentionPage,
  BackupPage,
  BackupEntryView,
  BackupDoneView,
  RestoreCheckView,
  RestoreNoticeView,
  PrefsImportView,
  DigestSummary,
  DigestView,
  FeatureView,
  FeedTree,
  FeedView,
  FolderView,
  TreeLayout,
  FocusPage,
  FeedsCursor,
  FeedsPage,
  FeedCountsView,
  HiddenPage,
  RuleStatsPage,
  InterestHitView,
  InterestView,
  MemoryPage,
  MemoryView,
  MessagesPage,
  NewRuleSpec,
  PiPage,
  PiProbe,
  PiInstallView,
  SkillDraft,
  PresetView,
  ProviderView,
  ProviderPreset,
  ModelBindingsView,
  VectorSpaceStatusView,
  ModelChoice,
  SaveModelBindingsArgs,
  ModelPoolView,
  ModelCatalogView,
  PoolEntryView,
  SaveModelPoolArgs,
  CapabilitySupport,
  CapsView,
  CapsOverrideView,
  KeyState,
  ProviderTestView,
  ConnectionTestArgs,
  ConnectionTestView,
  UpdatePoolModelArgs,
  RankingSettings,
  ReadingSpeedsView,
  ReaderView,
  RelatedArticleView,
  RefreshOperation,
  RefreshStarted,
  SchedulerSnapshot,
  SearchHitView,
  SearchQuery,
  SearchScope,
  SearchHitBlocksView,
  SearchBlockHitView,
  ReasonView,
  RuleView,
  RulesPage,
  SaveAiSettingsArgs,
  SaveFeedArgs,
  SavePiSettingsArgs,
  NetworkProxyMode,
  NetworkProxyRuleView,
  NetworkProxyView,
  NetworkProxyScope,
  NetworkProxyTestArgs,
  NetworkProxyTestView,
  SystemProxyView,
  SaveNetworkProxyRuleArgs,
  SaveNetworkProxyArgs,
  ScopeOption,
  TaskBindView,
  ThreadView,
  ThreadFolderView,
  ThreadListView,
  ThreadSearchHitView,
  MediaRef,
  MediaReadyEvent,
  EmbedCoverView,
  EmbedCoverEvent,
  TranslatePage,
  SelectionTranslation,
  ExposureInput,
  ArticleEventInput,
  ReadSessionInput,
} from "../bindings";

export type {
  ClusterSummaryView,
  ClusterNoteView,
  ClusterArticleView,
  AddFeedResult,
  FeedUpdated,
  AiPage,
  GeneratedConfig,
  PiConfigView,
  SavePiConfigArgs,
  ArticleView,
  ChatMessageView,
  ChatStreamEvent,
  ChatErrorEvent,
  ChatStatusEvent,
  ChatStepView,
  ArticlePreviewView,
  ChatConfirmEvent,
  ChatContextEvent,
  ContextUsageView,
  SendMessageArgs,
  DigestThreadView,
  DigestSelection,
  CitationView,
  DiagnosticsPage,
  LogStatusView,
  LogUnavailableReason,
  SettingsSchema,
  SettingSpec,
  SettingValue,
  StorageMessage,
  StoragePage,
  StorageGroupView,
  StorageItemView,
  RetentionMessage,
  RetentionPage,
  BackupPage,
  BackupEntryView,
  BackupDoneView,
  RestoreCheckView,
  RestoreNoticeView,
  PrefsImportView,
  DigestSummary,
  DigestView,
  FeatureView,
  FeedTree,
  FeedView,
  FolderView,
  TreeLayout,
  FocusPage,
  FeedsCursor,
  FeedsPage,
  FeedCountsView,
  HiddenPage,
  RuleStatsPage,
  InterestHitView,
  InterestView,
  MemoryPage,
  MemoryView,
  MessagesPage,
  NewRuleSpec,
  PiPage,
  PiProbe,
  PiInstallView,
  SkillDraft,
  PresetView,
  ProviderView,
  ProviderPreset,
  ModelBindingsView,
  ModelChoice,
  SaveModelBindingsArgs,
  ModelPoolView,
  ModelCatalogView,
  PoolEntryView,
  SaveModelPoolArgs,
  CapabilitySupport,
  CapsView,
  CapsOverrideView,
  KeyState,
  ProviderTestView,
  ConnectionTestArgs,
  ConnectionTestView,
  UpdatePoolModelArgs,
  RankingSettings,
  ReadingSpeedsView,
  ReaderView,
  RelatedArticleView,
  CloseChoice,
  TrayLabels,
  RefreshOperation,
  RefreshStarted,
  SchedulerSnapshot,
  SearchHitView,
  SearchQuery,
  SearchScope,
  SearchHitBlocksView,
  SearchBlockHitView,
  ReasonView,
  RuleView,
  RulesPage,
  SaveAiSettingsArgs,
  SaveFeedArgs,
  SavePiSettingsArgs,
  NetworkProxyMode,
  NetworkProxyRuleView,
  NetworkProxyView,
  NetworkProxyScope,
  NetworkProxyTestArgs,
  NetworkProxyTestView,
  SystemProxyView,
  SaveNetworkProxyRuleArgs,
  SaveNetworkProxyArgs,
  ScopeOption,
  TaskBindView,
  ThreadView,
  ThreadFolderView,
  ThreadListView,
  ThreadSearchHitView,
  MediaRef,
  TranslatePage,
};

export const api = {
  siteRulesPage: () => invoke<SiteRulesPage>("site_rules_page"),
  saveSiteRule: (rule: SiteRuleView, enabled: boolean) => invoke<void>("save_site_rule", { rule, enabled }),
  deleteSiteRule: (domain: string) => invoke<void>("delete_site_rule", { domain }),
  setSiteRuleEnabled: (domain: string, enabled: boolean) => invoke<void>("set_site_rule_enabled", { domain, enabled }),
  testSiteRule: (url: string, rule: SiteRuleView) => invoke<SiteRuleTestView>("test_site_rule", { url, rule }),
  extractWebView: (label: string) => invoke<WebVerifyStateEvent>("extract_web_view", { label }),
  openWebView: (url: string, bounds: WebViewBounds) => invoke<string>("open_web_view", { url, bounds, nativeFailureMessage: i18n.t("web_native_failed", { ns: "errors" }) }),
  closeWebView: (label: string) => invoke<void>("close_web_view", { label }),
  setWebViewBounds: (label: string, bounds: WebViewBounds) => invoke<void>("set_web_view_bounds", { label, bounds }),
  setWebViewVisible: (label: string, visible: boolean) => invoke<void>("set_web_view_visible", { label, visible }),
  webViewAction: (label: string, action: string) => invoke<void>("web_view_action", { label, action }),
  clearWebData: () => invoke<void>("clear_web_data"),
  openWebLink: (url: string) => invoke<OpenWebLinkView>("open_web_link", { url }),
  collectArticle: (id: number) => invoke<void>("collect_article", { id }),
  linkSafety: (href: string, linkText: string) => invoke<LinkSafety>("link_safety", { href, linkText }),
  focus: (
    expanded?: number | null,
    opts?: { more?: boolean; excludeIds?: number[]; excludeClusterIds?: number[]; scope?: string },
  ) => invoke<FocusPage>("focus_stream", {
    expandedCluster: expanded ?? null,
    more: opts?.more ?? false,
    excludeIds: opts?.excludeIds ?? null,
    excludeClusterIds: opts?.excludeClusterIds ?? null,
    ...(opts?.scope ? { scope: opts.scope } : {}),
  }),
  focusClusterMembers: (clusterId: number, scope?: string) =>
    invoke<ArticleView[]>("focus_cluster_members", { clusterId, ...(scope ? { scope } : {}) }),
  /** 相近报道组尾「全部标为已读」：这组在范围内可见的未读一次标完，返回改动的 id。 */
  markClusterRead: (clusterId: number, scope?: string) =>
    invoke<number[]>("mark_cluster_read", { clusterId, ...(scope ? { scope } : {}) }),
  /** 相近报道综述：存着的结果与新旧状态，不调用模型。 */
  clusterSummary: (clusterId: number, scope?: string) =>
    invoke<ClusterSummaryView>("cluster_summary", { clusterId, ...(scope ? { scope } : {}) }),
  /** 生成或更新相近报道综述（用摘要模型），返回最新状态。 */
  generateClusterSummary: (clusterId: number, scope?: string) =>
    invoke<ClusterSummaryView>("generate_cluster_summary", { clusterId, ...(scope ? { scope } : {}) }),
  feeds: () => invoke<FeedTree>("feed_tree"),
  feedsPage: (scope: string, listScope: string, cursor: FeedsCursor | null) =>
    invoke<FeedsPage>("feeds_page", { scope, listScope, cursor }),
  feedCounts: () => invoke<FeedCountsView>("feed_counts"),
  markScopeRead: (scope: string) => invoke<number[]>("mark_scope_read", { scope }),
  rules: (filter = "all") => invoke<RulesPage>("rules_page", { filter }),
  readingSpeeds: () => invoke<ReadingSpeedsView>("reading_speeds"),
  reader: (id: number) => invoke<ReaderView>("article_reader", { id }),
  relatedArticles: (articleId: number, limit: number) => invoke<RelatedArticleView[]>("related_articles", { articleId, limit }),
  openArticle: (id: number) => invoke<ReaderView>("open_article", { id }),
  searchThreads: (query: string, limit = 20) => invoke<ThreadSearchHitView[]>("search_threads", { query, limit }),
  threadList: () => invoke<ThreadListView>("thread_list"),
  archivedThreads: () => invoke<ThreadView[]>("archived_threads"),
  renameThread: (threadId: number, title: string) => invoke<void>("rename_thread", { threadId, title }),
  setThreadFolder: (threadId: number, folderId: number | null) => invoke<void>("set_thread_folder", { threadId, folderId }),
  setThreadPinned: (threadId: number, pinned: boolean) => invoke<void>("set_thread_pinned", { threadId, pinned }),
  archiveThread: (threadId: number) => invoke<void>("archive_thread", { threadId }),
  unarchiveThread: (threadId: number) => invoke<void>("unarchive_thread", { threadId }),
  deleteThread: (threadId: number) => invoke<void>("delete_thread", { threadId }),
  createThreadFolder: (name: string) => invoke<ThreadFolderView>("create_thread_folder", { name }),
  renameThreadFolder: (folderId: number, name: string) => invoke<void>("rename_thread_folder", { folderId, name }),
  deleteThreadFolder: (folderId: number) => invoke<void>("delete_thread_folder", { folderId }),
  reorderThreadFolders: (ids: number[]) => invoke<void>("reorder_thread_folders", { ids }),
  setThreadFolderCollapsed: (folderId: number, collapsed: boolean) => invoke<void>("set_thread_folder_collapsed", { folderId, collapsed }),
  /** 最新一页（`beforeId` 缺省）或 `beforeId` 之前的一页历史；条数由宿主决定。 */
  messages: (threadId: number, beforeId: number | null = null) =>
    invoke<MessagesPage>("messages", { threadId, beforeId }),
  memory: (query = "") => invoke<MemoryPage>("memory_page", { query }),
  ai: (selectedId = 0) => invoke<AiPage>("ai_page", { selectedId }),
  dismissAutoSummaryNotice: () => invoke<void>("dismiss_auto_summary_notice"),
  translatePage: () => invoke<TranslatePage>("translate_page"),
  saveTranslateSettings: (args: {
    enabled: boolean;
    backend: string;
    target_lang: string;
    deepl_key?: string | null;
    deepl_url?: string | null;
    deeplx_url?: string | null;
    deeplx_token?: string | null;
    http_url?: string | null;
    http_key?: string | null;
  }) => invoke("save_translate_settings", { args }),
  testTranslate: () => invoke<string>("test_translate"),
  translateSelection: (text: string, configIdentity: string) => invoke<SelectionTranslation>("translate_selection", { text, configIdentity }),
  digests: (period: string) => invoke<DigestSummary[]>("digest_list", { period }),
  digest: (id: number) => invoke<DigestView>("digest", { id }),
  /** 设置选项注册表：简单选项的描述符与当前值。 */
  settingsSchema: () => invoke<SettingsSchema>("settings_schema"),
  /** 按描述符校验后写入一项，返回规范化后的值；非法值抛出可展示的原因。 */
  setSetting: (id: string, value: string) => invoke<SettingValue>("set_setting", { id, value }),
  digestThread: (digestId: number) => invoke<DigestThreadView>("digest_thread", { digestId }),
  /** 本地文章的悬停预览（对话里的链接与引用小签）；只读，不标已读。 */
  articlePreviews: (ids: number[]) => invoke<ArticlePreviewView[]>("article_previews", { ids }),
  logClient: (level: ClientLogLevel, scope: string, message: string, stack?: string) => invoke<void>("log_client", { level, scope, message, stack }),
  diagnostics: () => invoke<DiagnosticsPage>("diagnostics"),
  /** 本次运行的文件日志级别；返回新的日志状态。 */
  logSetLevel: (detailed: boolean) => invoke<LogStatusView>("log_set_level", { detailed }),
  /** 弹出保存对话框后打包；返回保存位置，取消时为 null。 */
  logExport: () => invoke<string | null>("log_export"),
  /** 自绘标题栏：窗口三键按宿主给的平台样子画；没有宿主（浏览器预览）时为 null，不画三键。 */
  windowChrome: (): Promise<WindowChromeView | null> => isTauri() ? invoke<WindowChromeView>("window_chrome") : Promise.resolve(null),
  /** 关闭询问的结果：隐藏到托盘或退出程序；`remember` 时写进设置「关闭窗口时」。 */
  windowCloseChoice: (choice: CloseChoice, remember: boolean) => invoke<void>("window_close_choice", { choice, remember }),
  /** 建托盘或换菜单文字（文字由前端给）；没有宿主或托盘不可用时为 false。 */
  trayInstall: (labels: TrayLabels): Promise<boolean> => isTauri() ? invoke<boolean>("tray_install", { labels }) : Promise.resolve(false),
  storagePage: () => invoke<StoragePage>("storage_page"),
  /** 推荐评测日志（阶段 23 E0）；统一由 `lib/signals.ts` 批量调用，组件不要直接用。 */
  recordExposures: (records: ExposureInput[]) => invoke<Array<number | null>>("record_exposures", { records }),
  recordArticleEvents: (records: ArticleEventInput[]) => invoke<number>("record_article_events", { records }),
  recordReadSessions: (records: ReadSessionInput[]) => invoke<number>("record_read_sessions", { records }),
  storageClean: (action: string) => invoke<StorageMessage>("storage_clean", { action }),
  retentionPage: () => invoke<RetentionPage>("retention_page"),
  retentionSetDays: (days: number) => invoke<void>("retention_set_days", { days }),
  retentionCleanNow: () => invoke<RetentionMessage>("retention_clean_now"),
  backupPage: () => invoke<BackupPage>("backup_page"),
  /** 宿主弹保存对话框；取消返回 null。 */
  backupCreate: (includeSecrets = false) => invoke<BackupDoneView | null>("backup_create", { includeSecrets }),
  /** 不给路径时宿主弹打开对话框；取消返回 null。 */
  backupInspect: (path?: string) => invoke<RestoreCheckView | null>("backup_inspect", { path: path ?? null }),
  /** 成功时宿主会重启，不会返回。 */
  backupRestore: (path: string) => invoke<void>("backup_restore", { path }),
  backupCancelRestore: () => invoke<void>("backup_cancel_restore"),
  backupSetSchedule: (value: string) => invoke<void>("backup_set_schedule", { value }),
  backupDismissNotice: () => invoke<void>("backup_dismiss_notice"),
  backupReveal: (path: string) => invoke<void>("backup_reveal", { path }),
  prefsExport: (parts: { rules: boolean; interests: boolean; memories: boolean }) => invoke<string>("prefs_export", parts),
  prefsImport: (text: string) => invoke<PrefsImportView>("prefs_import", { text }),
  pi: () => invoke<PiPage>("pi_page"),
  piProbe: () => invoke<PiProbe>("pi_probe"),
  piConfig: () => invoke<PiConfigView>("pi_config"),
  networkProxy: () => invoke<NetworkProxyView>("network_proxy"),
  saveNetworkProxy: (args: SaveNetworkProxyArgs) => invoke<NetworkProxyView>("save_network_proxy", { args }),
  testNetworkProxy: (args: NetworkProxyTestArgs) => invoke<NetworkProxyTestView>("test_network_proxy", { args }),
  savePiConfig: (args: SavePiConfigArgs) => invoke<PiConfigView>("save_pi_config", { args }),
  /** 只读、脱敏的启动参数与生成文件；不运行 Pi。 */
  piGeneratedConfig: () => invoke<GeneratedConfig>("pi_generated_config"),
  /** 用户显式操作：可能启动 Pi 读取它的模型列表。 */
  piAvailableModels: () => invoke<string[]>("pi_available_models"),
  addFeed: async (url: string) => {
    if (isTauri() && !feedUpdates) throw new Error(i18n.t("api.feedUpdatesNotReady", { ns: "shell" }));
    await feedUpdates?.ready;
    const result = await invoke<AddFeedResult>("add_feed", { url });
    // Reconcile after acknowledgement too: completion can precede the command response.
    feedUpdates?.changed({ run_id: "", generation: null, status: "acknowledged", derived_degraded: false, feed_id: result.id, inserted: 0, error: null });
    return result;
  },
  saveFeed: (args: SaveFeedArgs) => invoke<FeedTree>("save_feed", { args }),
  createFolder: (name: string, parentFolderId?: number | null) =>
    invoke<number>("create_folder", { name, parentFolderId: parentFolderId ?? null }),
  renameFolder: (id: number, name: string) => invoke("rename_folder", { id, name }),
  moveTreeItems: (items: string[], parentFolderId: number | null, before: string | null) =>
    invoke<TreeLayout[]>("move_tree_items", { items, parentFolderId, before }),
  restoreTreeLayout: (layout: TreeLayout[]) => invoke<void>("restore_tree_layout", { layout }),
  deleteFolder: (id: number) => invoke("delete_folder", { id }),
  refresh: () => invoke<RefreshStarted>("refresh"),
  schedulerStatus: () => invoke<SchedulerSnapshot>("scheduler_status"),
  rate: (id: number, up: boolean) => invoke("rate_article", { id, up }),
  citeArticle: (id: number) => invoke<CitationView>("cite_article", { id }),
  citeSelection: (articleId: number, excerpt: string, blockId: number) =>
    invoke<CitationView>("cite_selection", { articleId, excerpt, blockId }),
  citeDigestSelection: (digestId: number, revision: string, blocks: SelectedBlock[]) => {
    const selections: DigestSelection[] = blocks.map((block) => ({ block_id: block.blockId, excerpt: block.excerpt }));
    return invoke<CitationView[]>("cite_digest_selection", { digestId, revision, selections });
  },
  markRead: (id: number, read: boolean) => invoke("mark_read", { id, read }),
  starArticle: (id: number, starred: boolean) => invoke("star_article", { id, starred }),
  unmarkRead: (ids: number[]) => invoke("unmark_read", { ids }),
  searchArticles: (query: SearchQuery) =>
    invoke<SearchHitView[]>("search_articles", { query }),
  searchHitBlocks: (articleId: number, query: string) =>
    invoke<SearchHitBlocksView>("search_hit_blocks", { articleId, query }),
  fetchFulltext: (id: number) => invoke<ReaderView>("fetch_fulltext", { id }),
  sendMessage: (threadId: number, text: string, citations: CitationView[], digestId: number | null = null, model: ModelChoice | null = null, thinking?: string | null) => {
    const args: SendMessageArgs = { threadId, text, citations, digestId, model, ...(thinking == null ? {} : { thinking }) };
    return invoke<number>("send_message", { args });
  },
  abort: () => invoke("abort_generation"),
  prefetch: (articleId: number) => invoke<MediaRef[]>("prefetch_images", { articleId }),
  retryImage: (articleId: number, url: string) => invoke<MediaRef>("retry_image", { articleId, url }),
  prefetchEmbeds: (articleId: number) => invoke<void>("prefetch_embed_covers", { articleId }),
  downloadDirect: (articleId: number, url: string) => invoke<MediaRef>("download_direct_media", { articleId, url }),
  ruleStats: () => invoke<RuleStatsPage>("rule_stats"),
  hidden: () => invoke<HiddenPage>("hidden_page"),
  hiddenInScope: (scope: string, listScope: string) =>
    invoke<HiddenPage>("hidden_page", { scope, listScope }),
  releaseArticle: (id: number) => invoke<void>("release_article", { id }),
  allowRuleForFeed: (ruleId: number, feedId: number) => invoke<number>("allow_rule_for_feed", { ruleId, feedId }),
  createRule: (spec: NewRuleSpec) => invoke<number>("create_rule", { spec }),
  updateRule: (id: number, spec: NewRuleSpec) => invoke("update_rule", { id, spec }),
  setRuleAction: (id: number, action: number) => invoke("set_rule_action", { id, action }),
  confirmRule: (id: number) => invoke("confirm_suggested_rule", { id }),
  discardRule: (id: number) => invoke("discard_rule", { id }),
  applyPreset: (id: string) => invoke("apply_preset", { id }),
  removePreset: (id: string) => invoke("remove_preset", { id }),
  rescore: () => invoke<number>("rescore"),
  addInterest: (label: string) => invoke<number>("add_interest", { label }),
  deleteInterest: (id: number) => invoke("delete_interest", { id }),
  setInterestEnabled: (id: number, enabled: boolean) => invoke("set_interest_enabled", { id, enabled }),
  interestHits: (id: number) => invoke<InterestHitView[]>("interest_hits", { id }),
  confirmMemory: (id: number) => invoke("confirm_memory", { id }),
  discardMemory: (id: number) => invoke("discard_memory", { id }),
  pinMemory: (id: number, pinned: boolean) => invoke("pin_memory", { id, pinned }),
  clearMemories: () => invoke("clear_memories"),
  exportMemories: () => invoke<string>("export_memories"),
  editMemory: (id: number, content: string) => invoke("edit_memory", { id, content }),
  memoryAsInterest: (id: number) => invoke<number>("memory_as_interest", { id }),
  generateDigest: (period: string) => invoke("generate_digest", { period }),
  summarize: (id: number) => invoke<string>("summarize_article", { id }),
  translate: (id: number, mode: TranslationStartMode = "default", initial: TranslationInitialViewport | null = null) => invoke<TranslationReply>("translate_article", { id, mode, initial }),
  translateSnapshot: (id: number, generation: number | null = null) => invoke<TranslationReply>("translate_snapshot", { id, generation }),
  translateAttach: (generation: number, ticket: number) => invoke<void>("translate_attach", { generation, ticket }),
  translateDetach: (generation: number, ticket: number) => invoke<void>("translate_detach", { generation, ticket }),
  translateHint: (hint: TranslationHint) => invoke<void>("translate_hint", { hint }),
  translateRetrySave: (id: number, generation: number) => invoke<TranslationReply>("translate_retry_save", { id, generation }),
  importOpml: (xml: string) => invoke<string>("import_opml", { xml }),
  exportOpml: () => invoke<string>("export_opml"),
  unsubscribe: (id: number) => invoke("unsubscribe", { id }),
  unsubscribeFeeds: (ids: number[]) => invoke<FeedTree>("unsubscribe_feeds", { ids }),
  dismissFeedHealth: (id: number) => invoke<FeedTree>("dismiss_feed_health", { id }),
  deleteProvider: (id: number) => invoke("delete_provider", { id }),
  saveAiSettings: (args: SaveAiSettingsArgs) => invoke<AiPage>("save_ai_settings", { args }),
  /** 「测试」连接：只请求一次模型列表；测的是已保存的地址与密钥时结果记到连接上。 */
  testConnection: (args: ConnectionTestArgs) => invoke<ConnectionTestView>("test_connection", { args }),
  providerPresets: () => invoke<ProviderPreset[]>("provider_presets"),
  cancelVectorRebuild: () => invoke<VectorSpaceStatusView>("cancel_vector_rebuild"),
  setVectorDims: (value: string) => invoke<VectorSpaceStatusView>("set_vector_dims", { value }),
  checkVectorSpace: () => invoke<VectorSpaceStatusView>("check_vector_space"),
  restoreVectorSpace: () => invoke<VectorSpaceStatusView>("restore_vector_space"),
  vectorSpaceStatus: () => invoke<VectorSpaceStatusView>("vector_space_status"),
  modelBindings: () => invoke<ModelBindingsView>("model_bindings"),
  saveModelBindings: (args: SaveModelBindingsArgs) => invoke<ModelBindingsView>("save_model_bindings", { args }),
  modelPool: () => invoke<ModelPoolView>("model_pool"),
  saveModelPool: (args: SaveModelPoolArgs) => invoke<ModelPoolView>("save_model_pool", { args }),
  /** 获取并记住连接的模型目录；失败时目录不变。 */
  fetchModelCatalog: (providerId: number) => invoke<ModelPoolView>("fetch_model_catalog", { providerId }),
  /** 改一个模型的设置，返回全部模型的新快照。 */
  updatePoolModel: (args: UpdatePoolModelArgs) => invoke<ModelPoolView>("update_pool_model", { args }),
  /** 「测试能力」：会向模型发请求，结果存进模型记录。 */
  probePoolModel: (providerId: number, model: string) =>
    invoke<ModelPoolView>("probe_pool_model", { providerId, model }),
  savePiSettings: (args: SavePiSettingsArgs) => invoke<PiPage>("save_pi_settings", { args }),
  /** 不打断进行中的回复；回复未结束时返回忙。 */
  restartPi: () => invoke("restart_pi"),
  /** 用户显式操作：在后台下载或升级托管对话引擎，会联网；立即返回进度，后续经 `pi:install` 推送。 */
  installPi: () => invoke<PiInstallView>("install_pi"),
  /** 当前（或最近一次）对话引擎下载进度。 */
  piInstallStatus: () => invoke<PiInstallView>("pi_install_status"),
  /** 用户显式操作：安装外部工具（MCP）组件，会联网。 */
  togglePiTool: (name: string, enabled: boolean) => invoke("toggle_pi_tool", { name, enabled }),
  /** 空白恢复默认说明。 */
  setPiToolDescription: (name: string, description: string) => invoke("set_pi_tool_description", { name, description }),
  toggleSkill: (name: string, enabled: boolean) => invoke("toggle_skill", { name, enabled }),
  piSkill: (name: string) => invoke<SkillDraft>("pi_skill", { name }),
  saveSkill: (draft: SkillDraft) => invoke("save_skill", { draft }),
  deleteSkill: (name: string) => invoke("delete_skill", { name }),
  /** `id = 0` 新建；正数只改名称、命令与参数，保留启用状态。 */
  upsertMcp: (input: McpInput) => invoke<number>("upsert_mcp", { input }),
  previewMcpImport: (json: string) => invoke<McpImportEntry[]>("preview_mcp_import", { json }),
  importMcp: (json: string) => invoke<McpImportEntry[]>("import_mcp", { json }),
  deleteMcp: (id: number) => invoke("delete_mcp_server", { id }),
  toggleMcp: (id: number, enabled: boolean) => invoke("toggle_mcp_server", { id, enabled }),
  probeMcp: (input: McpInput) => invoke<McpProbeView>("probe_mcp", { input }),
  loginMcp: (id: number) => invoke<McpLoginResult>("login_mcp", { id }),
  cancelMcpLogin: (id: number) => invoke<void>("cancel_mcp_login", { id }),
  confirmFetch: (id: string, ok: boolean, remember = false) => invoke("confirm_fetch", { id, ok, remember }),
  /** 只在「未验证版本」提示实际展示后调用，传事件里的版本。 */
  acknowledgePiVersionNotice: (version: string) => invoke("acknowledge_pi_version_notice", { version }),
  embedNow: () => invoke("embed_now"),
  resetFailed: () => invoke("reset_failed_embeds"),
  training: (articleId: number, excerpt?: string) =>
    invoke<FeatureView[]>("training_features", { articleId, excerpt: excerpt ?? null }),
  confirmFeature: (p: {
    articleId: number; feedId: number; dislike: boolean; feedScope: boolean; dimension: string; pattern: string;
  }) => invoke<number>("confirm_feature", p),
  probe: () => invoke<Record<string, unknown>>("probe_env"),
};

export function isSafeHttpUrl(url: string): boolean {
  try {
    const parsed = new URL(url);
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch {
    return false;
  }
}

export async function openExternal(url: string): Promise<void> {
  const href = url.trim();
  if (!isSafeHttpUrl(href)) return;
  if (isTauri()) {
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(href);
      return;
    } catch {
      /* fall through */
    }
  }
  window.open(href, "_blank", "noopener,noreferrer");
}

export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    /* fall through */
  }
  if (isTauri()) {
    const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
    await writeText(text);
  }
}

export function onChatDelta(cb: (snapshot: ChatStreamEvent) => void): Promise<UnlistenFn> {
  return onWebEvent<ChatStreamEvent>("chat:delta", cb);
}
export function onChatStart(cb: (id: number) => void): Promise<UnlistenFn> {
  return onWebEvent<number>("chat:start", cb);
}
export function onChatError(cb: (event: ChatErrorEvent) => void): Promise<UnlistenFn> {
  return onWebEvent<ChatErrorEvent>("chat:error", cb);
}
export function onChatDone(cb: (id: number) => void): Promise<UnlistenFn> {
  return onWebEvent<number>("chat:done", cb);
}
/** 结算或压缩后的上下文快照；按 thread_id 替换，不累加。 */
export function onChatContext(cb: (event: ChatContextEvent) => void): Promise<UnlistenFn> {
  return onWebEvent<ChatContextEvent>("chat:context", cb);
}
/** 对话引擎后台下载的进度与结论；离开设置页也照常推送。 */
export function onPiInstall(cb: (view: PiInstallView) => void): Promise<UnlistenFn> {
  return onWebEvent<PiInstallView>("pi:install", cb);
}
/** 一行进度（「正在查阅本地文章」等）；空文本表示清除。 */
export function onChatStatus(cb: (event: ChatStatusEvent) => void): Promise<UnlistenFn> {
  return onWebEvent<ChatStatusEvent>("chat:status", cb);
}
/** 助手请求确认（例如读取网页），用 `confirmFetch` 回应；超时由对话引擎按拒绝处理。 */
export function onChatConfirm(cb: (event: ChatConfirmEvent) => void): Promise<UnlistenFn> {
  return onWebEvent<ChatConfirmEvent>("chat:confirm", cb);
}

export function onMediaReady(cb: (payload: MediaReadyEvent) => void): Promise<UnlistenFn> {
  return onWebEvent<MediaReadyEvent>("media:ready", cb);
}

/** 四条通道全部就绪才允许启动；任意注册失败都清理已注册的监听。 */
export async function onTranslationEvent(cb: (payload: TranslationEvent) => void): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  const results = await Promise.allSettled(["translate:start", "translate:delta", "translate:done", "translate:error"].map(name =>
    listen<TranslationEvent>(name, event => cb(event.payload))));
  const failure = results.find(result => result.status === "rejected");
  const dispose = () => { for (const result of results) if (result.status === "fulfilled") result.value(); };
  if (failure?.status === "rejected") { dispose(); throw failure.reason; }
  return dispose;
}

export function onEmbedCover(cb: (payload: EmbedCoverEvent) => void): Promise<UnlistenFn> {
  return onWebEvent<EmbedCoverEvent>("embed:cover", cb);
}

function onWebEvent<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  if (!isTauri()) return Promise.resolve(() => {});
  return listen<T>(event, e => cb(e.payload));
}

export function onWebNavigated(cb: (event: WebNavigated) => void): Promise<UnlistenFn> {
  return onWebEvent("web:navigated", cb);
}
export function onWebClosed(cb: (event: WebClosed) => void): Promise<UnlistenFn> {
  return onWebEvent("web:closed", cb);
}
export function onWebEscape(cb: (event: WebEscape) => void): Promise<UnlistenFn> {
  return onWebEvent("web:escape", cb);
}
export function onWebDownloadBlocked(cb: (event: WebDownloadBlocked) => void): Promise<UnlistenFn> {
  return onWebEvent("web:download-blocked", cb);
}

/** 设置为「每次询问」时，宿主拦下关闭后请前端弹关闭询问。 */
export function onWindowCloseRequested(cb: () => void): Promise<UnlistenFn> {
  return onWebEvent<null>("window:close-requested", () => cb());
}

/** 窗口从托盘唤回（托盘图标、托盘菜单或再次启动）；最小化后恢复不算。 */
export function onWindowRestored(cb: () => void): Promise<UnlistenFn> {
  return onWebEvent<null>("window:restored", () => cb());
}

export function onWebVerifyState(cb: (event: WebVerifyStateEvent) => void): Promise<UnlistenFn> {
  return onWebEvent("web:verify-state", cb);
}

let feedUpdates: { ready: Promise<UnlistenFn>; changed: (event: FeedUpdated) => void } | undefined;

export async function onFeedUpdated(changed: (event: FeedUpdated) => void): Promise<UnlistenFn> {
  const subscription = { changed, ready: isTauri()
    ? listen<FeedUpdated>("feed:updated", event => changed(event.payload))
    : Promise.resolve(() => {}) };
  feedUpdates = subscription;
  try {
    const unlisten = await subscription.ready;
    return () => {
      unlisten();
      if (feedUpdates === subscription) feedUpdates = undefined;
    };
  } catch (error) {
    if (feedUpdates === subscription) feedUpdates = undefined;
    throw error;
  }
}

export function onSchedulerUpdated(changed: (snapshot: SchedulerSnapshot) => void): Promise<UnlistenFn> {
  return onWebEvent<SchedulerSnapshot>("scheduler:updated", changed);
}

/** 窗口三键的动作与最大化状态；只在有宿主时调用。 */
export const windowControls = {
  minimize: () => getCurrentWindow().minimize(),
  toggleMaximize: () => getCurrentWindow().toggleMaximize(),
  close: () => getCurrentWindow().close(),
  isMaximized: () => getCurrentWindow().isMaximized(),
  onResized: (changed: () => void): Promise<UnlistenFn> => isTauri() ? getCurrentWindow().onResized(() => changed()) : Promise.resolve(() => {}),
};
