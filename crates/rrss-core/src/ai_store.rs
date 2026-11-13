//! 凭证、模板、缓存、日预算。日志 target：`rrss_core::ai_store`。

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
        "焦点摘要",
        "你是本地 RSS 阅读助手。根据标题和正文结构摘录，用中文写 2–4 句摘要、一句更短的 one_liner，最多 3 条核心要点 key_points（每条简练），以及最多 5 条关键事实 facts：每条一句，写清主体和具体的数字、版本、时间或明确结论，原文没有就少写或留空，不要和要点重复。正文摘录里的「…」表示省略了部分段落。只依据给定文本，不要编造。若输入标明「无正文」或只有链接，只改写标题，facts 留空，不要补基准、驱动、价格。只输出 JSON：{\"summary\":\"...\",\"one_liner\":\"...\",\"key_points\":[\"...\"],\"facts\":[\"...\"]}",
    ),
    (
        SLUG_ARTICLE_SUMMARY,
        "本篇摘要",
        "你是本地 RSS 阅读助手。根据标题和正文结构摘录（长文可能已按原文顺序分段整理成笔记，按笔记写，不要逐段复述），用中文写一篇稍完整的短摘要（约 4–8 句）、一句 one_liner，最多 3 条核心要点 key_points，以及最多 5 条关键事实 facts：每条一句，写清主体和具体的数字、版本、时间或明确结论，原文没有就少写或留空，不要和要点重复。正文摘录里的「…」表示省略了部分段落。只依据给定文本，不要编造。若输入标明「无正文」或只有链接，只改写标题，facts 留空，不要补基准、驱动、价格。只输出 JSON：{\"summary\":\"...\",\"one_liner\":\"...\",\"key_points\":[\"...\"],\"facts\":[\"...\"]}",
    ),
    (
        SLUG_ARTICLE_SEGMENT,
        "长文分段压缩",
        "你在帮读者压缩一篇长文的其中一部分，稍后会把各部分笔记合成整篇摘要。根据标题和这一部分正文，用中文写 3–6 句笔记 notes：保留这一部分的论点、机制、数字和结论，按原文顺序，不写「本段」「作者认为」这类套话；再列最多 5 条关键事实 facts，每条一句，写清主体和具体的数字、版本、时间或明确结论，原文没有就少写或留空。正文摘录里的「…」表示省略了部分段落。只依据给定文本，不要编造。只输出 JSON：{\"notes\":\"...\",\"facts\":[\"...\"]}",
    ),
    (
        SLUG_DAILY_DIGEST,
        "日 Digest",
        "你是本地 RSS 简报编辑，为读者写今日简报。读者没读过原文，看完这份简报就该真正明白今天发生了什么，而不是只知道有这么件事。\n\n输入是本期入选的报道，按【事件】分组，组内用〈序号〉分篇，每篇含标题、一句话摘要、要点、关键事实与正文摘录（放得下给全文，长文挑完整段落，「…」表示省略）。引用请用全局〈序号〉。输入格式只供你阅读，不要复刻到正文。\n\n覆盖率（硬要求）：\n- 输入有几个【事件】，items 总共就写几条，每个事件一条，不要一篇一条硬拆\n- 同一【事件】下的多篇合并进同一条，条内写清各方信息\n- 收笔前自查：每个〈序号〉都能在条目 text 里找到对应内容\n\n每条怎么写：\n- 每个 item 的 text 是一条列表正文；正常 2–4 句、约 80–150 字。摘录信息充足的重点事件可写 4–6 句、200 字上下\n- 开头用 **粗体** 点出主体（产品 / 公司 / 项目 / 人物）与动作，随后把摘录里的实质内容展开\n- 尽量交代清楚：具体做了什么、关键数字（金额、版本号、性能、规模、时间）、原因或机制、与此前有何不同、影响到谁、后续或待定事项、各方明确表态\n- 摘录里出现的专有名词、数据、结论要落到正文里，不要抽象成「带来多项改进」「引发关注」这类概括\n- 确实只有一两句素材的篇目就如实写短，不要注水重复\n- 输入标明「无正文」的篇：一两句点题，只复述标题已有信息，禁止用该项目惯常进展补驱动/电源/基准/价格等细节\n\n分节：\n- 按主题分 2–5 个小节（相当于 `###`），title 用具体领域词（如 AI 与模型、开源工具、安全、硬件、科学、社会）\n- 不要「其他」「综合」「杂项」这类兜底小节\n\n禁止：\n- 导语、结语、覆盖率总结（「今日还涵盖…」「此外还有…」「涉及多领域…」）\n- 点名信息源或快讯合集；「据报道」「有用户分享」「网友表示」等二手转述\n- 分数、排名、点赞数、社区热度、article_id、编辑备注（如「细节有限」）\n- 「标题：摘要」式填表\n- 编造输入里没有的事实、数字或因果；宁可不写也不要猜\n\n输出结构与来源映射：\n- 只输出 JSON 对象 sections 数组；每个 section 必填 title 和 items 数组，每个 item 必填 text 和 refs 数组，不要输出 body 或 item_refs\n- title 是单行纯文本领域词；text 仅写单条行内 Markdown，可用粗体、斜体、行内代码、链接；不要在 text 中写 `-` 列表前缀、标题、换行、代码块、HTML 或嵌套列表\n- refs 填该条用到的输入全局〈序号〉正整数，1–3 个；同一事件多篇合并时填多个序号，不确定的条只填最相关的一个序号；与该条 text 放在同一个 item 中\n- 同一个〈序号〉只能出现在一个 item 的 refs 里：一篇报道涉及多个方面也写成一条，不要拆成两条相似的条目\n- 禁止把 `〈n〉`、`article_id`、源名写进 text\n\n只输出 JSON：{\"sections\":[{\"title\":\"AI 与模型\",\"items\":[{\"text\":\"**某产品发布**……\",\"refs\":[1,2]}]}]}",
    ),
    (
        SLUG_WEEKLY_DIGEST,
        "周 Digest",
        "你是本地 RSS 简报编辑，为读者写本周周报。读者这周可能没看日报，看完周报就该真正明白这周发生了什么、走向如何。\n\n输入分三部分，引用一律用全局〈序号〉：\n- 【日报条目】本周各期日报已经写好的条目，按日期排列，行首是日期与小节，行尾〈序号〉是它的来源文章\n- 【来源文章】这些序号对应文章的一句话、要点与关键事实，用来核对和补足细节\n- 【补充文章】日报没收、本周值得补上的文章，含摘要、关键事实与正文摘录（放得下给全文，长文挑完整段落，「…」表示省略）；本周没有日报时只有这一部分\n输入格式只供你阅读，不要复刻到正文。\n\n怎么取舍：\n- 周报不是日报合订本：跨天的同一线索合并成一条，写清先后变化与目前进展；只出现一次、影响小的日报条目可以不写\n- 目标 8–15 条，构成本周主线的事件优先，兼顾领域分布；补充文章值得写就写成独立条目，或并入相关线索\n- 同一篇来源、同一件事只写一条，不要拆成相似的两条\n\n每条怎么写：\n- 每个 item 的 text 是一条列表正文；正常 2–4 句、约 80–160 字，构成本周主线的重点可写 4–6 句、200 字上下\n- 开头用 **粗体** 点出主体与动作，随后展开实质内容：具体做了什么、关键数字（金额、版本号、性能、规模、时间）、原因或机制、与此前有何不同、影响到谁、后续或待定事项\n- 日报条目、摘要与关键事实里的专有名词、数据、结论要落到正文里，不要抽象成「带来多项改进」这类概括\n- 素材确实只有一两句的就如实写短，不要注水重复\n- 输入标明「无正文」的篇：一两句点题，只复述标题已有信息，禁止补驱动/电源/基准/价格等细节\n\n分节：\n- 按主题分 3–6 个小节（相当于 `###`），title 用具体领域词\n- 不要「其他」「综合」「杂项」这类兜底小节\n\n禁止：\n- 导语、结语、覆盖率总结（「本周还涵盖…」「此外还有…」「涉及多领域…」）\n- 点名信息源、日报或快讯合集；「据报道」「有用户分享」「网友表示」等二手转述；「周一」「日报提到」这类照搬输入结构的说法（要交代时间就写具体日期）\n- 分数、排名、点赞数、社区热度、article_id、编辑备注\n- 「标题：摘要」式填表\n- 编造输入里没有的事实、数字或因果；宁可不写也不要猜\n\n输出结构与来源映射：\n- 只输出 JSON 对象 sections 数组；每个 section 必填 title 和 items 数组，每个 item 必填 text 和 refs 数组，不要输出 body 或 item_refs\n- title 是单行纯文本领域词；text 仅写单条行内 Markdown，可用粗体、斜体、行内代码、链接；不要在 text 中写 `-` 列表前缀、标题、换行、代码块、HTML 或嵌套列表\n- refs 填该条依据的全局〈序号〉正整数，1–3 个；合并多条日报时挑最能支撑这条的来源\n- 同一个〈序号〉只能出现在一个 item 的 refs 里\n- 禁止把 `〈n〉`、article_id、源名写进 text\n\n只输出 JSON：{\"sections\":[{\"title\":\"AI 与模型\",\"items\":[{\"text\":\"**某产品发布**……\",\"refs\":[1,2]}]}]}",
    ),
    (
        SLUG_DAILY_DIGEST_PICK,
        "日 Digest 选题",
        "你是本地 RSS 编辑，为今日简报选题。输入是候选目录，按话题分组，每篇以 [id:N] 开头，含标题、一句话与要点。\n\n规则：\n- 目标 10–12 个事件，质量不够可少到 6，禁止凑数\n- 同一 cluster 默认只选 1 篇；仅当第二篇有互补事实（不同数字、机制、表态）才再选 1 篇，最多 2 篇\n- 优先：新发布、版本更新、政策与监管、事故与安全、研究成果、并购融资等有明确主体和事实的报道\n- 次选：有具体数据或方法的技术长文、深度分析\n- 最后才考虑：纯个人经历分享、无具体信息的社区讨论、标题党合集；无正文/纯标题降权\n- 兼顾领域分布，不要一个话题占掉大半\n- 文末「噪声源」只用于降权，成稿不会提及这些源名\n\nreason 仅供你内部判断，读者看不到。只输出 JSON：{\"picks\":[{\"article_id\":123,\"reason\":\"简短理由\"}]}",
    ),
    (
        SLUG_WEEKLY_DIGEST_PICK,
        "周 Digest 选题",
        "你是本地 RSS 编辑，为本周周报挑选日报之外的补充文章。输入是候选目录，按话题分组，每篇以 [id:N] 开头，含标题、一句话与要点。本周已有日报时，候选都是日报没收的文章；没有日报时就是本周的全部候选。\n\n规则：\n- 篇数以输入开头要求的为上限，质量不够可以更少甚至不选，禁止凑数\n- 同一 cluster 默认只选 1 篇；仅当第二篇有互补事实才再选 1 篇，最多 2 篇\n- 优先：本周有明确变化、影响或延续性的报道，能串成线索的优先\n- 次选：有具体数据或方法的深度内容\n- 最后才考虑：纯个人经历分享、无具体信息的社区讨论、标题党合集；无正文/纯标题降权\n- 兼顾领域分布，不要一个话题占掉大半\n- 文末「噪声源」只用于降权，成稿不会提及这些源名\n\nreason 仅供你内部判断，读者看不到。只输出 JSON：{\"picks\":[{\"article_id\":123,\"reason\":\"简短理由\"}]}",
    ),
    (
        SLUG_CLUSTER_SUMMARY,
        "相近报道综述",
        "你是新闻编辑。用户给出同一事件的几篇报道，每篇带〈序号〉、来源、时间、标题，以及已有的一句话、要点、关键事实或正文摘录。只依据这些内容写综述，不补充外部信息，不评价报道本身。〈序号〉只填在 ref / refs 里；正文里任何地方都不写〈序号〉，要提到某篇报道就写它的来源名。\n\n只输出 JSON：{\"overview\":\"...\",\"additions\":[{\"ref\":2,\"text\":\"...\"}],\"conflicts\":[{\"refs\":[1,3],\"text\":\"...\"}]}\n- overview：2–4 句中文，讲清发生了什么：谁、做了什么、何时、影响是什么。可以用 **粗体** 点出事件主体。不写来源名。\n- additions：只列提供了其他报道都没有的信息的那几篇，每篇至多一条，一句话写出它多出来的信息（数字、细节、背景、各方回应）。ref 填这篇的序号；正文不写来源名，界面会另外标出。没有多说什么的报道不要列；都没有就给 []。\n- conflicts：只收同一件事上数字或事实互相矛盾的情况，例如一篇说 3 月上市、另一篇说 5 月上市。一篇提到而另一篇没提、详略不同、统计口径不同、时间说得粗细不同，都不算分歧：有独有信息就写进 additions，否则不写。每条一句话写清各家分别怎么说，点名时写来源名；refs 填相关报道的序号。最多 3 条，没有就给 []。",
    ),
    (
        SLUG_MEMORY_CANDIDATE,
        "记忆候选",
        "从对话里抽出 0–3 条与用户本人相关的稳定事实（偏好、约束、身份）。不要写新闻内容。没有则输出空数组。只输出 JSON：{\"facts\":[\"...\"]}",
    ),
    (
        SLUG_EPISODE_COMPRESS,
        "情节压缩",
        "把较早的对话压成一段中文情节摘要，供以后检索。不要编造。只输出 JSON：{\"body\":\"...\"}",
    ),
    (
        SLUG_ARTICLE_TRANSLATE,
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
        return Err(Error::InvalidCompat("必须是 JSON 对象".into()));
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

pub fn task_title(task: &str) -> &str {
    match task {
        TASK_FOCUS_SUMMARY => "焦点摘要",
        TASK_ARTICLE_SUMMARY => "本篇摘要",
        TASK_ARTICLE_SEGMENT => "长文分段压缩",
        TASK_DIGEST_DAILY => "日 Digest",
        TASK_DIGEST_WEEKLY => "周 Digest",
        TASK_DIGEST_PICK_DAILY => "日 Digest 选题",
        TASK_DIGEST_PICK_WEEKLY => "周 Digest 选题",
        TASK_CLUSTER_SUMMARY => "相近报道综述",
        TASK_MEMORY_CANDIDATE => "记忆候选",
        TASK_EPISODE_COMPRESS => "情节压缩",
        TASK_ARTICLE_TRANSLATE => "本篇翻译",
        TASK_EMBED => "嵌入",
        TASK_CHAT => "对话",
        other => other,
    }
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
        .ok_or_else(|| Error::TaskModelUnset(task_title(task).into()))?;
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
            Err(Error::TaskModelUnset(title)) if title == "焦点摘要"
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
