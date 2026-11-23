import type { ArticleView, ChatMessageView, ClusterArticleView, ClusterSummaryView, DigestSourceView, DigestView, FeedTree, FeedView, ReaderView, RulesPage } from "../src/bindings";
import { threadFixture } from "../src/test/fixtures/chat";
import { MOCK_FOCUS, MOCK_RULES, type MockHostOptions } from "./mockHost";

// README 截图的演示数据：虚构的来源与 `.example` 域名，中英文各一份，结构相同、内容贴近日常科技与博客阅读。
// 界面文字由语言包给出；这里只有内容（标题、摘要、正文、对话）和截图脚本要点击的界面名称。

export type DemoLanguage = "zh-CN" | "en";

type Copy = {
  /** 截图脚本按名称点击的界面元素，取自对应语言包。 */
  ui: { digest: string; feeds: RegExp; feedTree: string; search: string; searchDialog: string; settings: string; suggestedRules: string };
  searchQuery: string;
  folders: [string, string, string];
  feeds: {
    deskTools: string;
    teardown: string;
    livingLab: string;
    slowLiving: string;
    paperInk: string;
    craftNotes: string;
    lightLens: string;
    dealRadar: string;
    circuitDaily: string;
    openWeekly: string;
    sundayStudy: string;
  };
  interestMain: string;
  interestBackup: string;
  homeServer: { title: string; summary: string };
  smartHomeHead: string;
  smartHomeRep: { title: string; summary: string };
  smartHomeOld: string;
  smartHomeNew: string;
  /** 焦点里其余五篇：电子墨水屏笔记、家庭照片备份、手机快充、好价速报（噪声源）、老笔记本升级。 */
  others: [
    { title: string; summary: string },
    { title: string; summary: string },
    { title: string; summary: string },
    { title: string; summary: string },
    { title: string; summary: string },
  ];
  timelineOnly: [string, string, string];
  lede: string;
  cluster: { overview: string; overviewCheck: string; oldLine: string; repLine: string; addition: string; conflict: string };
  reader: {
    summary: string;
    reasonTitleKw: string;
    reasonBodyPhrase: string;
    intro: string;
    h1: string;
    p1a: string;
    p1code: string;
    p1b: string;
    quote: string;
    h2: string;
    items: [[string, string], [string, string], [string, string]];
    codeComment: string;
    outro: string;
  };
  /** 搜索与列表点开时的正文：智能家居、墨水屏、照片备份、好价速报、手机快充、老笔记本升级。 */
  briefs: [string[], string[], string[], string[], string[], string[]];
  digest: { heading: string; items: [[string, string], [string, string], [string, string]]; closing: string };
  threads: [string, string, string];
  chat: {
    ask: string;
    excerpt: string;
    answerLead: string;
    intro: [string, string, string];
    points: [[string, string], [string, string, string, string], [string, string]];
    outro: [string, string, string, string, string];
    follow: string;
    followLead: string;
    followBody: [string, string, string];
    other: [string, string];
    digestAsk: string;
    digestLead: string;
    digestBody: [string, string, string];
  };
  rules: {
    boostTitle: string;
    hideTitle: string;
    suggestBody: string;
    boostBody: string;
    hideScope: string;
  };
};

