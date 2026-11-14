//! UI 无关的应用层：视图模型、动作、作业、Pi 运行时、CLI。

pub mod actions;
pub mod capabilities;
pub mod chat;
mod configuration;
pub mod digest;
pub mod discover;
pub mod document;
pub mod embed_fetch;
pub mod execution;
pub mod extract;
pub mod fulltext;
pub mod image_fetch;
pub mod image_preload;
pub mod jobs;
pub mod log;
pub mod mcp;
pub mod model_pool;
pub mod network;
pub mod network_proxy;
pub mod pi;
pub mod provider_presets;
pub mod queries;
mod query_embedding;
pub mod scheduler;
pub mod settings_schema;
pub mod site_rules;
pub mod tool;
pub mod translate;
pub mod ui_error;
pub mod views;
pub mod web;
pub mod web_snapshot;
pub mod web_verify;
pub mod web_window;
pub mod window_close;

pub use views::*;

#[cfg(test)]
mod test_net;

#[cfg(test)]
mod export_ts {
    use super::*;
    use std::path::Path;
    use ts_rs::TS;

    fn bindings_source() -> String {
        let decls = [
            UiError::decl(),
            UiErrorKind::decl(),
            ui_error::common::CommonError::decl(),
            ui_error::common::LegacyErrorSource::decl(),
            ui_error::feeds::FeedError::decl(),
            ui_error::folders::FolderError::decl(),
            ui_error::rules::RuleError::decl(),
            ui_error::models::ModelError::decl(),
            ui_error::chat::ChatUiError::decl(),
            ui_error::reader::ReaderError::decl(),
            ui_error::network::NetworkError::decl(),
            ui_error::network::NetworkMessage::decl(),
            crate::fulltext::FulltextErrorKind::decl(),
            crate::fulltext::FulltextError::decl(),
            WebNavigated::decl(),
            WebVerifyState::decl(),
            WebVerifyStateEvent::decl(),
            WebClosed::decl(),
            WebEscape::decl(),
            WebDownloadBlocked::decl(),
            WebViewBounds::decl(),
            SiteRuleView::decl(),
            StoredSiteRuleView::decl(),
            SiteRulesPage::decl(),
            SiteRuleTestView::decl(),
            WebLinkLayer::decl(),
            OpenWebLinkView::decl(),
            LinkWarning::decl(),
            LinkSafety::decl(),
            SignalSurface::decl(),
            SignalEventKind::decl(),
            ReadSessionEndedBy::decl(),
            ReadSessionVia::decl(),
            format!("type ExposureIds = {};", ExposureIds::inline()),
            ExposureInput::decl(),
            ArticleEventInput::decl(),
            ReadSessionInput::decl(),
            ArticleView::decl(),
            RankReasonView::decl(),
            ClusterSourceView::decl(),
            ClusterSummaryView::decl(),
            ClusterNoteView::decl(),
            ClusterArticleView::decl(),
            RelatedArticleView::decl(),
            FeedView::decl(),
            AddFeedResult::decl(),
            FeedUpdated::decl(),
            SaveFeedArgs::decl(),
            FolderView::decl(),
            FeedTree::decl(),
            TreeLayout::decl(),
            ReasonView::decl(),
            HiddenArticleView::decl(),
            HiddenRuleCount::decl(),
            HiddenPage::decl(),
            RuleStatsView::decl(),
            SpotCheckStatsView::decl(),
            RuleStatsPage::decl(),
            FeatureView::decl(),
            RuleView::decl(),
            PresetView::decl(),
            ScopeOption::decl(),
            InterestView::decl(),
            InterestHitView::decl(),
            CitationView::decl(),
            DigestSelection::decl(),
            MediaRef::decl(),
            MediaReadyEvent::decl(),
            EmbedCoverView::decl(),
            EmbedCoverEvent::decl(),
            ReadingSpeedsView::decl(),
            ReaderView::decl(),
            TranslationFailure::decl(),
            TranslationUnitState::decl(),
            TranslationPhase::decl(),
            TranslationSaveState::decl(),
            TranslationIdentity::decl(),
            TranslationViewport::decl(),
            TranslationInitialViewport::decl(),
            TranslationHint::decl(),
            TranslationBlockPatch::decl(),
            TranslationSnapshot::decl(),
            TranslationEventKind::decl(),
            TranslationEvent::decl(),
            TranslationStartMode::decl(),
            TranslationReplyKind::decl(),
            TranslationReply::decl(),
            TranslationCommandError::decl(),
            ThreadView::decl(),
            ThreadFolderView::decl(),
            ThreadListView::decl(),
            ThreadSearchHitView::decl(),
            ContextUsageView::decl(),
            ChatContextEvent::decl(),
            ModelCatalogView::decl(),
            PoolEntryView::decl(),
            ModelPoolView::decl(),
            SaveModelPoolArgs::decl(),
            SendMessageArgs::decl(),
            DigestThreadView::decl(),
            MessagesPage::decl(),
            ChatStreamEvent::decl(),
            ChatErrorEvent::decl(),
            ChatStatusEvent::decl(),
            ChatStepView::decl(),
            ArticlePreviewView::decl(),
            ChatConfirmEvent::decl(),
            ChatMessageView::decl(),
            MemoryView::decl(),
            ProviderView::decl(),
            ProviderPreset::decl(),
            CapabilitySupport::decl(),
            CapsView::decl(),
            CapsOverrideView::decl(),
            KeyState::decl(),
            SecretView::decl(),
            ProviderTestView::decl(),
            ConnectionTestArgs::decl(),
            ConnectionTestView::decl(),
            UpdatePoolModelArgs::decl(),
            PiConfigView::decl(),
            SavePiConfigArgs::decl(),
            NetworkProxyMode::decl(),
            NetworkProxyRuleView::decl(),
            NetworkProxyView::decl(),
            NetworkProxyScope::decl(),
            NetworkProxyTestArgs::decl(),
            NetworkProxyTestView::decl(),
            SystemProxyView::decl(),
            SaveNetworkProxyRuleArgs::decl(),
            SaveNetworkProxyArgs::decl(),
            ModelChoice::decl(),
            ModelBindingsView::decl(),
            ModelBindingError::decl(),
            VectorSpaceView::decl(),
            VectorRebuildView::decl(),
            VectorSpaceStatusView::decl(),
            TaskModelInput::decl(),
            SaveModelBindingsArgs::decl(),
            pi::launch::GeneratedConfig::decl(),
            TaskBindView::decl(),
            DigestSummary::decl(),
            DigestView::decl(),
            DigestLineView::decl(),
            DigestSourceView::decl(),
            PiToolView::decl(),
            SkillView::decl(),
            McpView::decl(),
            McpSecretView::decl(),
            McpSecretInput::decl(),
            McpInput::decl(),
            McpToolView::decl(),
            McpProbeView::decl(),
            McpLoginResult::decl(),
            McpImportEntry::decl(),
            SkillDraft::decl(),
            RankingSettings::decl(),
            FocusPage::decl(),
            FeedsCursor::decl(),
            FeedsPage::decl(),
            ScopeCount::decl(),
            FeedCountsView::decl(),
            RefreshOperation::decl(),
            RefreshStarted::decl(),
            SchedulerSnapshot::decl(),
            BackgroundJobView::decl(),
            TimingView::decl(),
            SchedulerTiming::decl(),
            SchedulerCounter::decl(),
            FeedDiagnostics::decl(),
            ModelDiagnostics::decl(),
            CooldownDiagnostics::decl(),
            BackgroundDiagnostics::decl(),
            SchedulerDiagnostics::decl(),
            SearchScope::decl(),
            SearchQuery::decl(),
            SearchBlockHitView::decl(),
            SearchHitBlocksView::decl(),
            SearchHitView::decl(),
            RulesPage::decl(),
            AiPage::decl(),
            AiUsageView::decl(),
            UsageLine::decl(),
            AutoSummaryView::decl(),
            AutoSummaryNotice::decl(),
            SaveAiSettingsArgs::decl(),
            ProviderInput::decl(),
            TranslatePage::decl(),
            SelectionTranslationConfig::decl(),
            SelectionTranslation::decl(),
            MemoryPage::decl(),
            WindowControlsStyle::decl(),
            WindowChromeView::decl(),
            crate::window_close::CloseChoice::decl(),
            TrayLabels::decl(),
            ClientLogLevel::decl(),
            LogUnavailableReason::decl(),
            LogStatusView::decl(),
            DiagnosticsPage::decl(),
            SettingDomain::decl(),
            SettingLevel::decl(),
            SettingApply::decl(),
            SettingKind::decl(),
            SettingOption::decl(),
            SettingSpec::decl(),
            SettingValue::decl(),
            SettingsSchema::decl(),
            StoragePage::decl(),
            StorageGroupView::decl(),
            StorageItemView::decl(),
            RetentionPage::decl(),
            BackupPage::decl(),
            BackupEntryView::decl(),
            RestoreNoticeView::decl(),
            BackupDoneView::decl(),
            RestoreCheckView::decl(),
            PrefsImportView::decl(),
            PiPage::decl(),
            SavePiSettingsArgs::decl(),
            PiProbe::decl(),
            PiInstallView::decl(),
            NewRuleSpec::decl(),
            DiscoverFeedsRequest::decl(),
            DiscoverFeedsResult::decl(),
            DiscoverFeedsError::decl(),
            DiscoveredFeed::decl(),
        ];
        let mut out = String::from("// Generated by rrss-app export_ts. Do not edit by hand.\n\n");
        for decl in decls {
            out.push_str("export ");
            out.push_str(&decl);
            if !decl.trim_end().ends_with(';') {
                out.push(';');
            }
            out.push('\n');
        }
        out.push_str(&format!(
            "export const SELECTION_TRANSLATE_MAX_CHARS = {};\n",
            translate::selection::MAX_CHARS
        ));
        out.replace("bigint", "number")
    }

    fn bindings_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src/bindings.ts")
    }

    #[test]
    #[ignore = "explicit export: cargo test -p rrss-app export_ts -- --ignored"]
    fn write_bindings() {
        let path = bindings_path();
        std::fs::create_dir_all(path.parent().unwrap()).expect("bindings dir");
        std::fs::write(&path, bindings_source()).expect("write bindings.ts");
    }

    #[test]
    fn bindings_are_current() {
        assert_eq!(
            std::fs::read_to_string(bindings_path())
                .unwrap()
                .replace("\r\n", "\n"),
            bindings_source(),
            "run cargo test -p rrss-app export_ts -- --ignored"
        );
    }
}
