//! 凭证、模板、缓存、日预算。日志 target：`rrss_core::ai_store`。

use crate::error::models::{CompatProblem, TaskModelLabel};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::{get_setting, set_setting};
use crate::error::{Error, Result};
use crate::feeds::now_ms;

pub const AI_DAILY_BUDGET_TOKENS_KEY: &str = "ai_daily_budget_tokens";
pub const AI_EMBED_DAILY_BUDGET_TOKENS_KEY: &str = "ai_embed_daily_budget_tokens";

/// 注册表共享的 AI 设置默认值；core 的读取与 app 的描述符共用这些条目。
pub struct SettingDefault {
    pub id: &'static str,
    pub default: &'static str,
}

impl SettingDefault {
    pub fn int(&self, conn: &Connection) -> Result<i64> {
        Ok(get_setting(conn, self.id)?
            .and_then(|s| s.trim().parse::<i64>().ok())
            .filter(|n| (0..=1_000_000_000).contains(n))
            .unwrap_or_else(|| self.default.parse().expect("registered integer default")))
    }

    pub fn enabled(&self, conn: &Connection) -> Result<bool> {
        crate::db::setting_enabled(conn, self.id, self.default == "1")
    }
}

pub const GENERATION_BUDGET: SettingDefault = SettingDefault {
    id: AI_DAILY_BUDGET_TOKENS_KEY,
    default: "200000",
};
pub const EMBED_BUDGET: SettingDefault = SettingDefault {
    id: AI_EMBED_DAILY_BUDGET_TOKENS_KEY,
    default: "1000000",
};
pub const FOCUS_AUTO_SUMMARY: SettingDefault = SettingDefault {
    id: "focus_auto_summary",
    default: "0",
};
pub const CLUSTER_AUTO_SUMMARY: SettingDefault = SettingDefault {
    id: "cluster_auto_summary",
    default: "0",
};

pub const FOCUS_AUTO_SUMMARY_NOTICE_KEY: &str = "focus_auto_summary_notice";
pub const CLUSTER_DAILY_LIMIT: i64 = 20;
pub const CLUSTER_WINDOW_DAYS: i64 = 3;
pub const CLUSTER_REFRESH_HOURS: i64 = 24;
pub const CLUSTER_BATCH: i64 = 4;
pub const FOCUS_DAILY_LIMIT: i64 = 100;
pub const FOCUS_WINDOW_DAYS: i64 = 3;
pub const FOCUS_BATCH: i64 = 8;
pub const FOCUS_BATCH_MINUTES: u64 = 15;

#[derive(Clone, Copy)]
pub enum BudgetKind {
    Generation,
    Embedding,
}

/// 输入档位：low / medium / high，缺省 medium。
pub const AI_INPUT_TIER_KEY: &str = "ai_input_tier";
pub const PI_BINARY_KEY: &str = "pi_binary";
pub const DEVELOPER_TOOLS_KEY: &str = "developer_tools";
pub const FETCH_URL_ENABLED_KEY: &str = "fetch_url_enabled";

/// 用途当前选的「连接 + 模型」（阶段 19-7：没有主模型与继承）。没选、连接已删或模型为空时为 `None`。
pub fn resolve_binding_model(
    conn: &Connection,
    binding: &TaskBinding,
) -> Result<Option<(AiProvider, String)>> {
    let (Some(id), Some(model)) = (
        binding.provider_id,
        binding
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty()),
    ) else {
        return Ok(None);
    };
    match get_provider(conn, id) {
        Ok(p) => Ok(Some((p, model.to_string()))),
        Err(Error::ProviderNotFound) => Ok(None),
        Err(err) => Err(err),
    }
}

/// 对话默认模型（`chat` 用途）。
pub fn chat_default(conn: &Connection) -> Result<Option<crate::model_pool::ModelRef>> {
    let binding = get_task_binding(conn, TASK_CHAT)?;
    Ok(resolve_binding_model(conn, &binding)?
        .map(|(p, model)| crate::model_pool::ModelRef::new(p.id, &model)))
}

pub const TASK_FOCUS_SUMMARY: &str = "focus_summary";
pub const TASK_ARTICLE_SUMMARY: &str = "article_summary";
/// 手动「本篇摘要」遇到长文时，先逐段压缩成笔记，再交给 `article_summary` 汇总。
pub const TASK_ARTICLE_SEGMENT: &str = "article_segment";
pub const TASK_DIGEST_DAILY: &str = "digest_daily";
pub const TASK_DIGEST_WEEKLY: &str = "digest_weekly";
pub const TASK_DIGEST_PICK_DAILY: &str = "digest_pick_daily";
pub const TASK_DIGEST_PICK_WEEKLY: &str = "digest_pick_weekly";
/// 相近报道综述：手动或显式开启的后台汇总，按簇存库。默认沿用本篇摘要的模型。
pub const TASK_CLUSTER_SUMMARY: &str = "cluster_summary";
pub const TASK_MEMORY_CANDIDATE: &str = "memory_candidate";
pub const TASK_EPISODE_COMPRESS: &str = "episode_compress";
pub const TASK_ARTICLE_TRANSLATE: &str = "article_translate";
pub const TASK_EMBED: &str = "embed";
pub const TASK_CHAT: &str = "chat";

pub const ALL_TASKS: &[&str] = &[
    TASK_FOCUS_SUMMARY,
    TASK_ARTICLE_SUMMARY,
    TASK_ARTICLE_SEGMENT,
    TASK_DIGEST_DAILY,
    TASK_DIGEST_WEEKLY,
    TASK_DIGEST_PICK_DAILY,
    TASK_DIGEST_PICK_WEEKLY,
    TASK_CLUSTER_SUMMARY,
    TASK_MEMORY_CANDIDATE,
    TASK_EPISODE_COMPRESS,
    TASK_ARTICLE_TRANSLATE,
    TASK_EMBED,
    TASK_CHAT,
];

/// AI 输出语言（阶段 27 · I3）：后台作业写给用户看的字段用哪种语言。提示词只有一份英文，
/// 语言名用英文写进指令；加语言只加取值与语言名，不改提示词。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputLanguage {
    ZhCn,
    En,
}

impl OutputLanguage {
    /// 界面语言标签（`zh-CN`、`en`）到输出语言；认不出的落到英文。
    pub fn from_tag(tag: &str) -> Self {
        if tag.trim().to_ascii_lowercase().starts_with("zh") {
            Self::ZhCn
        } else {
            Self::En
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::En => "en",
        }
    }

    /// 写进英文指令里的语言名。
    pub fn english_name(self) -> &'static str {
        match self {
            Self::ZhCn => "Simplified Chinese",
            Self::En => "English",
        }
    }
}