const ZH: Copy = {
  ui: { digest: "简报", feeds: /^订阅/, feedTree: "订阅", search: "搜索文章", searchDialog: "搜索文章", settings: "设置", suggestedRules: "建议规则" },
  searchQuery: "备份",
  folders: ["硬件与生活", "深度随笔", "科技速览"],
  feeds: {
    deskTools: "书桌与工具",
    teardown: "拆机研究所",
    livingLab: "客厅实验室",
    slowLiving: "慢生活志",
    paperInk: "纸上笔记",
    craftNotes: "工程笔记",
    lightLens: "光影手记",
    dealRadar: "好价速报",
    circuitDaily: "极客早报",
    openWeekly: "开源周刊",
    sundayStudy: "周末书房",
  },
  interestMain: "家庭服务器",
  interestBackup: "备份",
  homeServer: {
    title: "把迷你主机改造成家庭备份站：一年电费、静音与踩坑记录",
    summary: "用一台 8 瓦低功耗小主机接管全家相册与文档备份，放电视柜里几乎听不到声音，一年电费不到四十元，也省掉了几份网盘会员。",
  },
  smartHomeHead: "通用智能家居标准更新，本地离线联动成为标配",
  smartHomeRep: {
    title: "智能家居新标准实测：断网后客厅灯光与传感器终于照常响应",
    summary: "把自动化规则迁回本地网关之后，人来灯亮的延迟从半秒降到几乎无感，路由器拔掉外网线也能正常触发。",
  },
  smartHomeOld: "新版智能家居通用标准发布：本地网关正式接管自动化规则",
  smartHomeNew: "老设备能升级吗？本地离线联动落地后的旧款插座兼容性实测",
  others: [
    {
      title: "用电子墨水屏记了一个月读书笔记，聊聊顺手和劝退的地方",
      summary: "护眼和两周一充确实让人安心，但回翻旧批注依然偏慢。最后把它固定成晚间阅读器，长笔记还是回到电脑整理。",
    },
    {
      title: "家庭照片存了十几年，我整理出一套不折腾的三份备份方案",
      summary: "手机原片、家里硬盘加一份加密异地备份，按年份分目录并定期校验。不绑定特定软件，换设备也能直接打开。",
    },
    {
      title: "为什么现在很多手机快充在 80% 之后悄悄变慢了？",
      summary: "电池循环寿命与发热成了新重点：前半小时补足大部分电量，后段放缓反而能让一块电池多用两年。",
    },
    {
      title: "【限时好价】桌面插线板、氮化镓充电头与机械键盘今日凑单汇总",
      summary: "整理了二十多条晚间限时满减与领券信息，多数优惠今晚十二点失效。",
    },
    {
      title: "给五年前的轻薄本清灰、换硅脂再升级硬盘：两百块续命三年",
      summary: "风扇不再一开网页就起飞，日常写作与修图依然流畅。文末附拆机前备份旧盘的步骤与实测温度对比。",
    },
  ],
  timelineOnly: [
    "在家冲一杯稳定的手冲咖啡：比豆子更重要的是磨豆机与水温",
    "在阳台搭了一套自动浇花，最难的不是代码而是选水阀",
    "为什么不少常用 App 越更新越慢？从启动耗时看包体膨胀",
  ],
  lede: "低功耗家庭服务器与相册备份受关注，智能家居通用标准转向本地离线联动，硬件耐用与翻新经验走热。",
  cluster: {
    overview: "**新版智能家居标准**把自动化执行迁回本地网关，断网时灯光、门磁和温湿度联动照常运行，响应延迟也明显降低。",
    overviewCheck: "新版智能家居标准",
    oldLine: "本地网关正式接管自动化规则",
    repLine: "断网后灯光与传感器照常响应",
    addition: "旧款纯 Wi-Fi 插座需要借助桥接网关才能接入本地规则，电池传感器大多可直接配对。",
    conflict: "三年前的老款中枢能否升级支持新标准说法不一：一说仅限带低功耗射频芯片的新机型，一说下季度会推出桥接固件。",
  },
  reader: {
    summary: "作者用一台 8 瓦低功耗迷你主机接管全家相册与文档备份，运行一年后复盘电费、噪音控制和三层备份脚本。核心经验是少装花哨服务，把稳定和随时可恢复放在第一位。",
    reasonTitleKw: "家庭服务器",
    reasonBodyPhrase: "备份",
    intro: "去年冬天，家里的旧台式机风扇越转越响，几份网盘会员也刚好陆续到期。我干脆花几百块买了一台巴掌大的低功耗迷你主机，塞进电视柜角落，专门负责全家照片、文档和笔记的自动备份。",
    h1: "先算电费和噪音",
    p1a: "开机跑满一年，插座电表显示整机日常待机只有 7 到 8 瓦，算下来一年电费不到四十元。真正提升体验的是把机械硬盘换成安静的固态，并改用 ",
    p1code: "restic",
    p1b: " 在凌晨做增量快照，白天客厅里几乎完全听不到风扇声。",
    quote: "好的家庭工具应该像路由器一样：装好之后，半年都想不起它在运行。",
    h2: "三层备份怎么配",
    items: [
      ["手机与电脑同步", "：回家连上 Wi-Fi 自动把新拍照片归档到按年份命名的普通文件夹。"],
      ["本机每日快照", "：凌晨自动去重并保留最近一周和四周的历史版本，误删随时找回。"],
      ["每周加密异地备份", "：只上传加密后的变化块到对象存储，每季度抽查一次能否正常解包。"],
    ],
    codeComment: "# 凌晨两点执行加密增量备份并清理过期快照",
    outro: "折腾一年最大的体会是：家庭服务器不需要堆一堆监控面板。先保证停电重启后能自己恢复、硬盘坏了一块也能很快把照片找回来，就赢过九成方案了。",
  },
  briefs: [
    [
      "新版通用智能家居标准上线后，我第一时间升级了客厅里的主网关，把原本写在云端的十来条灯光与人体传感器联动全部迁回本地。",
      "拔掉宽带测试了一整个晚上，起夜感应灯和窗帘定时依然秒级响应，再也不用担心服务器抖动把家里变成盲盒。",
    ],
    [
      "这一个月里我把晚间读长文和划线批注都搬到了电子墨水屏上。没有消息弹窗打扰，加上两周才充一次电，睡前看半小时书的节奏舒服很多。",
      "唯一的短板是检索和整理旧笔记太慢，后来我索性只拿它看书和随手圈画，周末再导出到电脑里归档。",
    ],
    [
      "整理了家里十几年攒下的两万多张照片，最靠谱的做法依然是「三份副本、两种介质、一份异地」。",
      "我把原片按年份放进普通文件夹，家里两块硬盘互为备份，再把加密后的增量包同步到异地空间。即便以后换掉现在的相册软件，照片照样能直接打开。",
    ],
    [
      "晚间限时好价汇总：多口氮化镓充电头、桌面防浪涌插线板和几款热插拔机械键盘都在今晚十二点前结束满减。",
    ],
    [
      "不少新机标称百瓦快充，实测却会在 80% 之后明显降功率。拆解充电曲线能看出，厂商更看重长期循环寿命和温控表现。",
    ],
    [
      "这台五年前买的轻薄本原本一开十几个网页就风扇狂转。动手升级前，我先用移动硬盘把旧盘里的工程和文档完整备份了一遍。",
      "清掉散热鳍片里的积灰、换上新硅脂和一块 1 TB 固态硬盘后，整机温度降了十几度，花两百多块又踏实多用三年。",
    ],
  ],
  digest: {
    heading: "今日三条主线",
    items: [
      ["家庭备份回归简洁", "：低功耗迷你主机配合加密增量快照，用极低电费就能稳妥管好全家照片与文档。"],
      ["智能家居走向本地联动", "：新标准把自动化规则迁回本地网关，断网照常响应；旧款 Wi-Fi 插座则需桥接过渡。"],
      ["把老硬件用久一点", "：从快充保护策略到旧笔记本清灰换盘，大家越来越看重设备的长期耐用与可维护性。"],
    ],
    closing: "今天的共同主题：少一点云端焦虑，把常用设备和重要数据稳稳握在自己手里。",
  },
  threads: ["家里照片和文档的异地备份怎么配", "对比三篇智能家居本地联动的实测", "挑几篇周末适合细读的硬件长文"],
  chat: {
    ask: "家里照片和文档大概有 400 GB，照这篇的思路配异地备份，怎么选才省心又省钱？",
    excerpt: "每周加密异地备份：只上传加密后的变化块到对象存储，每季度抽查一次能否正常解包。",
    answerLead: "400 GB 规模，对象存储的冷归档档位最省心。",
    intro: ["如果日常改动主要是新增照片和文档，", "选支持增量加密的对象存储", "就行，重点看三点："],
    points: [
      ["先压成本", "：400 GB 放低频或归档存储，每月费用通常只需几元到十几元，比多买几份网盘会员稳妥。"],
      ["再做加密", "：直接复用文里的 ", "restic", "，本机加密并去重后再上传，服务商看不到明文内容。"],
      ["定期演练", "：每隔两三个月随便挑一个旧目录还原试试，确认密码和备份清单都没问题。"],
    ],
    outro: ["目录结构建议按年份整理，换软件也能直接看，可参考 ", "三层备份怎么配", " 和 ", "家庭照片三份备份方案", " 两篇里的做法。"],
    follow: "第一次传 400 GB 会不会把家里宽带全占满？",
    followLead: "加一条限速参数，分两三个晚上跑完就行。",
    followBody: ["首次全量备份可以给上传加上 ", "--limit-upload", " 限制在带宽一半以内，并安排在夜间运行；支持断点续传，之后每周只传新增照片，几分钟就能跑完。"],
    other: ["三篇实测里，对老款中枢能不能升级的说法为什么不一样？", "从这周收藏里挑三篇适合周末慢慢看的硬件与生活长文。"],
    digestAsk: "展开第二条：老款 Wi-Fi 插座不换新怎么接入本地联动？",
    digestLead: "可以通过支持新标准的本地桥接网关过渡。",
    digestBody: ["据 ", "客厅实验室的实测", "，旧款纯 Wi-Fi 插座可挂在本地桥接网关下参与断网联动，触发延迟会略慢于原生设备，部分早期型号需等下季度的桥接固件。"],
  },
  rules: {
    boostTitle: "家庭服务器",
    hideTitle: "凑单",
    suggestBody: "备份",
    boostBody: "异地备份",
    hideScope: "科技速览",
  },
};

