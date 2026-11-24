//! 轻量的语种判断：先按字符种类（汉字、假名、谚文、西里尔等）判断，拉丁字母为主的再交给 whichlang。
//! 只服务「是不是外文、标什么语言码」，不追求小语种精度。按字符种类先判，是为了让
//! 「Rust 1.90 发布」这类夹着英文词的中文短标题仍算中文。

/// 一段文字的语言（ISO 639-1）。没有可判断的字母时为 `None`。
pub fn detect_lang(text: &str) -> Option<&'static str> {
    let mut c = Counts::default();
    for ch in text.chars() {
        c.add(ch);
    }
    let letters = c.letters();
    if letters == 0 {
        return None;
    }
    // 短文字（标题）看「出现没有」：英文标题里几乎不会混入假名、谚文或成段汉字；
    // 长文字看占比，避免英文正文里引用的几个汉字把整篇判成中文。
    let short = letters < 200;
    let share = |n: usize| n * 100 / letters;
    if c.kana > 0 && (short || share(c.kana + c.han) >= 20) && (short || share(c.kana) >= 3) {
        return Some("ja");
    }
    if c.hangul >= 2 && (short || share(c.hangul) >= 20) {
        return Some("ko");
    }
    if c.han >= 2 && (short || share(c.han) >= 20) {
        return Some("zh");
    }
    let dominant = |n: usize| n > 0 && n * 2 >= letters;
    if dominant(c.cyrillic) {
        return Some(if c.ukrainian > 0 { "uk" } else { "ru" });
    }
    if dominant(c.greek) {
        return Some("el");
    }
    if dominant(c.arabic) {
        return Some("ar");
    }
    if dominant(c.hebrew) {
        return Some("he");
    }
    if dominant(c.thai) {
        return Some("th");
    }
    if dominant(c.devanagari) {
        return Some("hi");
    }
    if c.latin == 0 {
        return None;
    }
    Some(latin_lang(text))
}

/// 相对目标语言是外文时返回它的语言码。目标语言取主码（`zh-CN` → `zh`）。
/// `english_only` 为真时只有英文算外文。
pub fn foreign_lang(text: &str, target: &str, english_only: bool) -> Option<&'static str> {
    let target = target.trim().to_ascii_lowercase();
    let base = target.split(['-', '_']).next().unwrap_or_default();
    let lang = detect_lang(text)?;
    if base.is_empty() || lang == base || (english_only && lang != "en") {
        return None;
    }
    Some(lang)
}

/// 正文词数：以空格分词的文字按词计，中日韩文字按两个字一个词。
pub fn word_count(text: &str) -> usize {
    let mut words = 0usize;
    let mut cjk = 0usize;
    for token in text.split_whitespace() {
        let mut latin = false;
        for ch in token.chars() {
            if is_han(ch) || is_kana(ch) || is_hangul(ch) {
                cjk += 1;
            } else if ch.is_alphanumeric() {
                latin = true;
            }
        }
        words += usize::from(latin);
    }
    words + cjk.div_ceil(2)
}

#[derive(Default)]
struct Counts {
    han: usize,
    kana: usize,
    hangul: usize,
    cyrillic: usize,
    ukrainian: usize,
    greek: usize,
    arabic: usize,
    hebrew: usize,
    thai: usize,
    devanagari: usize,
    latin: usize,
    other: usize,
}

impl Counts {
    fn add(&mut self, ch: char) {
        match ch {
            _ if is_han(ch) => self.han += 1,
            _ if is_kana(ch) => self.kana += 1,
            _ if is_hangul(ch) => self.hangul += 1,
            'і' | 'ї' | 'є' | 'ґ' | 'І' | 'Ї' | 'Є' | 'Ґ' => {
                self.cyrillic += 1;
                self.ukrainian += 1;
            }
            '\u{0400}'..='\u{04FF}' => self.cyrillic += 1,
            '\u{0370}'..='\u{03FF}' => self.greek += 1,
            '\u{0600}'..='\u{06FF}' => self.arabic += 1,
            '\u{0590}'..='\u{05FF}' => self.hebrew += 1,
            '\u{0E00}'..='\u{0E7F}' => self.thai += 1,
            '\u{0900}'..='\u{097F}' => self.devanagari += 1,
            _ if ch.is_ascii_alphabetic()
                || ('\u{00C0}'..='\u{024F}').contains(&ch)
                || ('\u{1E00}'..='\u{1EFF}').contains(&ch) =>
            {
                self.latin += 1
            }
            _ if ch.is_alphabetic() => self.other += 1,
            _ => {}
        }
    }

    fn letters(&self) -> usize {
        self.han
            + self.kana
            + self.hangul
            + self.cyrillic
            + self.greek
            + self.arabic
            + self.hebrew
            + self.thai
            + self.devanagari
            + self.latin
            + self.other
    }
}