pub const SLUG_FOCUS_SUMMARY: &str = "focus-summary";
pub const SLUG_ARTICLE_SUMMARY: &str = "article-summary";
pub const SLUG_ARTICLE_SEGMENT: &str = "article-segment";
pub const SLUG_DAILY_DIGEST: &str = "daily-digest";
pub const SLUG_WEEKLY_DIGEST: &str = "weekly-digest";
pub const SLUG_DAILY_DIGEST_PICK: &str = "daily-digest-pick";
pub const SLUG_WEEKLY_DIGEST_PICK: &str = "weekly-digest-pick";
pub const SLUG_CLUSTER_SUMMARY: &str = "cluster-summary";
pub const SLUG_MEMORY_CANDIDATE: &str = "memory-candidate";
pub const SLUG_EPISODE_COMPRESS: &str = "episode-compress";
pub const SLUG_ARTICLE_TRANSLATE: &str = "article-translate";

const SEED_TEMPLATES: &[(&str, &str, &str)] = &[
    (
        SLUG_FOCUS_SUMMARY,
        "Focus summary",
        "You are the reading assistant of a local RSS reader. From the title and the structured body excerpt, write:\n- summary: 2–4 sentences. Open with what happened, then the specifics that matter most (numbers, versions, dates, how it works), and end with why it matters or what is still open when the text supports it.\n- one_liner: one sentence a reader can take in at a glance on a card: who did what, with the most telling number or consequence. It is not a compressed copy of the summary.\n- key_points: at most 3 short points, each adding something the one_liner does not already say.\n- facts: at most 5. Each fact is one sentence that names the subject and a concrete number, version, date or clear conclusion; if the source has none, write fewer or leave the list empty; skip anything already stated in the key points.\n\nWrite about the subject directly; never describe the source (\"the article says\", \"according to the title\", \"the excerpt shows\"). Write only what the text says: do not point out what it leaves out (\"unclear\", \"not disclosed\", \"details are not given\") unless the text itself says so, and do not add general background or speculation. Leave out scores, upvotes, comment counts and community reactions. These fields are shown as plain text: no Markdown, asterisks or bullet characters. A \"…\" in the excerpt marks omitted paragraphs. Use only the given text; do not invent anything. If the input is marked [No body text] or only has a link, the summary and one_liner just restate the title in your own words, key_points and facts are empty, and you add nothing else. Output only JSON: {\"summary\":\"...\",\"one_liner\":\"...\",\"key_points\":[\"...\"],\"facts\":[\"...\"]}",
    ),
    (
        SLUG_ARTICLE_SUMMARY,
        "Article summary",
        "You are the reading assistant of a local RSS reader. From the title and the structured body excerpt (a long article may already be condensed into notes, part by part in the original order; write from the notes and do not retell them part by part), write:\n- summary: a fuller short summary of about 4–8 sentences. Open with the main point, follow the article's argument or sequence of events with its key specifics (numbers, versions, dates, mechanisms), and end with the conclusion, consequence or open question.\n- one_liner: one sentence a reader can take in at a glance on a card: who did what, with the most telling number or consequence. It is not a compressed copy of the summary.\n- key_points: at most 3 short points, each adding something the one_liner does not already say.\n- facts: at most 5. Each fact is one sentence that names the subject and a concrete number, version, date or clear conclusion; if the source has none, write fewer or leave the list empty; skip anything already stated in the key points.\n\nWrite about the subject directly; never describe the source (\"the article says\", \"according to the title\", \"the excerpt shows\"). Write only what the text says: do not point out what it leaves out (\"unclear\", \"not disclosed\", \"details are not given\") unless the text itself says so, and do not add general background or speculation. Leave out scores, upvotes, comment counts and community reactions. These fields are shown as plain text: no Markdown, asterisks or bullet characters. A \"…\" in the excerpt marks omitted paragraphs. Use only the given text; do not invent anything. If the input is marked [No body text] or only has a link, the summary and one_liner just restate the title in your own words, key_points and facts are empty, and you add nothing else. Output only JSON: {\"summary\":\"...\",\"one_liner\":\"...\",\"key_points\":[\"...\"],\"facts\":[\"...\"]}",
    ),
    (
        SLUG_ARTICLE_SEGMENT,
        "Long article segment notes",
        "You are helping a reader condense one part of a long article; the notes for all parts will later be combined into one summary. From the title and this part of the body, write notes of 3–6 sentences: keep this part's arguments, mechanisms, numbers and conclusions in the original order, without filler such as \"this section\" or \"the author argues\". Then list at most 5 facts. Plain text only, no Markdown. Each fact is one sentence that names the subject and a concrete number, version, date or clear conclusion; if the source has none, write fewer or leave the list empty. A \"…\" in the excerpt marks omitted paragraphs. Use only the given text; do not invent anything. Output only JSON: {\"notes\":\"...\",\"facts\":[\"...\"]}",
    ),
    (
        SLUG_DAILY_DIGEST,
        "Daily digest",
        "You are the digest editor of a local RSS reader, writing today's digest. The reader has not read the originals; after reading the digest they should truly understand what happened today, not merely know that something happened.\n\nThe input is the reporting selected for this issue, grouped into 【Event n】 blocks; within a block, each article is numbered 〈n〉 and has a title, a one-line summary, key points, key facts and a body excerpt (the full text when it fits; long articles get whole paragraphs, and \"…\" marks omissions). Cite with the global 〈n〉 numbers. The input format is only for you to read; do not reproduce it in the digest.\n\nCoverage (hard requirement):\n- Write exactly one item per 【Event n】 block: as many items in total as there are events. Do not split one event into one item per article\n- Merge all articles of the same 【Event n】 into that one item and give each side's information in it\n- Before you finish, check that every 〈n〉 is reflected in the text of some item\n\nHow to write each item:\n- Open with a **bold lead** that reads like a headline: the subject (product / company / project / person) and what it did, with the decisive number if there is one. The lead begins the first sentence: continue that sentence after it (with a comma or the next clause) instead of restating it\n- Then 1–3 sentences with the substance: what exactly changed, how it works or why it happened, what is different from before, and who is affected or what comes next. Pick the details a reader needs to understand the story; drop minor numbers rather than listing every one\n- Length after the lead: normally about 60–120 Chinese characters or 40–80 English words; an important event with rich excerpts may reach about 200 Chinese characters or 130 English words, never more\n- Proper nouns, data and conclusions from the excerpts belong in the text; do not abstract them into vague phrases like \"brings several improvements\" or \"draws attention\"\n- When an article really offers only a sentence or two, keep the item short; do not pad or repeat\n- For an article marked [No body text]: one or two sentences that state the topic, repeating only what the title already says. Do not fill in drivers, power, benchmarks, prices or similar details from what such projects usually do\n- Inline Markdown that helps scanning: `code` for commands, file names, configuration options, APIs and package names; besides the bold lead, you may bold the single most important figure or outcome in the item; *italics* only for titles of works. No other formatting\n\nSections:\n- Group items into 2–5 sections by topic (each like a `###` heading); titles are concrete fields (such as AI and models, open-source tools, security, hardware, science, society)\n- No catch-all sections such as \"Other\", \"Miscellaneous\" or \"Roundup\"\n\nDo not write:\n- Introductions, conclusions or coverage summaries (\"today also covers…\", \"in addition…\", \"spanning many fields…\")\n- Names of the information sources or news roundups; second-hand framing such as \"reportedly\", \"a user shared\" or \"netizens say\"\n- Scores, rankings, upvotes, community buzz, article_id, or remarks about what the input leaves out (\"details are limited\", \"not yet disclosed\", \"remains unclear\") unless a source itself says so\n- \"Title: summary\" form-filling\n- Facts, numbers or causes that are not in the input; leave it out rather than guess\n\nOutput structure and source mapping:\n- Output only a JSON object with a sections array; every section must have title and an items array, and every item must have text and a refs array. Do not output body or item_refs\n- title is a single line of plain text naming the field; text is a single line of inline Markdown (bold, italics, inline code and links are fine). Do not put a `-` list marker, headings, line breaks, code blocks, HTML or nested lists in text\n- refs lists the global 〈n〉 numbers (positive integers) the item uses, 1–3 of them; when articles of one event are merged, list several; when unsure, list only the most relevant one. Keep refs in the same item as its text\n- Each 〈n〉 may appear in the refs of only one item: an article that touches several aspects still becomes one item, not two similar ones\n- Never put `〈n〉`, `article_id` or source names in text\n\nOutput only JSON: {\"sections\":[{\"title\":\"AI and models\",\"items\":[{\"text\":\"**Some product launches** …\",\"refs\":[1,2]}]}]}",
    ),
    (
        SLUG_WEEKLY_DIGEST,
        "Weekly digest",
        "You are the digest editor of a local RSS reader, writing this week's digest. The reader may have skipped the daily digests; after reading the weekly digest they should truly understand what happened this week and where things are heading.\n\nThe input has three parts; always cite with the global 〈n〉 numbers:\n- Daily digest items: the items already written in this week's daily digests, in date order. Each line starts with the date and section, and the 〈n〉 at the end is its source article\n- Source articles: the one-line summary, key points and key facts of the articles behind those numbers, for checking and filling in details\n- Supplementary articles: articles the daily digests missed that are worth adding this week, with a summary, key facts and a body excerpt (the full text when it fits; long articles get whole paragraphs, and \"…\" marks omissions). Weeks without daily digests have only this part\nThe input format is only for you to read; do not reproduce it in the digest.\n\nWhat to keep:\n- The weekly digest is not a bundle of dailies: merge one thread that spans several days into one item, and describe how it developed and where it stands now. Daily items that appeared once and matter little may be dropped\n- Aim for 8–15 items, favoring the events that make up this week's main threads while keeping a spread of fields. Write a supplementary article as its own item when it deserves one, or fold it into a related thread\n- One source, one story, one item; do not split it into two similar items\n\nHow to write each item:\n- Open with a **bold lead** that reads like a headline: the subject and what it did, with the decisive number if there is one. The lead begins the first sentence: continue that sentence after it (with a comma or the next clause) instead of restating it\n- Then 1–3 sentences with the substance: what exactly changed, how it developed over the week, what is different from before, and who is affected or what comes next. Pick the details a reader needs; drop minor numbers rather than listing every one\n- Length after the lead: normally about 60–140 Chinese characters or 40–90 English words; a main thread of the week may reach about 200 Chinese characters or 130 English words, never more\n- Proper nouns, data and conclusions from the daily items, summaries and key facts belong in the text; do not abstract them into vague phrases like \"brings several improvements\"\n- When the material really is only a sentence or two, keep the item short; do not pad or repeat\n- For an article marked [No body text]: one or two sentences that state the topic, repeating only what the title already says. Do not fill in drivers, power, benchmarks, prices or similar details\n- Inline Markdown that helps scanning: `code` for commands, file names, configuration options, APIs and package names; besides the bold lead, you may bold the single most important figure or outcome in the item; *italics* only for titles of works. No other formatting\n\nSections:\n- Group items into 3–6 sections by topic (each like a `###` heading); titles are concrete fields\n- No catch-all sections such as \"Other\", \"Miscellaneous\" or \"Roundup\"\n\nDo not write:\n- Introductions, conclusions or coverage summaries (\"this week also covers…\", \"in addition…\", \"spanning many fields…\")\n- Names of the information sources, the daily digests or news roundups; second-hand framing such as \"reportedly\", \"a user shared\" or \"netizens say\"; phrases that echo the input structure such as \"on Monday\" or \"the daily digest mentioned\" (when timing matters, give the actual date)\n- Scores, rankings, upvotes, community buzz, article_id, or remarks about what the input leaves out (\"details are limited\", \"not yet disclosed\", \"remains unclear\") unless a source itself says so\n- \"Title: summary\" form-filling\n- Facts, numbers or causes that are not in the input; leave it out rather than guess\n\nOutput structure and source mapping:\n- Output only a JSON object with a sections array; every section must have title and an items array, and every item must have text and a refs array. Do not output body or item_refs\n- title is a single line of plain text naming the field; text is a single line of inline Markdown (bold, italics, inline code and links are fine). Do not put a `-` list marker, headings, line breaks, code blocks, HTML or nested lists in text\n- refs lists the global 〈n〉 numbers (positive integers) the item is based on, 1–3 of them; when you merge several daily items, pick the sources that best support this one\n- Each 〈n〉 may appear in the refs of only one item\n- Never put `〈n〉`, article_id or source names in text\n\nOutput only JSON: {\"sections\":[{\"title\":\"AI and models\",\"items\":[{\"text\":\"**Some product launches** …\",\"refs\":[1,2]}]}]}",
    ),
    (
        SLUG_DAILY_DIGEST_PICK,
        "Daily digest picks",
        "You are the editor of a local RSS reader, picking stories for today's digest. The input is a catalog of candidates grouped by topic; each article starts with [id:N] and has a title, a one-line summary and key points.\n\nRules:\n- Aim for 10–12 events; if quality is lacking, go as low as 6. Never pick just to fill the count\n- Pick 1 article per cluster by default; pick a second only when it adds complementary facts (different numbers, mechanisms or statements), at most 2\n- First choice: launches, version updates, policy and regulation, incidents and security, research results, acquisitions and funding, and other reports with a clear subject and facts\n- Second choice: technical long reads or in-depth analysis with concrete data or methods\n- Last resort: purely personal stories, community discussions without concrete information, clickbait roundups. Rank articles with no body text or only a title lower\n- Keep a spread of fields; do not let one topic take most of the picks\n- The \"noisy sources\" listed at the end only lower priority; the digest never names them\n\nreason is only for your own judgment; readers never see it. Output only JSON: {\"picks\":[{\"article_id\":123,\"reason\":\"short reason\"}]}",
    ),
    (
        SLUG_WEEKLY_DIGEST_PICK,
        "Weekly digest picks",
        "You are the editor of a local RSS reader, picking supplementary articles for this week's digest beyond the daily digests. The input is a catalog of candidates grouped by topic; each article starts with [id:N] and has a title, a one-line summary and key points. When this week already has daily digests, the candidates are the articles they missed; otherwise they are all of this week's candidates.\n\nRules:\n- The count requested at the start of the input is an upper limit; if quality is lacking, pick fewer or none. Never pick just to fill the count\n- Pick 1 article per cluster by default; pick a second only when it adds complementary facts, at most 2\n- First choice: reports with a clear change, impact or continuation this week, especially ones that tie into a thread\n- Second choice: in-depth pieces with concrete data or methods\n- Last resort: purely personal stories, community discussions without concrete information, clickbait roundups. Rank articles with no body text or only a title lower\n- Keep a spread of fields; do not let one topic take most of the picks\n- The \"noisy sources\" listed at the end only lower priority; the digest never names them\n\nreason is only for your own judgment; readers never see it. Output only JSON: {\"picks\":[{\"article_id\":123,\"reason\":\"short reason\"}]}",
    ),
    (
        SLUG_CLUSTER_SUMMARY,
        "Related coverage overview",
        "You are a news editor. The user gives several reports of the same event; each has a 〈n〉 number, source, time and title, plus its existing one-line summary, key points, key facts or a body excerpt. Write the overview from this material only: add no outside information and do not judge the reports themselves. Put 〈n〉 numbers only in ref / refs; never write 〈n〉 anywhere in the text, and refer to a report by its source name.\n\nOutput only JSON: {\"overview\":\"...\",\"additions\":[{\"ref\":2,\"text\":\"...\"}],\"conflicts\":[{\"refs\":[1,3],\"text\":\"...\"}]}\n- overview: 2–4 sentences on what happened: who did what, when, and with what impact. Bold (**…**) the subject of the event and at most one decisive figure; no other Markdown. Do not name sources.\n- additions: list only the reports that give information none of the others have, at most one entry each, in one plain-text sentence stating what extra they add (numbers, details, background, responses). ref is that report's number; do not name the source in text, the interface shows it separately. Scores, upvotes, comment counts and community reactions are not information: leave out a report whose only extra is one of these. Leave out reports that add nothing; if none do, give [].\n- conflicts: only cases where reports contradict each other on a number or fact about the same thing, for example one says it launches in March and another says May. One report mentioning something the other omits, different levels of detail, different counting methods, or more or less precise dates are not conflicts: put unique information in additions, otherwise leave it out. Each entry is one plain-text sentence giving what each side says, naming them by source; refs lists the reports involved. Both sides must be stated explicitly in the reports; something one report leaves unsaid is never a conflict. At most 3; if none, give [].\nEverything you write must be supported by the reports; do not point out what they leave out.",
    ),
    (
        SLUG_MEMORY_CANDIDATE,
        "Memory candidates",
        "From the conversation, extract 0–3 stable facts about the user themselves (preferences, constraints, identity). Do not record news content. If there are none, output an empty array. Output only JSON: {\"facts\":[\"...\"]}",
    ),
    (
        SLUG_EPISODE_COMPRESS,
        "Episode compression",
        "Compress the earlier conversation into one episode summary for later retrieval. Do not invent anything. Output only JSON: {\"body\":\"...\"}",
    ),
    (
        SLUG_ARTICLE_TRANSLATE,
        // 原文不改：模型译文按模板正文判断是否过期，改一个字就会让已存的整篇译文全部失效、要重新付费翻译；
        // 输出语言由请求里的目标语言决定，这段说明只管标记。
        "本篇翻译",
        "你是翻译器。把用户给出的 JSON 单元译成指定目标语言。保留所有 <x数字>...</x数字> 与 <x数字/> 标记及编号，不要翻译标记本身，不要改动代码占位。不要解释。只输出 JSON：{\"units\":[{\"i\":0,\"t\":\"...\"}]}",
    ),
];