const EN: Copy = {
  ui: { digest: "Digest", feeds: /^Feeds/, feedTree: "Feeds", search: "Search", searchDialog: "Search articles", settings: "Settings", suggestedRules: "Suggested rules" },
  searchQuery: "backup",
  folders: ["Hardware", "Essays", "Tech News"],
  feeds: {
    deskTools: "Desk & Tools",
    teardown: "Bench Notes",
    livingLab: "Living Lab",
    slowLiving: "Slow Living",
    paperInk: "Paper & Ink",
    craftNotes: "Craft Notes",
    lightLens: "Light & Lens",
    dealRadar: "Deal Radar",
    circuitDaily: "Circuit Daily",
    openWeekly: "Open Weekly",
    sundayStudy: "Sunday Study",
  },
  interestMain: "home server",
  interestBackup: "backup",
  homeServer: {
    title: "A year with a quiet home server: power, backups and lessons",
    summary: "An 8-watt mini PC tucked behind the router now handles family photos and encrypted backups—whisper quiet, under $10 a year in electricity, and no scattered cloud plans.",
  },
  smartHomeHead: "Smart-home spec adds offline local automation",
  smartHomeRep: {
    title: "Smart-home update tested: lights and sensors work offline",
    summary: "Moving automation rules onto the local hub cut motion-light delay from half a second to instant, even with the modem unplugged.",
  },
  smartHomeOld: "New smart-home spec launches with local hubs taking over automation rules",
  smartHomeNew: "Can older gear upgrade? Testing legacy smart plugs on the new offline spec",
  others: [
    {
      title: "A month of taking reading notes on an e-ink tablet: what clicked and what didn't",
      summary: "Easy on the eyes with two-week battery life, though jumping back to old annotations still feels sluggish. It became my evening reader while long notes stay on the laptop.",
    },
    {
      title: "Ten years of family photos: a low-maintenance three-copy backup routine",
      summary: "Phone originals, a local drive at home, and one encrypted offsite backup organized by year with regular checks—no proprietary catalog lock-in.",
    },
    {
      title: "Why modern phone fast-charging quietly slows down past 80%",
      summary: "Cell longevity and thermals now beat headline wattage: topping up fast early and tapering late keeps a battery healthy for two extra years.",
    },
    {
      title: "[Flash Deals] Desk power strips, GaN chargers and mechanical keyboards on sale today",
      summary: "A roundup of twenty limited-time coupons and bundle promos expiring at midnight.",
    },
    {
      title: "Fresh thermal paste and an SSD swap on a five-year-old laptop: three more years for $40",
      summary: "The fan stopped roaring on every browser tab, and daily writing and photo edits feel snappy again. Includes pre-swap drive backup notes and temp charts.",
    },
  ],
  timelineOnly: [
    "Brewing a consistent pour-over at home: grind size and water temp beat rare beans",
    "Automating balcony planters: the code took an evening, picking a reliable valve took weeks",
    "Why everyday apps keep getting slower: tracing cold-start time and bundle bloat",
  ],
  lede: "Low-power home servers and photo backups stand out, open smart-home automation goes offline-first, and keeping older hardware alive is back in style.",
  cluster: {
    overview: "**The updated smart-home standard** moves automation execution onto the local hub so lights, door sensors, and climate rules keep working offline with noticeably lower latency.",
    overviewCheck: "The updated smart-home standard",
    oldLine: "Local hubs take over automation rules",
    repLine: "Lights and sensors keep working offline",
    addition: "Older Wi-Fi-only smart plugs need a local bridge hub to join offline routines, while most battery sensors pair directly.",
    conflict: "Reports disagree on whether three-year-old hubs will get the update: one says only newer low-power radio models qualify, while another expects a bridge firmware next quarter.",
  },
  reader: {
    summary: "After a year running an 8-watt mini PC for family photos and document backups, the author breaks down electricity costs, noise tuning, and a three-tier backup script. The main lesson: skip complex dashboards and optimize for effortless recovery.",
    reasonTitleKw: "home server",
    reasonBodyPhrase: "backup",
    intro: "Last winter my old desktop tower sounded like a hair dryer and three cloud storage renewals landed in the same week. I picked up a palm-sized low-power mini PC, tucked it into the media console, and gave it one job: quietly keeping our family photos and notes backed up.",
    h1: "Power draw and noise first",
    p1a: "A wall meter measured 7 to 8 watts at idle all year—less than ten dollars in electricity. Swapping the spinning drive for a quiet SSD and scheduling overnight incremental snapshots with ",
    p1code: "restic",
    p1b: " kept the living room completely silent during the day.",
    quote: "Good home infrastructure should feel like a router: set it up once and forget it for six months.",
    h2: "How the three-tier backup works",
    items: [
      ["Phone & laptop sync", ": new photos land in plain year-by-year folders automatically once you join home Wi-Fi."],
      ["Nightly local snapshots", ": deduplicated versions kept for seven days and four weeks so accidental deletes are trivial to undo."],
      ["Weekly encrypted offsite backup", ": only changed encrypted chunks go to object storage, with a quarterly restore spot-check."],
    ],
    codeComment: "# Nightly 2 AM encrypted incremental backup and retention prune",
    outro: "My biggest takeaway after a year: a home server doesn't need a wall of status graphs. If it recovers cleanly after a power cut and lets you restore photos from a dead drive in ten minutes, it already beats most setups.",
  },
  briefs: [
    [
      "As soon as the new smart-home update landed, I updated our living-room hub and migrated a dozen cloud routines for hallway lights and motion sensors to run locally.",
      "With the modem unplugged all evening, night lights and blind schedules still fired instantaneously—no more waiting on a remote server just to flip a switch.",
    ],
    [
      "For the past month I moved all evening reading and margin notes onto an e-ink tablet. Without notification badges and with two-week battery life, winding down with a long essay felt much calmer.",
      "The catch is flipping back through old highlights still feels slow, so I now treat it as a distraction-free reader and sync summaries back to my laptop on weekends.",
    ],
    [
      "After organizing twenty thousand family photos from the past decade, the plain 3-2-1 backup habit still wins: three copies, two media types, one offsite.",
      "Originals live in ordinary year-based folders mirrored across two home drives, plus an encrypted offsite copy. Even if I switch photo apps tomorrow, every file opens directly.",
    ],
    [
      "Today's flash deals roundup: multi-port GaN chargers, surge-protected desk strips, and hot-swap mechanical keyboards with coupons expiring at midnight.",
    ],
    [
      "Plenty of phones advertise 100W charging yet quietly taper right past 80%. Looking at the charge curve shows makers now trade the last ten minutes for two extra years of battery health.",
    ],
    [
      "My five-year-old ultrabook used to spin its fan at full blast with ten tabs open. Before opening the bottom cover, I ran a full backup of my projects and documents onto an external SSD.",
      "Cleaning dust out of the heatsink fins, applying fresh thermal paste, and dropping in a 1 TB drive shaved twelve degrees off load temps—three more quiet years for forty dollars.",
    ],
  ],
  digest: {
    heading: "Three threads today",
    items: [
      ["Simpler home backups", ": 8-watt mini PCs with encrypted snapshots keep family photos safe for pennies."],
      ["Smart homes go local", ": routines now run offline on local hubs, while older Wi-Fi plugs bridge in."],
      ["Keeping hardware longer", ": gentler fast-charging and $40 laptop refreshes beat yearly upgrades."],
    ],
    closing: "Less cloud lock-in, and calmer control over everyday devices and data.",
  },
  threads: ["Setting up offsite backups for home photos", "Comparing three local smart-home reviews", "Weekend long reads on hardware and craft"],
  chat: {
    ask: "We have around 400 GB of family photos and documents. Following this post, what's the simplest, cheapest way to add offsite backup?",
    excerpt: "Weekly encrypted offsite backup: only changed encrypted chunks go to object storage, with a quarterly restore spot-check.",
    answerLead: "At 400 GB, cold object storage with client-side encryption is the sweet spot.",
    intro: ["Since most changes are just new photos and documents, ", "pair object storage with incremental encryption", " and keep three habits:"],
    points: [
      ["Keep costs tiny", ": 400 GB on infrequent-access storage runs roughly $2–3 a month, well below bundled cloud plans."],
      ["Encrypt before upload", ": reuse ", "restic", " from the article so files are deduplicated and encrypted locally before leaving home."],
      ["Test a restore", ": every few months, pull back one random folder to verify your password and recovery steps still work."],
    ],
    outro: ["Keeping plain year-based folders makes moving tools painless later—see ", "How the three-tier backup works", " and ", "Ten years of family photos", " for folder layout ideas."],
    follow: "Won't uploading 400 GB the first time saturate our home internet?",
    followLead: "Cap the upload speed and let it finish over a couple of nights.",
    followBody: ["Pass ", "--limit-upload", " set to half your uplink during the initial sync and run it overnight; uploads resume cleanly if interrupted, and weekly runs after that take only a few minutes."],
    other: ["Why do the three reviews disagree on whether older hubs can upgrade?", "Pick three hardware and lifestyle long reads from my stars for the weekend."],
    digestAsk: "Expand item two: can older Wi-Fi plugs join local routines?",
    digestLead: "Yes—through a local bridge hub that supports the new standard.",
    digestBody: ["According to ", "Living Lab's tests", ", legacy Wi-Fi plugs can join offline routines via a bridge hub, with slightly higher latency while early models wait for bridge firmware."],
  },
  rules: {
    boostTitle: "home server",
    hideTitle: "flash deals",
    suggestBody: "backup",
    boostBody: "offsite backup",
    hideScope: "Tech News",
  },
};

