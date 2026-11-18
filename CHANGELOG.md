# Changelog

User-visible changes in each version. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow [Semantic Versioning](https://semver.org/). When a version is released, its section is used as the GitHub Release notes. Entries for 0.3.1 and earlier are in Chinese.

## [Unreleased]

### Changed

- While you read in Focus, new articles below the screen appear quietly. Batches that affect your reading position share one button that brings the updates to the top of the list.
- Chat suggests lasting preferences from your own words and uses them across conversations after you confirm them. Conversation summaries stay with their original thread and are deleted with it. Clearing all memories also forgets which suggestions you already saw. Older summaries with no known thread remain available in memory settings.
- Sending a question from the reader opens the conversation immediately with your question and citations visible. The usual chat progress appears as the engine starts, and a failed send keeps the draft ready to retry.
- Chat titles can now be generated on demand from the conversation menu or automatically after the second successful reply in a new conversation. Automatic titles are off by default. Choose a title model in Settings to use either option.

## [0.4.1] - 2026-11-18

### Fixed

- Cache eviction and cleanup errors no longer make successfully saved images, audio/video, feed icons, or web snapshots appear to fail.
- Interest changes save successfully in large libraries while embeddings are updating; expensive ranking recalculation runs in the background.
- Full-text fetches report concurrent content changes separately from network failures, and unreadable body caches are rebuilt automatically.
- Reading, search, and feed lists avoid unnecessary icon scans and repeated body parsing. Failed settings queries report errors instead of showing defaults. Chat startup can be stopped during a slow handshake, streamed replies render off the event loop, and client setup no longer holds a shared cache lock while reading configuration or building connections.
- Interest edits handle overlapping small batches of embedding updates, cluster merges avoid loading old article vectors, and failed media registration or rolled-back web snapshots clean up newly written cache files.
- Background refreshes, clustering, interest updates, and vector preparation use shorter database writes so reading actions stay responsive. Cached images no longer write access timestamps while loading, file cleanup runs after commit, and diagnostics capture database waits and delayed close prompts.
- Closing the window responds right away: the "close RRSS" prompt (or hiding to the tray) no longer waits while background updates are busy, and the choice in the prompt takes effect immediately.
- Digest follow-ups: sending a question now scrolls to it and keeps following the reply, instead of jumping to the start of the follow-up section; the input no longer overlaps the digest text behind it when it grows taller, and long model names are shortened so they do not crowd the input.
- Original web pages open again after you change the proxy settings, without having to clear the original-page data first. Pages opened with the old proxy lose their sign-ins for the rest of the session.
- Settings → Diagnostics shows its status in the interface language again (it had switched to English only), with sizes in MB and the statistics split into lines.

## [0.4.0] - 2026-11-16

### Added

- English interface: a Language option at the bottom of the settings navigation (System / 简体中文 / English) switches immediately without a restart. It follows the system language by default: Chinese systems get Chinese, everything else gets English.
- AI output language: summaries, digests, related coverage overviews and memories are written in the interface language by default, or in a language you choose under Settings → Models → "AI output language" (Simplified Chinese or English). Only content generated afterwards changes; existing summaries and digests keep their language until regenerated. The chat assistant replies in the language you write in (or the one you ask for) and falls back to this setting when it cannot tell, for example when you only send a link.
- Run in the background: closing the window can hide RRSS to the system tray, where it keeps updating feeds on schedule. The first close asks and can remember the choice; change it later under Settings → Advanced → "When closing the window". The tray menu opens the window, refreshes now or quits, and coming back from the tray shows the latest items in Today. Starting RRSS again brings up the running window.
- "Related reading" at the end of an article: up to 3 articles with similar titles and summaries after "Next article" (requires an embedding model). Esc or "Previous" returns to the original article, and previous / next still follow the original list.
- Syntax highlighting in code blocks: code in the reader, chats and digests is highlighted by the language the source declares; without a declaration it is only colored when the language is recognized reliably. Copied and cited text is unchanged.
- The chat assistant can look up its own current configuration (instructions, enabled tools and skills, current model) to answer questions like "what can you do right now". Read-only, without keys or service addresses.

### Changed

- AI summaries and digests read more directly: summaries no longer say "the article says" or point out what the source leaves out, and drop scores and community reactions; digest items open with a headline-style bold lead, and commands and settings are shown as code.
- Translation now targets the interface language by default: a Chinese interface still translates into Chinese, an English interface into English. A target language chosen earlier is kept.
- Feeds without a title show their host name in every list (local bridges show the route name or address) instead of "Unnamed source" or the full address.
- Proxy addresses with a username and password are shown as `***@host` in the detected system proxy and the generated chat-engine configuration under Settings → Advanced.
- Settings → Models no longer lists "Citation summary" under Summary: the task was never actually used, and a model chosen for it no longer appears.
- Today: right after opening the app, before you start browsing, newly fetched articles appear at the top of the list directly instead of behind a "N new items" prompt; the prompt is used once you scroll, select an article with the keyboard or open one. A fetch that was running at startup no longer scrolls the list you are reading back to the top when it finishes.

### Fixed

- Confirmation prompts for the chat assistant's actions (such as marking an article read or fetching a web page) show the action's name in the interface language instead of fixed Chinese text.
- Compressing the older part of a long chat into memory no longer fails with model services that enforce structured output.
- Importing a preferences file explains why a rule was skipped (for example, an invalid regular expression), without mixed-language messages.
- When a feed fails on its first fetch, a short failure note is kept, and timeouts read "the feed request timed out".
- Code blocks in the reader show the language declared by the source (including common blog formats such as Jekyll, GitHub and Hugo), and Markdown code block language names are normalized. Applies to articles fetched from now on.
- Network proxy: changing the proxy to another host (different scheme, host or port) no longer reuses the saved proxy credentials, and the settings page says they will be cleared on save, so the password is not sent to a different proxy.
- Network proxy: a system bypass list with entries such as `*.example.com` or `host:8080` no longer makes requests fail; entries that can be converted take effect as equivalents, and unrecognized ones are skipped.
- Network proxy: in "System proxy" mode, local tools the chat assistant calls (searching articles, reading web pages) now use the system proxy settings instead of the chat assistant's (Pi's) own proxy or a direct connection.