const SEED_BINDINGS: &[(&str, &str)] = &[
    (TASK_FOCUS_SUMMARY, SLUG_FOCUS_SUMMARY),
    (TASK_ARTICLE_SUMMARY, SLUG_ARTICLE_SUMMARY),
    (TASK_ARTICLE_SEGMENT, SLUG_ARTICLE_SEGMENT),
    (TASK_DIGEST_DAILY, SLUG_DAILY_DIGEST),
    (TASK_DIGEST_WEEKLY, SLUG_WEEKLY_DIGEST),
    (TASK_DIGEST_PICK_DAILY, SLUG_DAILY_DIGEST_PICK),
    (TASK_DIGEST_PICK_WEEKLY, SLUG_WEEKLY_DIGEST_PICK),
    (TASK_CLUSTER_SUMMARY, SLUG_CLUSTER_SUMMARY),
    (TASK_MEMORY_CANDIDATE, SLUG_MEMORY_CANDIDATE),
    (TASK_EPISODE_COMPRESS, SLUG_EPISODE_COMPRESS),
    (TASK_ARTICLE_TRANSLATE, SLUG_ARTICLE_TRANSLATE),
];

/// 连接。`api_key` 为空串表示从不含密钥的备份恢复、需要重新填写（阶段 19-9）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProvider {
    pub id: i64,
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub kind: String,
    pub protocol: String,
    pub last_test: Option<ProviderTest>,
}