const COPY: Record<DemoLanguage, Copy> = { "zh-CN": ZH, en: EN };

export type ReadmeDemo = {
  host: MockHostOptions;
  ui: Copy["ui"];
  overviewCheck: string;
  firstThread: string;
  searchQuery: string;
};

export function readmeDemo(language: DemoLanguage): ReadmeDemo {
  const c = COPY[language];
  const MINUTE = 60_000;
  const now = Date.now();
  const ago = (minutes: number) => now - minutes * MINUTE;
  const yesterdayAt = (hours: number, minutes: number) => {
    const at = new Date();
    at.setDate(at.getDate() - 1);
    at.setHours(hours, minutes, 0, 0);
    return at.getTime();
  };
  const today = new Date();
  const day = (ms: number) => {
    const d = new Date(ms);
    return `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  };

  // ── 订阅 ──
  const feed = (id: number, title: string, folder_id: number, url: string, extra: Partial<FeedView> = {}): FeedView => ({
    id, title, url, folder_id, icon_href: "", error: null, noisy: false, retention_days: null, interval_minutes: null, last_fetched_at: null, next_fetch_at: null, limited_until: null,
    health: null, position: id, empty: false, ...extra,
  });
  const fn = c.feeds;
  const feeds: FeedTree = {
    folders: [
      {
        id: 1, name: c.folders[0], label: c.folders[0], indent: 0, position: 0, feeds: [
          feed(1, fn.deskTools, 1, "https://desk-tools.example/rss"),
          feed(2, fn.teardown, 1, "https://teardown-bench.example/rss"),
          feed(3, fn.livingLab, 1, "https://living-lab.example/feed.xml"),
          feed(4, fn.slowLiving, 1, "https://slow-living.example/atom.xml"),
        ],
      },
      {
        id: 2, name: c.folders[1], label: c.folders[1], indent: 0, position: 1, feeds: [
          feed(5, fn.paperInk, 2, "https://paper-ink.example/feed"),
          feed(6, fn.craftNotes, 2, "https://craft-notes.example/rss"),
          feed(7, fn.lightLens, 2, "https://light-lens.example/atom.xml"),
          feed(8, fn.dealRadar, 2, "https://deal-radar.example/index.xml", { noisy: true, health: { code: "feed_health_noisy", params: { window_days: 14 } } }),
        ],
      },
      {
        id: 3, name: c.folders[2], label: c.folders[2], indent: 0, position: 2, feeds: [
          feed(9, fn.circuitDaily, 3, "https://circuit-daily.example/feed"),
          feed(10, fn.openWeekly, 3, "https://open-weekly.example/rss"),
        ],
      },
    ],
    uncategorized: [feed(11, fn.sundayStudy, 0, "https://sunday-study.example/index.xml")],
    folder_combo: [...c.folders],
  };
  const allFeeds = [...feeds.folders.flatMap(f => f.feeds), ...feeds.uncategorized];
  const feedById = (id: number) => allFeeds.find(f => f.id === id);
  const feedTitle = (id: number) => feedById(id)?.title ?? "";
  const feedOrigin = (id: number) => {
    const raw = feedById(id)?.url;
    return raw ? new URL(raw).origin : "https://desk-tools.example";
  };

  // ── 今日焦点 ──
  const article = (id: number, title: string, feed_id: number, published_at: number, extra: Partial<ArticleView> = {}): ArticleView => ({
    id, title, feed_id, feed_title: feedTitle(feed_id), feed_icon_href: "", published_at, url: `${feedOrigin(feed_id)}/posts/${id}`,
    read: false, starred: false, score: 0, summary: "", row_kind: 0, cluster_id: 0, cluster_size: 1, cluster_label: "",
    cluster_span: "", interest_reason: null, rank_reasons: [], noisy: false, spot_check: false, explore: false, ...extra,
  });
  const interest = (pattern: string) => ({ code: "interest_match" as const, params: { pattern } });

  const homeServer = article(101, c.homeServer.title, 1, ago(18), { score: 14, interest_reason: interest(c.interestMain), summary: c.homeServer.summary });
  const smartHomeRep = article(901, c.smartHomeRep.title, 2, ago(42), { score: 8, cluster_id: 900, cluster_size: 3, summary: c.smartHomeRep.summary });
  const smartHomeOld = article(902, c.smartHomeOld, 9, ago(5 * 60), { cluster_id: 900, cluster_size: 3, read: true });
  const smartHomeNew = article(903, c.smartHomeNew, 3, ago(95), { cluster_id: 900, cluster_size: 3 });
  const smartHomeHead = article(900, c.smartHomeHead, 0, 0, {
    feed_title: "", published_at: null, row_kind: 1, cluster_id: 900, cluster_size: 3,
    cluster_label: c.smartHomeHead, cluster_span: `${day(yesterdayAt(12, 0))} → ${day(now)}`,
    cluster_sources: [
      { feed_id: 2, title: fn.teardown, icon_href: "" },
      { feed_id: 9, title: fn.circuitDaily, icon_href: "" },
      { feed_id: 3, title: fn.livingLab, icon_href: "" },
    ],
  });
  const [o1, o2, o3, o4, o5] = c.others;
  const eink = article(102, o1.title, 5, ago(2 * 60 + 10), { score: 6, starred: true, summary: o1.summary });
  const photos = article(103, o2.title, 7, ago(3 * 60 + 25), { score: 5, interest_reason: interest(c.interestBackup), summary: o2.summary });
  const charging = article(104, o3.title, 2, ago(5 * 60 + 40), { score: 3, read: true, summary: o3.summary });
  const deals = article(105, o4.title, 8, yesterdayAt(21, 40), { score: -3, noisy: true, summary: o4.summary });
  const laptop = article(106, o5.title, 1, yesterdayAt(9, 12), { score: 4, summary: o5.summary });
  const focusArticles = [homeServer, smartHomeHead, smartHomeRep, eink, photos, charging, deals, laptop];

  const clusterRef = (row: ArticleView, one_liner = ""): ClusterArticleView => ({
    article_id: row.id, feed_id: row.feed_id, feed_title: row.feed_title, icon_href: "", title: row.title, published_at: row.published_at, one_liner,
  });
  const smartHomeSummary: ClusterSummaryView = {
    cluster_id: 900, state: "ready", model_ready: true, estimate_tokens: 2000, basis: 3, generated_at: ago(6), fresh: [],
    members: [clusterRef(smartHomeOld, c.cluster.oldLine), clusterRef(smartHomeRep, c.cluster.repLine), clusterRef(smartHomeNew)],
    overview: c.cluster.overview,
    additions: [{ text: c.cluster.addition, sources: [clusterRef(smartHomeNew)] }],
    conflicts: [{ text: c.cluster.conflict, sources: [clusterRef(smartHomeOld), clusterRef(smartHomeRep)] }],
  };

  // ── 阅读器 ──
  const t = (text: string) => ({ t: "t", text });
  const r = c.reader;
  const reader: ReaderView = {
    id: homeServer.id, source_hash: "readme-home-server", title: homeServer.title,
    feed_id: homeServer.feed_id, feed_title: homeServer.feed_title,
    meta: fn.deskTools, feed_icon_href: "", url: "https://desk-tools.example/posts/quiet-home-server",
    summary: r.summary, score: 14, feedback: 0, starred: false,
    reasons: [
      { rule_id: 11, dimension: "title_kw", snippet: r.reasonTitleKw, code: "rule_title_kw", params: { pattern: r.reasonTitleKw }, weight: 10 },
      { rule_id: 12, dimension: "body_phrase", snippet: r.reasonBodyPhrase, code: "rule_body_phrase", params: { pattern: r.reasonBodyPhrase }, weight: 2 },
      { rule_id: 0, dimension: "interest", snippet: "", code: "interest_match", params: { pattern: c.interestMain }, weight: 2 },
    ],
    media: [], embed_covers: [], translated_title: "", translated_document: null, translation_ready: false, translation_note: "",
    truncated: false, pruned: false, summary_stale: false,
    document: {
      v: 1,
      blocks: [
        { t: "p", inlines: [t(r.intro)] },
        { t: "h", level: 2, inlines: [t(r.h1)] },
        { t: "p", inlines: [t(r.p1a), { t: "c", text: r.p1code }, t(r.p1b)] },
        { t: "q", inlines: [t(r.quote)] },
        { t: "h", level: 2, inlines: [t(r.h2)] },
        ...r.items.map(([lead, rest]) => ({ t: "li", indent: 0, marker: "•", inlines: [{ t: "s", children: [t(lead)] }, t(rest)] })),
        {
          t: "pre",
          lang: "bash",
          text: `${r.codeComment}\nrestic backup ~/Photos ~/Notes --exclude-caches\nrestic forget --keep-daily 7 --keep-weekly 4 --prune`,
        },
        { t: "p", inlines: [t(r.outro)] },
      ],
    },
  };
  // 其余文章的正文：搜索命中各自的段落。
  const brief = (row: ArticleView, paragraphs: string[]): ReaderView => ({
    ...reader, id: row.id, source_hash: `readme-${row.id}`, title: row.title, url: row.url, summary: row.summary, score: row.score,
    feed_id: row.feed_id, feed_title: row.feed_title, meta: row.feed_title, reasons: [], starred: row.starred,
    document: { v: 1, blocks: paragraphs.map(text => ({ t: "p", inlines: [t(text)] })) },
  });
  const readers: Record<number, ReaderView> = Object.fromEntries(
    [smartHomeRep, eink, photos, deals, charging, laptop].map((row, index) => brief(row, c.briefs[index])).map(view => [view.id, view]),
  );

  // ── 简报 ──
  const source = (row: ArticleView, number: number): DigestSourceView => ({
    article_id: row.id, title: row.title, feed_title: row.feed_title, number, summary: row.summary || null, published_at: row.published_at, status: "available",
  });
  const sources = [source(homeServer, 1), source(photos, 2), source(smartHomeRep, 3), source(smartHomeNew, 4), source(laptop, 5)];
  const digest: DigestView = {
    id: 101, period: "daily", period_start: new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime(),
    revision: "readme-digest", source_state: "verified", source_index: sources, unlocated_sources: [],
    body: "",
    items: [
      { block_id: 1, sources: [sources[0], sources[1]] },
      { block_id: 2, sources: [sources[2], sources[3]] },
      { block_id: 3, sources: [sources[4]] },
    ],
    document: {
      v: 1,
      blocks: [
        { t: "h", level: 3, inlines: [t(c.digest.heading)] },
        ...c.digest.items.map(([lead, rest], index) => ({ t: "li", marker: `${index + 1}.`, indent: 0, inlines: [{ t: "s", children: [t(lead)] }, t(rest)] })),
        { t: "p", inlines: [t(c.digest.closing)] },
      ],
    },
  };
  const previousDigest: DigestView = { ...digest, id: 100, revision: "readme-digest-prev", period_start: digest.period_start - 86_400_000 };

  // ── 对话 ──
  const message = (id: number, role: string, text: string, extra: Partial<ChatMessageView> = {}): ChatMessageView => ({
    id, role, text, streaming: false, cite_article_id: 0, cite_title: "", citations: [], document: null, ...extra,
  });
  const threads = [
    threadFixture({ id: 7, title: c.threads[0], updated_at: ago(3) }),
    threadFixture({ id: 8, title: c.threads[1], updated_at: ago(60) }),
    threadFixture({ id: 9, title: c.threads[2], updated_at: yesterdayAt(22, 5) }),
  ];
  const ch = c.chat;
  const [[p1, p1rest], [p2, p2a, p2code, p2b], [p3, p3rest]] = ch.points;
  const messages: Record<number, ChatMessageView[]> = {
    7: [
      message(1, "user", ch.ask, {
        citations: [
          { kind: "article", article_id: homeServer.id, title: homeServer.title, feed_title: fn.deskTools, url: homeServer.url, excerpt: "", block_id: 0 },
          { kind: "excerpt", article_id: homeServer.id, title: homeServer.title, feed_title: fn.deskTools, url: homeServer.url, excerpt: ch.excerpt, block_id: 7 },
        ],
      }),
      message(2, "assistant", ch.answerLead, {
        document: {
          v: 1,
          blocks: [
            { t: "p", inlines: [t(ch.intro[0]), { t: "s", children: [t(ch.intro[1])] }, t(ch.intro[2])] },
            { t: "li", indent: 0, marker: "1.", inlines: [{ t: "s", children: [t(p1)] }, t(p1rest)] },
            { t: "li", indent: 0, marker: "2.", inlines: [{ t: "s", children: [t(p2)] }, t(p2a), { t: "c", text: p2code }, t(p2b)] },
            { t: "li", indent: 0, marker: "3.", inlines: [{ t: "s", children: [t(p3)] }, t(p3rest)] },
            { t: "p", inlines: [t(ch.outro[0]), { t: "a", href: `rrss://article/${homeServer.id}`, children: [t(ch.outro[1])] }, t(ch.outro[2]), { t: "a", href: "rrss://article/103", children: [t(ch.outro[3])] }, t(ch.outro[4])] },
          ],
        },
      }),
      message(5, "user", ch.follow),
      message(6, "assistant", ch.followLead, {
        document: {
          v: 1,
          blocks: [
            { t: "p", inlines: [t(ch.followBody[0]), { t: "c", text: ch.followBody[1] }, t(ch.followBody[2])] },
          ],
        },
      }),
    ],
    8: [message(3, "user", ch.other[0])],
    9: [message(4, "user", ch.other[1])],
    301: [
      message(11, "user", ch.digestAsk),
      message(12, "assistant", ch.digestLead, {
        document: {
          v: 1,
          blocks: [
            { t: "p", inlines: [t(ch.digestBody[0]), { t: "a", href: "rrss://article/903", children: [t(ch.digestBody[1])] }, t(ch.digestBody[2])] },
          ],
        },
      }),
    ],
  };

  // ── 订阅时间线 ──
  const timeline: ArticleView[] = [
    homeServer, smartHomeRep, smartHomeNew, eink, photos,
    article(107, c.timelineOnly[0], 4, ago(4 * 60)),
    smartHomeOld, charging,
    article(108, c.timelineOnly[1], 3, yesterdayAt(23, 5), { read: true }),
    laptop,
    article(109, c.timelineOnly[2], 6, yesterdayAt(8, 30), { read: true }),
  ];

  const rules: RulesPage = {
    ...MOCK_RULES,
    rules: [
      { ...MOCK_RULES.rules[0], id: 11, summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: "", pattern: c.rules.boostTitle },
      { ...MOCK_RULES.rules[1], id: 13, summary: { code: "rule_summary", params: { effect: "hide" } }, scope_name: "", pattern: c.rules.hideTitle },
      { ...MOCK_RULES.rules[2], id: 14, summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: fn.deskTools, scope_id: 1, pattern: c.rules.suggestBody },
      { ...MOCK_RULES.rules[0], id: 12, summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: "", dimension_id: "body_phrase", pattern: c.rules.boostBody, weight: 4 },
      { ...MOCK_RULES.rules[3], id: 15, summary: { code: "rule_summary", params: { effect: "hide" } }, scope_name: c.rules.hideScope, scope_id: 3, enabled: true },
    ],
    folders: feeds.folders.map(f => ({ id: f.id, label: f.label })),
    feeds: feeds.folders.flatMap(f => f.feeds).map(f => ({ id: f.id, label: f.title })),
  };

  const host: MockHostOptions = {
    systemLanguage: language,
    focus: { ...MOCK_FOCUS, digest_banner: "daily", digest_lede: c.lede, articles: focusArticles, has_more: false },
    feeds, threads, messages, digests: [digest, previousDigest],
    reader, readers, clusterArticles: { 900: [smartHomeOld, smartHomeRep, smartHomeNew] }, clusterSummaries: { 900: smartHomeSummary },
    timelineArticles: timeline, rules,
  };
  return { host, ui: c.ui, overviewCheck: c.cluster.overviewCheck, firstThread: c.threads[0], searchQuery: c.searchQuery };
}