## [0.3.1] - 2026-11-12

首个公开版本。

### 主要功能

- **今日焦点**：打开就是按规则与兴趣排好序的卡片流，每条都写明推荐原因；「仅关注 / 隐藏弱化 / 所有文章」一键切换，噪声源自动收起。
- **相近报道成组**：同一事件的多家报道合成一组，可一键生成综述，列出共识、各家补充与说法分歧，并标出来源。
- **焦点栏目**：把文件夹设为栏目，按主题分开看。
- **阅读器**：正文清洗后排版，附 AI 摘要；整篇对照翻译与划词翻译（大模型 / DeepL / DeepLX）；原网页在隔离的内嵌网页视图中打开。
- **深度对话**：把整篇文章或划选的一段带进对话，能检索本机文章、调用你配置的外部工具（MCP），回答里的链接回到原文。
- **简报**：自动生成日报 / 周报，每条结论标出来源，读完可直接追问。
- **搜索**：<kbd>Ctrl</kbd> + <kbd>K</kbd> 检索文章与对话，本机全文索引，不调用模型。
- **订阅与规则**：文件夹树与拖动整理、OPML 导入导出、源健康提示；规则读起来像一句话，系统建议的规则要你确认才生效。
- **长期使用**：备份与恢复、文章保留策略、存储占用一览、运行日志与诊断包；全局或按用途的 HTTP / SOCKS5 代理，Windows 可读取系统代理。

### 修复

- Windows 任务栏与标题栏图标按显示缩放选用对应尺寸，不再发糊。

### 发布包

- Linux x86_64、Windows x64（MSVC）与 Windows x64（MinGW），附 `SHA256SUMS`。