/// 最近一次「测试」（阶段 19-7）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderTest {
    pub at: i64,
    pub ok: bool,
    /// 失败原因，或通过时返回的模型数。
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub protocol: String,
}

#[derive(Debug, Clone)]
pub struct PromptTemplate {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub body: String,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskBinding {
    pub task: String,
    pub provider_id: Option<i64>,
    pub template_id: Option<i64>,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedTask {
    pub task: String,
    pub template: PromptTemplate,
    pub provider: AiProvider,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct EmbedEndpoint {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UsageDay {
    pub tokens: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub embed_tokens: i64,
    pub auto_summaries: i64,
    pub auto_cluster_summaries: i64,
}

/// 新库与迁移到 v4 后补种子（模板 / 任务绑定）。可重复执行。
pub fn seed_defaults(conn: &Connection) -> Result<()> {
    for (slug, title, body) in SEED_TEMPLATES {
        conn.execute(
            "INSERT INTO prompt_templates (slug, title, body, version)
             VALUES (?1, ?2, ?3, 1)
             ON CONFLICT (slug) DO UPDATE SET
               title = excluded.title,
               body = excluded.body,
               version = CASE WHEN prompt_templates.body = excluded.body
                              THEN prompt_templates.version
                              ELSE prompt_templates.version + 1 END",
            params![slug, title, body],
        )?;
    }
    for (task, slug) in SEED_BINDINGS {
        conn.execute(
            "INSERT INTO task_bindings (task, provider_id, template_id, model)
             SELECT ?1, sibling.provider_id, t.id, sibling.model
             FROM prompt_templates t
             LEFT JOIN task_bindings sibling ON sibling.task = CASE
               WHEN ?1 IN ('digest_pick_weekly', 'digest_weekly') THEN 'digest_weekly'
               WHEN ?1 IN ('digest_pick_daily', 'digest_daily') THEN 'digest_daily'
               WHEN ?1 IN ('article_segment', 'cluster_summary') THEN 'article_summary'
               ELSE NULL
             END
             WHERE t.slug = ?2
             ON CONFLICT (task) DO NOTHING",
            params![task, slug],
        )?;
    }
    conn.execute(
        "INSERT INTO task_bindings (task, provider_id, template_id, model)
         VALUES (?1, NULL, NULL, NULL)
         ON CONFLICT (task) DO NOTHING",
        [TASK_EMBED],
    )?;
    conn.execute(
        "INSERT INTO task_bindings (task, provider_id, template_id, model)
         VALUES (?1, NULL, NULL, NULL)
         ON CONFLICT (task) DO NOTHING",
        [TASK_CHAT],
    )?;
    Ok(())
}

pub fn insert_provider(conn: &Connection, new: &NewProvider) -> Result<i64> {
    let protocol = normalize_protocol(&new.protocol);
    conn.execute(
        "INSERT INTO ai_providers (name, base_url, api_key, kind, protocol, created_at)
         VALUES (?1, ?2, ?3, 'openai_compat', ?4, ?5)",
        params![new.name, new.base_url, new.api_key, protocol, now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_provider(conn: &Connection, provider: &AiProvider) -> Result<()> {
    let protocol = normalize_protocol(&provider.protocol);
    let n = conn.execute(
        "UPDATE ai_providers SET name = ?1, base_url = ?2, api_key = ?3, protocol = ?4, kind = ?6
         WHERE id = ?5",
        params![
            provider.name,
            provider.base_url,
            provider.api_key,
            protocol,
            provider.id,
            provider.kind
        ],
    )?;
    if n == 0 {
        Err(Error::ProviderNotFound)
    } else {
        Ok(())
    }
}

pub fn normalize_protocol(raw: &str) -> &'static str {
    match raw.trim() {
        "responses" => "responses",
        "auto" => "auto",
        _ => "chat",
    }
}

pub fn set_provider_protocol(conn: &Connection, id: i64, protocol: &str) -> Result<()> {
    let n = conn.execute(
        "UPDATE ai_providers SET protocol = ?1 WHERE id = ?2",
        params![normalize_protocol(protocol), id],
    )?;
    if n == 0 {
        Err(Error::ProviderNotFound)
    } else {
        Ok(())
    }
}

/// Pi 兼容开关对象（`models.json` 的 `compat`），缺省为空对象。
pub fn provider_compat(conn: &Connection, id: i64) -> Result<serde_json::Value> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT compat_json FROM ai_providers WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    let raw = raw.ok_or(Error::ProviderNotFound)?;
    Ok(serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({})))
}

/// 只接受 JSON 对象；非法值拒绝写入，保留原配置。
pub fn set_provider_compat(conn: &Connection, id: i64, compat: &serde_json::Value) -> Result<()> {
    if !compat.is_object() {
        return Err(Error::InvalidCompat(CompatProblem::ObjectRequired));
    }
    let n = conn.execute(
        "UPDATE ai_providers SET compat_json = ?1 WHERE id = ?2",
        params![compat.to_string(), id],
    )?;
    if n == 0 {
        Err(Error::ProviderNotFound)
    } else {
        Ok(())
    }
}

/// 记下「测试」的结果。
pub fn set_provider_test(conn: &Connection, id: i64, ok: bool, note: &str) -> Result<()> {
    let n = conn.execute(
        "UPDATE ai_providers SET last_test_at = ?2, last_test_ok = ?3, last_test_note = ?4 WHERE id = ?1",
        params![id, now_ms(), ok, note],
    )?;
    if n == 0 {
        Err(Error::ProviderNotFound)
    } else {
        Ok(())
    }
}

/// 删除连接：用到它的用途变为未选择，对话线程上选过它的模型清掉（之后用对话默认）；
/// 它的目录与模型随外键级联删除。
pub fn delete_provider(conn: &Connection, id: i64) -> Result<()> {
    let tx = crate::db::write_transaction(conn)?;
    delete_provider_in_transaction(&tx, id)?;
    tx.commit()?;
    Ok(())
}

/// 让应用层同时提交配置代际；用途、线程和连接仍在同一事务内修改。
pub fn delete_provider_in_transaction(tx: &rusqlite::Transaction<'_>, id: i64) -> Result<()> {
    tx.execute(
        "UPDATE task_bindings SET provider_id = NULL, model = NULL WHERE provider_id = ?1",
        [id],
    )?;
    tx.execute(
        "UPDATE threads SET chat_provider_id = NULL, chat_model = NULL WHERE chat_provider_id = ?1",
        [id],
    )?;
    let n = tx.execute("DELETE FROM ai_providers WHERE id = ?1", [id])?;
    if n == 0 {
        Err(Error::ProviderNotFound)
    } else {
        Ok(())
    }
}

pub fn get_provider(conn: &Connection, id: i64) -> Result<AiProvider> {
    conn.query_row(
        "SELECT id, name, base_url, api_key, kind, protocol, last_test_at, last_test_ok, last_test_note
         FROM ai_providers WHERE id = ?1",
        [id],
        map_provider,
    )
    .optional()?
    .ok_or(Error::ProviderNotFound)
}

pub fn list_providers(conn: &Connection) -> Result<Vec<AiProvider>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, base_url, api_key, kind, protocol, last_test_at, last_test_ok, last_test_note
         FROM ai_providers ORDER BY id",
    )?;
    let rows = stmt.query_map([], map_provider)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn map_provider(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiProvider> {
    Ok(AiProvider {
        id: row.get(0)?,
        name: row.get(1)?,
        base_url: row.get(2)?,
        api_key: row.get(3)?,
        kind: row.get(4)?,
        protocol: row
            .get::<_, Option<String>>(5)?
            .unwrap_or_else(|| "chat".into()),
        last_test: match row.get::<_, Option<i64>>(6)? {
            Some(at) => Some(ProviderTest {
                at,
                ok: row.get::<_, Option<bool>>(7)?.unwrap_or(false),
                note: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
            }),
            None => None,
        },
    })
}

pub fn get_template_by_slug(conn: &Connection, slug: &str) -> Result<PromptTemplate> {
    conn.query_row(
        "SELECT id, slug, title, body, version FROM prompt_templates WHERE slug = ?1",
        [slug],
        map_template,
    )
    .optional()?
    .ok_or_else(|| Error::TemplateNotFound(slug.into()))
}

fn map_template(row: &rusqlite::Row<'_>) -> rusqlite::Result<PromptTemplate> {
    Ok(PromptTemplate {
        id: row.get(0)?,
        slug: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        version: row.get(4)?,
    })
}

pub fn set_task_binding(
    conn: &Connection,
    task: &str,
    provider_id: Option<i64>,
    model: Option<&str>,
) -> Result<()> {
    if !SEED_BINDINGS.iter().any(|(t, _)| *t == task) && task != TASK_EMBED && task != TASK_CHAT {
        return Err(Error::UnknownAiTask(task.into()));
    }
    crate::db::atomic_write(conn, |conn| {
        if task == TASK_EMBED {
            crate::vector_space::prepare_binding(conn, provider_id, model)?;
        }
        let n = conn.execute(
            "UPDATE task_bindings SET provider_id = ?1, model = ?2 WHERE task = ?3",
            params![provider_id, model, task],
        )?;
        if n == 0 {
            Err(Error::UnknownAiTask(task.into()))
        } else {
            Ok(())
        }
    })
}

pub fn get_task_binding(conn: &Connection, task: &str) -> Result<TaskBinding> {
    conn.query_row(
        "SELECT task, provider_id, template_id, model FROM task_bindings WHERE task = ?1",
        [task],
        map_binding,
    )
    .optional()?
    .ok_or_else(|| Error::UnknownAiTask(task.into()))
}

fn map_binding(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskBinding> {
    Ok(TaskBinding {
        task: row.get(0)?,
        provider_id: row.get(1)?,
        template_id: row.get(2)?,
        model: row.get(3)?,
    })
}

pub fn list_task_bindings(conn: &Connection) -> Result<Vec<TaskBinding>> {
    let mut stmt = conn
        .prepare("SELECT task, provider_id, template_id, model FROM task_bindings ORDER BY task")?;
    let rows = stmt.query_map([], map_binding)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn resolve_task(conn: &Connection, task: &str) -> Result<ResolvedTask> {
    let binding = get_task_binding(conn, task)?;
    let template = match binding.template_id {
        Some(id) => conn
            .query_row(
                "SELECT id, slug, title, body, version FROM prompt_templates WHERE id = ?1",
                [id],
                map_template,
            )
            .optional()?
            .ok_or_else(|| Error::TemplateNotFound(task.into()))?,
        None => return Err(Error::TemplateNotFound(task.into())),
    };
    let (provider, model) = resolve_binding_model(conn, &binding)?
        .ok_or_else(|| Error::TaskModelUnset(TaskModelLabel::Id(task.into())))?;
    Ok(ResolvedTask {
        task: task.to_string(),
        template,
        provider,
        model,
    })
}

/// 嵌入用途没选模型时为 `None`，嵌入作业据此跳过。
pub fn resolve_embed(conn: &Connection) -> Result<Option<EmbedEndpoint>> {
    let binding = match get_task_binding(conn, TASK_EMBED) {
        Ok(b) => b,
        Err(Error::UnknownAiTask(_)) => return Ok(None),
        Err(err) => return Err(err),
    };
    let Some((provider, model)) = resolve_binding_model(conn, &binding)? else {
        return Ok(None);
    };
    if provider.base_url.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(EmbedEndpoint {
        base_url: provider.base_url,
        api_key: provider.api_key,
        model: model.trim().to_string(),
    }))
}

pub fn hash_input(payload: &str) -> String {
    sha256_hex(&[payload])
}

pub fn cache_key(task: &str, model: &str, template_version: i64, input_hash: &str) -> String {
    sha256_hex(&[task, model, &template_version.to_string(), input_hash])
}

pub fn cache_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM ai_cache WHERE cache_key = ?1",
        [key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn cache_put(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO ai_cache (cache_key, value, created_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (cache_key) DO UPDATE SET value = excluded.value, created_at = excluded.created_at",
        params![key, value, now_ms()],
    )?;
    Ok(())
}

pub fn cache_delete(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM ai_cache WHERE cache_key = ?1", [key])?;
    Ok(())
}

pub fn local_day() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub fn daily_budget_tokens(conn: &Connection) -> Result<i64> {
    GENERATION_BUDGET.int(conn)
}

pub fn embed_daily_budget_tokens(conn: &Connection) -> Result<i64> {
    EMBED_BUDGET.int(conn)
}

pub fn set_daily_budget_tokens(conn: &Connection, tokens: i64) -> Result<()> {
    set_setting(conn, AI_DAILY_BUDGET_TOKENS_KEY, &tokens.to_string())
}

pub fn usage_on(conn: &Connection, day: &str) -> Result<UsageDay> {
    Ok(conn
        .query_row(
            "SELECT tokens, prompt_tokens, completion_tokens, embed_tokens, auto_summaries, auto_cluster_summaries FROM ai_usage WHERE day = ?1",
            [day],
            |row| {
                Ok(UsageDay {
                    tokens: row.get(0)?,
                    prompt_tokens: row.get(1)?,
                    completion_tokens: row.get(2)?,
                    embed_tokens: row.get(3)?,
                    auto_summaries: row.get(4)?,
                    auto_cluster_summaries: row.get(5)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

pub fn add_usage(
    conn: &Connection,
    day: &str,
    prompt_tokens: i64,
    completion_tokens: i64,
) -> Result<UsageDay> {
    let tokens = prompt_tokens.saturating_add(completion_tokens);
    conn.execute(
        "INSERT INTO ai_usage (day, tokens, prompt_tokens, completion_tokens)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (day) DO UPDATE SET
           tokens = tokens + excluded.tokens,
           prompt_tokens = prompt_tokens + excluded.prompt_tokens,
           completion_tokens = completion_tokens + excluded.completion_tokens",
        params![day, tokens, prompt_tokens, completion_tokens],
    )?;
    usage_on(conn, day)
}

pub fn add_embed_usage(conn: &Connection, day: &str, tokens: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO ai_usage(day,embed_tokens) VALUES(?1,?2)
        ON CONFLICT(day) DO UPDATE SET embed_tokens=embed_tokens+excluded.embed_tokens",
        params![day, tokens],
    )?;
    Ok(())
}

pub fn focus_summary_slots(conn: &Connection) -> Result<i64> {
    if !FOCUS_AUTO_SUMMARY.enabled(conn)? {
        return Ok(0);
    }
    Ok((FOCUS_DAILY_LIMIT - usage_on(conn, &local_day())?.auto_summaries).max(0))
}

pub fn cluster_summary_slots(conn: &Connection) -> Result<i64> {
    if !CLUSTER_AUTO_SUMMARY.enabled(conn)? {
        return Ok(0);
    }
    Ok((CLUSTER_DAILY_LIMIT - usage_on(conn, &local_day())?.auto_cluster_summaries).max(0))
}

/// 每次自动请求前核对对应预算；0 表示不限额。
pub fn ensure_auto_job_budget(conn: &Connection, kind: BudgetKind) -> Result<()> {
    let budget = match kind {
        BudgetKind::Generation => daily_budget_tokens(conn)?,
        BudgetKind::Embedding => embed_daily_budget_tokens(conn)?,
    };
    if budget <= 0 {
        return Ok(());
    }
    let usage = usage_on(conn, &local_day())?;
    let used = match kind {
        BudgetKind::Generation => usage.tokens,
        BudgetKind::Embedding => usage.embed_tokens,
    };
    if used >= budget {
        tracing::debug!(
            used,
            budget,
            error_kind = "budget",
            "daily ai budget exhausted; skip auto job"
        );
        Err(Error::DailyBudgetExhausted)
    } else {
        Ok(())
    }
}

pub fn summary_payload(
    title: &str,
    blocks_json: Option<&str>,
    content_html: Option<&str>,
    high_priority: bool,
    tier: InputTier,
) -> String {
    crate::summary_extract::extract_summary_payload(
        title,
        blocks_json,
        content_html,
        high_priority,
        tier,
    )
}

/// 输入档位；缺省或无法识别时按「中」。
pub fn input_tier(conn: &Connection) -> Result<InputTier> {
    Ok(get_setting(conn, AI_INPUT_TIER_KEY)?
        .and_then(|s| InputTier::parse(&s))
        .unwrap_or_default())
}

pub fn set_input_tier(conn: &Connection, tier: InputTier) -> Result<()> {
    set_setting(conn, AI_INPUT_TIER_KEY, tier.as_str())
}

fn sha256_hex(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            hasher.update(b"|");
        }
        hasher.update(part.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
}

pub use crate::digest::{
    DigestDeepInput, DigestItem, DigestRow, DigestSource, DigestWindow, WEEKLY_WINDOW_MS,
    align_digest_source_slots, align_digest_sources, apply_digest_picks,
    build_digest_items_json_strict, count_digest_list_items, digest_by_id, digest_candidates,
    digest_date_label, digest_generate_due, digest_generate_minutes, digest_items_by_ids,
    digest_pack_input, format_digest_catalog, format_digest_deep_input, format_digest_input,
    format_digest_items, format_weekly_input, insert_digest, latest_digest, list_digests,
    one_liner, parse_digest_item_ns, parse_digest_items_json, parse_digest_picks,
    parse_summary_details, parse_summary_details_for_digest, period_start_ms,
    sanitize_digest_blurb, sanitize_digest_body, sanitize_digest_body_with_item_refs,
    serialize_digest_items, serialize_digest_sources, set_digest_generate_minutes,
};

pub use crate::digest_sources::{
    DailyDigestLine, canonical_digest_items, decode_digest_sources, digest_revision,
    prepare_digest_draft, week_daily_lines,
};

pub use crate::summary_extract::InputTier;
pub use crate::text::truncate_chars;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{PooledConnection, open_at};
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn compat_defaults_to_an_empty_object_and_only_accepts_objects() {
        let (_dir, conn) = open_temp();
        let id = insert_provider(
            &conn,
            &NewProvider {
                name: "p".into(),
                base_url: "http://p/v1".into(),
                api_key: None,
                protocol: "chat".into(),
            },
        )
        .unwrap();
        assert_eq!(provider_compat(&conn, id).unwrap(), serde_json::json!({}));
        let compat = serde_json::json!({"supportsDeveloperRole": false});
        set_provider_compat(&conn, id, &compat).unwrap();
        assert_eq!(provider_compat(&conn, id).unwrap(), compat);
        assert!(matches!(
            set_provider_compat(&conn, id, &serde_json::json!([1])),
            Err(Error::InvalidCompat(_))
        ));
        assert_eq!(
            provider_compat(&conn, id).unwrap(),
            compat,
            "kept on rejection"
        );
        // 手改坏的值按空对象读。
        conn.execute(
            "UPDATE ai_providers SET compat_json = 'nope' WHERE id = ?1",
            [id],
        )
        .unwrap();
        assert_eq!(provider_compat(&conn, id).unwrap(), serde_json::json!({}));
        assert!(matches!(
            provider_compat(&conn, id + 1),
            Err(Error::ProviderNotFound)
        ));
    }

    #[test]
    fn seeds_templates_and_bindings() {
        let (_dir, conn) = open_temp();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompt_templates", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, SEED_TEMPLATES.len() as i64);
        let binding = get_task_binding(&conn, TASK_FOCUS_SUMMARY).unwrap();
        assert!(binding.template_id.is_some());
        assert!(matches!(
            resolve_task(&conn, TASK_FOCUS_SUMMARY),
            Err(Error::TaskModelUnset(TaskModelLabel::Id(task))) if task == TASK_FOCUS_SUMMARY
        ));
        let embed = get_task_binding(&conn, TASK_EMBED).unwrap();
        assert!(embed.template_id.is_none());
        assert!(resolve_embed(&conn).unwrap().is_none());
    }

    #[test]
    fn a_new_cluster_summary_binding_starts_from_the_article_summary_model() {
        let (_dir, conn) = open_temp();
        let pid = insert_provider(
            &conn,
            &NewProvider {
                name: "p".into(),
                base_url: "https://model.invalid".into(),
                api_key: None,
                protocol: "openai".into(),
            },
        )
        .unwrap();
        set_task_binding(&conn, TASK_ARTICLE_SUMMARY, Some(pid), Some("m")).unwrap();
        // 老库升级：综述的绑定还不存在，补种子时沿用本篇摘要的模型。
        conn.execute(
            "DELETE FROM task_bindings WHERE task = ?1",
            [TASK_CLUSTER_SUMMARY],
        )
        .unwrap();
        seed_defaults(&conn).unwrap();
        let resolved = resolve_task(&conn, TASK_CLUSTER_SUMMARY).unwrap();
        assert_eq!((resolved.provider.id, resolved.model.as_str()), (pid, "m"));
        assert_eq!(resolved.template.slug, SLUG_CLUSTER_SUMMARY);
    }

    fn provider(conn: &Connection, name: &str) -> i64 {
        insert_provider(
            conn,
            &NewProvider {
                name: name.into(),
                base_url: format!("http://{name}/v1"),
                api_key: None,
                protocol: "chat".into(),
            },
        )
        .unwrap()
    }

    #[test]
    fn a_use_runs_only_on_its_own_explicit_model() {
        let (_dir, conn) = open_temp();
        let (a, b) = (provider(&conn, "a"), provider(&conn, "b"));
        set_task_binding(&conn, TASK_FOCUS_SUMMARY, Some(a), Some(" qwen2.5 ")).unwrap();
        let resolved = resolve_task(&conn, TASK_FOCUS_SUMMARY).unwrap();
        assert_eq!(
            (resolved.provider.id, resolved.model.as_str()),
            (a, "qwen2.5")
        );
        // 没有主模型可以继承：其他用途仍未选择，连接没有模型也不补。
        assert!(matches!(
            resolve_task(&conn, TASK_DIGEST_DAILY),
            Err(Error::TaskModelUnset(_))
        ));
        set_task_binding(&conn, TASK_ARTICLE_SUMMARY, Some(a), None).unwrap();
        assert!(resolve_task(&conn, TASK_ARTICLE_SUMMARY).is_err());
        assert!(resolve_embed(&conn).unwrap().is_none());
        set_task_binding(&conn, TASK_EMBED, Some(b), Some("nomic-embed-text")).unwrap();
        let ep = resolve_embed(&conn).unwrap().unwrap();
        assert_eq!(
            (ep.model.as_str(), ep.base_url.as_str()),
            ("nomic-embed-text", "http://b/v1")
        );
        set_task_binding(&conn, TASK_CHAT, Some(b), Some("chat-model")).unwrap();
        assert_eq!(
            chat_default(&conn).unwrap(),
            Some(crate::model_pool::ModelRef::new(b, "chat-model"))
        );
    }

    #[test]
    fn deleting_a_connection_unsets_its_uses_and_thread_choices() {
        let (_dir, conn) = open_temp();
        let (a, b) = (provider(&conn, "a"), provider(&conn, "b"));
        set_task_binding(&conn, TASK_CHAT, Some(a), Some("x")).unwrap();
        set_task_binding(&conn, TASK_FOCUS_SUMMARY, Some(b), Some("y")).unwrap();
        conn.execute(
            "INSERT INTO threads (id, created_at, updated_at, chat_provider_id, chat_model) VALUES (1, 0, 0, ?1, 'x'), (2, 0, 0, ?2, 'y')",
            params![a, b],
        )
        .unwrap();
        delete_provider(&conn, a).unwrap();
        let chat = get_task_binding(&conn, TASK_CHAT).unwrap();
        assert_eq!((chat.provider_id, chat.model), (None, None));
        assert_eq!(chat_default(&conn).unwrap(), None);
        assert_eq!(
            resolve_task(&conn, TASK_FOCUS_SUMMARY).unwrap().provider.id,
            b
        );
        let threads: Vec<Option<i64>> = conn
            .prepare("SELECT chat_provider_id FROM threads ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(threads, [None, Some(b)]);
        assert!(matches!(
            get_provider(&conn, a),
            Err(Error::ProviderNotFound)
        ));
        assert!(matches!(
            delete_provider(&conn, a),
            Err(Error::ProviderNotFound)
        ));
    }

    #[test]
    fn the_last_test_is_remembered_on_the_connection() {
        let (_dir, conn) = open_temp();
        let a = provider(&conn, "a");
        assert_eq!(get_provider(&conn, a).unwrap().last_test, None);
        set_provider_test(&conn, a, false, "服务返回 401").unwrap();
        let test = get_provider(&conn, a).unwrap().last_test.unwrap();
        assert!(!test.ok && test.note == "服务返回 401" && test.at > 0);
        assert!(set_provider_test(&conn, a + 1, true, "").is_err());
    }

    #[test]
    fn cache_roundtrip_and_stable_key() {
        let (_dir, conn) = open_temp();
        let key = cache_key(TASK_FOCUS_SUMMARY, "m", 1, &hash_input("hello"));
        assert_eq!(
            key,
            cache_key(TASK_FOCUS_SUMMARY, "m", 1, &hash_input("hello"))
        );
        assert_ne!(
            key,
            cache_key(TASK_FOCUS_SUMMARY, "m", 2, &hash_input("hello"))
        );
        assert!(cache_get(&conn, &key).unwrap().is_none());
        cache_put(&conn, &key, r#"{"summary":"cached"}"#).unwrap();
        assert_eq!(
            cache_get(&conn, &key).unwrap().as_deref(),
            Some(r#"{"summary":"cached"}"#)
        );
    }

    #[test]
    fn digest_seed_upgrade_bumps_version_once_and_invalidates_completion_cache() {
        // 只使用内存库，不接触用户库或模型端点。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE prompt_templates (
                 id INTEGER PRIMARY KEY, slug TEXT NOT NULL UNIQUE,
                 title TEXT NOT NULL, body TEXT NOT NULL, version INTEGER NOT NULL
             );
             CREATE TABLE task_bindings (
                 task TEXT PRIMARY KEY, provider_id INTEGER,
                 template_id INTEGER, model TEXT
             );
             CREATE TABLE ai_cache (
                 cache_key TEXT PRIMARY KEY, value TEXT NOT NULL,
                 created_at INTEGER NOT NULL
             );",
        )
        .unwrap();
        seed_defaults(&conn).unwrap();

        for (task, slug) in [
            (TASK_DIGEST_DAILY, SLUG_DAILY_DIGEST),
            (TASK_DIGEST_WEEKLY, SLUG_WEEKLY_DIGEST),
        ] {
            let fresh = get_template_by_slug(&conn, slug).unwrap();
            assert_eq!(fresh.version, 1);
            assert!(fresh.body.contains("sections"));
            assert!(fresh.body.contains("refs"));
            assert!(!fresh.body.contains("只输出 JSON：{\"body\""));

            // 模拟升级前已经被旧模板生成并缓存的同任务、同模型、同输入响应。
            conn.execute(
                "UPDATE prompt_templates SET body = ?1, version = 7 WHERE slug = ?2",
                params!["旧式 body/item_refs 模板", slug],
            )
            .unwrap();
            let old = get_template_by_slug(&conn, slug).unwrap();
            let input_hash = hash_input("same digest input");
            let old_key = cache_key(task, "same-model", old.version, &input_hash);
            cache_put(&conn, &old_key, "old completion").unwrap();

            seed_defaults(&conn).unwrap();
            let upgraded = get_template_by_slug(&conn, slug).unwrap();
            assert_eq!(upgraded.body, fresh.body);
            assert_eq!(upgraded.version, old.version + 1);
            let new_key = cache_key(task, "same-model", upgraded.version, &input_hash);
            assert_ne!(new_key, old_key);
            assert!(cache_get(&conn, &new_key).unwrap().is_none());
            assert_eq!(
                cache_get(&conn, &old_key).unwrap().as_deref(),
                Some("old completion")
            );

            seed_defaults(&conn).unwrap();
            let repeated = get_template_by_slug(&conn, slug).unwrap();
            assert_eq!(repeated.version, upgraded.version);
            assert_eq!(
                cache_key(task, "same-model", repeated.version, &input_hash),
                new_key
            );
        }
    }

    #[test]
    fn budgets_are_independent_and_zero_is_unlimited() {
        for exhausted in [BudgetKind::Generation, BudgetKind::Embedding] {
            let (_dir, conn) = open_temp();
            assert_eq!(daily_budget_tokens(&conn).unwrap(), 200000);
            assert_eq!(embed_daily_budget_tokens(&conn).unwrap(), 1000000);
            set_setting(&conn, GENERATION_BUDGET.id, "10").unwrap();
            set_setting(&conn, EMBED_BUDGET.id, "10").unwrap();
            let other = match exhausted {
                BudgetKind::Generation => {
                    add_usage(&conn, &local_day(), 8, 2).unwrap();
                    BudgetKind::Embedding
                }
                BudgetKind::Embedding => {
                    add_embed_usage(&conn, &local_day(), 10).unwrap();
                    BudgetKind::Generation
                }
            };
            assert!(matches!(
                ensure_auto_job_budget(&conn, exhausted),
                Err(Error::DailyBudgetExhausted)
            ));
            ensure_auto_job_budget(&conn, other).unwrap();
            set_setting(&conn, GENERATION_BUDGET.id, "0").unwrap();
            set_setting(&conn, EMBED_BUDGET.id, "0").unwrap();
            ensure_auto_job_budget(&conn, exhausted).unwrap();
            ensure_auto_job_budget(&conn, other).unwrap();
        }
    }

    #[test]
    fn summary_payload_strips_html_and_truncates() {
        let payload = summary_payload(
            "Title",
            None,
            Some(&format!("<p>{}</p>", "字".repeat(2000))),
            false,
            InputTier::Medium,
        );
        assert!(payload.starts_with("Title\n\n"));
        assert!(!payload.contains('<'));
        assert!(payload.chars().count() <= InputTier::Medium.summary_budget(false));
    }

    #[test]
    fn input_tier_defaults_to_medium_and_round_trips() {
        let (_dir, conn) = open_temp();
        assert_eq!(input_tier(&conn).unwrap(), InputTier::Medium);
        set_input_tier(&conn, InputTier::High).unwrap();
        assert_eq!(input_tier(&conn).unwrap(), InputTier::High);
        set_setting(&conn, AI_INPUT_TIER_KEY, "huge").unwrap();
        assert_eq!(input_tier(&conn).unwrap(), InputTier::Medium);
    }
}