fn is_han(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}'
    )
}

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}')
}

fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7AF}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}')
}

/// 拉丁字母为主的文字交给 whichlang（字符 n-gram 加逻辑回归，16 种语言，零依赖）。
/// 它总会挑一个最像的：不在那 16 种里的拉丁语言会归到其中之一，对「是不是外文」影响不大；
/// 只有「只把英文算外文」时可能多算或漏算。
fn latin_lang(text: &str) -> &'static str {
    use whichlang::Lang;
    match whichlang::detect_language(text) {
        Lang::Eng => "en",
        Lang::Fra => "fr",
        Lang::Deu => "de",
        Lang::Spa => "es",
        Lang::Por => "pt",
        Lang::Ita => "it",
        Lang::Nld => "nl",
        Lang::Swe => "sv",
        Lang::Tur => "tr",
        Lang::Vie => "vi",
        // 这几种的文字已在前面按字符判过，混在拉丁文字里时按它说的算。
        Lang::Cmn => "zh",
        Lang::Jpn => "ja",
        Lang::Kor => "ko",
        Lang::Rus => "ru",
        Lang::Ara => "ar",
        Lang::Hin => "hi",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_scripts_in_titles() {
        assert_eq!(
            detect_lang("Quillstore 3.0 rewrites its storage engine"),
            Some("en")
        );
        assert_eq!(detect_lang("老旧小区加装电梯的三种出资方式"), Some("zh"));
        assert_eq!(detect_lang("苹果发布 iPhone 17 Pro"), Some("zh"));
        assert_eq!(
            detect_lang("東京の自転車通勤、この10年でどう変わったか"),
            Some("ja")
        );
        assert_eq!(detect_lang("서울 지하철 새 노선 개통"), Some("ko"));
        assert_eq!(detect_lang("Новый закон о связи"), Some("ru"));
        assert_eq!(detect_lang("Київ відкрив нову лінію"), Some("uk"));
        assert_eq!(detect_lang("2026 · 42%"), None);
    }

    #[test]
    fn separates_latin_languages() {
        assert_eq!(
            detect_lang("Why async traits took five years to stabilize"),
            Some("en")
        );
        assert_eq!(
            detect_lang("La réforme des retraites est adoptée"),
            Some("fr")
        );
        assert_eq!(
            detect_lang("Die Bahn ist wieder pünktlich und schnell"),
            Some("de")
        );
        assert_eq!(
            detect_lang("El gobierno anuncia una nueva ley para los alquileres"),
            Some("es")
        );
        assert_eq!(detect_lang("Kubernetes 1.40 released"), Some("en"));
        assert_eq!(detect_lang("Apple's AI push as Siri lags"), Some("en"));
        assert_eq!(
            detect_lang("Governo anuncia nova regra para aluguéis"),
            Some("pt")
        );
        assert_eq!(
            detect_lang("Il governo approva la nuova legge sul lavoro"),
            Some("it")
        );
        assert_eq!(
            detect_lang("Het kabinet wil meer woningen bouwen"),
            Some("nl")
        );
        // 夹着英文词的中文短标题先按汉字判，不交给拉丁字母模型。
        assert_eq!(detect_lang("Rust 1.90 发布"), Some("zh"));
        assert_eq!(
            detect_lang("Hà Nội mở thêm tuyến đường sắt đô thị mới"),
            Some("vi")
        );
    }

    #[test]
    fn long_text_uses_shares_not_presence() {
        let english =
            "The maintainers say write throughput rose by about forty percent. ".repeat(8);
        let quoted = format!("{english} They call it 躺平.");
        assert_eq!(detect_lang(&quoted), Some("en"));
        let chinese = format!(
            "{} Rust 与 Go 的对比。",
            "维护者称写入吞吐提升约四成，读取延迟持平。".repeat(10)
        );
        assert_eq!(detect_lang(&chinese), Some("zh"));
    }

    #[test]
    fn foreign_relative_to_target_main_code() {
        let en = "Quillstore rewrites its storage engine";
        assert_eq!(foreign_lang(en, "zh-CN", false), Some("en"));
        assert_eq!(foreign_lang(en, "en-us", false), None);
        assert_eq!(foreign_lang("东京的自行车", "zh", false), None);
        assert_eq!(foreign_lang("東京の自転車通勤", "zh", false), Some("ja"));
        assert_eq!(foreign_lang("東京の自転車通勤", "zh", true), None);
        assert_eq!(foreign_lang(en, "zh", true), Some("en"));
        assert_eq!(foreign_lang(en, "", false), None);
    }

    #[test]
    fn counts_words_and_cjk_pairs() {
        assert_eq!(word_count("Hello, wide world!"), 3);
        assert_eq!(word_count("四个汉字 and two"), 4);
        assert_eq!(word_count("  "), 0);
    }
}
