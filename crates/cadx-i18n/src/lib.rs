use std::{collections::HashMap, sync::Arc};

const EN_JSON: &str = include_str!("../locales/en.json");
const ZH_JSON: &str = include_str!("../locales/zh.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    SimplifiedChinese,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::English, Self::SimplifiedChinese];

    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::SimplifiedChinese => "zh",
        }
    }

    #[must_use]
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::SimplifiedChinese => "简体中文",
        }
    }

    #[must_use]
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::English => "EN",
            Self::SimplifiedChinese => "中文",
        }
    }

    #[must_use]
    pub fn from_locale(tag: &str) -> Self {
        let normalized = tag.trim();
        if normalized.eq_ignore_ascii_case("simplified-chinese")
            || normalized.eq_ignore_ascii_case("chinese")
        {
            return Self::SimplifiedChinese;
        }
        let base = normalized
            .split('.')
            .next()
            .unwrap_or_default()
            .split(['-', '_'])
            .next()
            .unwrap_or_default();
        if base.eq_ignore_ascii_case("zh") {
            Self::SimplifiedChinese
        } else {
            Self::English
        }
    }
}

#[derive(Debug)]
struct TranslationBundle {
    locale: HashMap<String, String>,
    english: HashMap<String, String>,
}

impl TranslationBundle {
    fn load(language: Language) -> Self {
        let english = serde_json::from_str(EN_JSON).expect("en.json must be valid");
        let locale = match language {
            Language::English => HashMap::new(),
            Language::SimplifiedChinese => {
                serde_json::from_str(ZH_JSON).expect("zh.json must be valid")
            }
        };
        Self { locale, english }
    }

    fn get<'a>(&'a self, key: &'a str) -> &'a str {
        self.locale
            .get(key)
            .or_else(|| self.english.get(key))
            .map_or(key, String::as_str)
    }
}

#[derive(Debug, Clone)]
pub struct Translator {
    language: Language,
    bundle: Arc<TranslationBundle>,
}

impl Translator {
    #[must_use]
    pub fn new(language: Language) -> Self {
        Self {
            language,
            bundle: Arc::new(TranslationBundle::load(language)),
        }
    }

    #[must_use]
    pub const fn language(&self) -> Language {
        self.language
    }

    pub fn set_language(&mut self, language: Language) {
        if language != self.language {
            self.language = language;
            self.bundle = Arc::new(TranslationBundle::load(language));
        }
    }

    #[must_use]
    pub fn text<'a>(&'a self, key: &'a str) -> &'a str {
        self.bundle.get(key)
    }

    #[must_use]
    pub fn format(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut value = self.text(key).to_owned();
        for (name, replacement) in args {
            value = value.replace(&format!("{{{name}}}"), replacement);
        }
        value
    }
}

impl Default for Translator {
    fn default() -> Self {
        Self::new(Language::English)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn parses_bcp47_and_posix_locales() {
        assert_eq!(Language::from_locale("zh-CN"), Language::SimplifiedChinese);
        assert_eq!(
            Language::from_locale("zh_CN.UTF-8"),
            Language::SimplifiedChinese
        );
        assert_eq!(
            Language::from_locale("simplified-chinese"),
            Language::SimplifiedChinese
        );
        assert_eq!(Language::from_locale("en-US"), Language::English);
        assert_eq!(Language::from_locale("C.UTF-8"), Language::English);
    }

    #[test]
    fn switches_language_at_runtime() {
        let mut translator = Translator::default();
        assert_eq!(translator.text("toolbar.create"), "Create");
        translator.set_language(Language::SimplifiedChinese);
        assert_eq!(translator.text("toolbar.create"), "创建");
    }

    #[test]
    fn falls_back_to_english_and_then_the_key() {
        let translator = Translator::new(Language::SimplifiedChinese);
        assert_eq!(translator.text("test.english_only"), "English fallback");
        assert_eq!(translator.text("missing.key"), "missing.key");
    }

    #[test]
    fn interpolates_named_values() {
        let translator = Translator::new(Language::SimplifiedChinese);
        assert_eq!(
            translator.format("status.geometry", &[("bodies", "2"), ("triangles", "1550")]),
            "2 个实体 · 1550 个三角面"
        );
    }

    #[test]
    fn production_translation_keys_are_aligned() {
        let english = serde_json::from_str::<HashMap<String, String>>(EN_JSON).unwrap();
        let chinese = serde_json::from_str::<HashMap<String, String>>(ZH_JSON).unwrap();
        let english = english.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let chinese = chinese.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let english_only = english.difference(&chinese).copied().collect::<Vec<_>>();
        let chinese_only = chinese.difference(&english).copied().collect::<Vec<_>>();

        assert_eq!(english_only, vec!["test.english_only"]);
        assert!(chinese_only.is_empty());
    }
}
