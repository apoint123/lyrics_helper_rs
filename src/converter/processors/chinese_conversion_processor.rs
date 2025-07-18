//! 简繁中文转换器。

use std::sync::Arc;

use crate::converter::types::{
    ChineseConversionMode, ChineseConversionOptions, LyricLine, TranslationEntry,
};
use dashmap::DashMap;
use ferrous_opencc::OpenCC;
use pinyin::ToPinyin;
use std::sync::LazyLock;
use tracing::{error, warn};

/// 使用 DashMap 来创建一个 OpenCC 实例缓存。
/// 键是配置文件名 (e.g., "s2t.json")，值是对应的 OpenCC 实例。
static CONVERTER_CACHE: LazyLock<DashMap<String, Arc<OpenCC>>> = LazyLock::new(DashMap::new);

/// 根据指定的 OpenCC 配置转换文本。
///
/// 此函数会缓存 OpenCC 实例。如果某个配置首次被请求，
/// 它会被创建并存入缓存。后续对同一配置的请求将直接使用缓存的实例。
///
/// # 参数
/// * `text` - 需要转换的文本。
/// * `config_name` - `ferrous-opencc` 的配置文件名，例如 "s2t.json"。
///
/// # 返回
/// 转换后的字符串。如果指定的配置加载失败，将打印错误日志并返回原始文本。
pub fn convert(text: &str, config_name: &str) -> String {
    // 检查缓存中是否已存在该转换器
    if let Some(converter) = CONVERTER_CACHE.get(config_name) {
        return converter.convert(text);
    }

    // 如果缓存中没有，则尝试创建并插入
    match CONVERTER_CACHE
        .entry(config_name.to_string())
        .or_try_insert_with(|| {
            OpenCC::from_config_name(config_name)
                .map(Arc::new)
                .map_err(|e| {
                    error!("使用配置 '{}' 初始化 Opencc 时失败: {}", config_name, e);
                    e // 将错误传递出去，or_try_insert_with 需要
                })
        }) {
        Ok(converter_ref) => converter_ref.value().convert(text),
        Err(_) => {
            // 如果创建失败，or_try_insert_with 不会插入任何值。
            // 直接返回原始文本。
            text.to_string()
        }
    }
}

/// 比较两个字符串的拼音是否相同。
///
/// 因为多音字的音调非常难以确定，所以忽略声调。
fn pinyin_is_same(original: &str, converted: &str) -> bool {
    if original.chars().count() != converted.chars().count() {
        return false;
    }

    // 获取无声调的拼音
    let original_pinyins: Vec<_> = original
        .to_pinyin()
        .map(|p| p.map_or("", |p_val| p_val.plain()))
        .collect();

    let converted_pinyins: Vec<_> = converted
        .to_pinyin()
        .map(|p| p.map_or("", |p_val| p_val.plain()))
        .collect();

    original_pinyins == converted_pinyins
}

/// 一个用于执行简繁中文转换的处理器。
#[derive(Debug, Default)]
pub struct ChineseConversionProcessor;

impl ChineseConversionProcessor {
    /// 创建一个新的处理器实例。
    pub fn new() -> Self {
        Self
    }

    /// 对一组歌词行应用简繁转换。
    ///
    /// # 参数
    /// * `lines` - 一个可变的歌词行切片，转换结果将直接写入其中。
    /// * `options` - 简繁转换的配置选项，决定是否执行以及执行何种模式的转换。
    pub fn process(&self, lines: &mut [LyricLine], options: &ChineseConversionOptions) {
        let Some(config_name) = &options.config_name else {
            return;
        };
        if config_name.is_empty() {
            return;
        }

        match options.mode {
            ChineseConversionMode::AddAsTranslation => {
                self.add_as_translation(lines, config_name, options);
            }
            ChineseConversionMode::Replace => {
                self.replace(lines, config_name);
            }
        }
    }

    fn deduce_lang_tag_from_config<'a>(&self, config_name: &'a str) -> Option<&'a str> {
        match config_name {
            "s2t.json" | "jp2t.json" | "hk2t.json" | "tw2t.json" => Some("zh-Hant"),
            "s2tw.json" | "s2twp.json" | "t2tw.json" => Some("zh-Hant-TW"),
            "s2hk.json" | "t2hk.json" => Some("zh-Hant-HK"),
            "t2s.json" | "tw2s.json" | "tw2sp.json" | "hk2s.json" => Some("zh-Hans"),
            "t2jp.json" => Some("ja"),
            _ => None,
        }
    }

    fn add_as_translation(
        &self,
        lines: &mut [LyricLine],
        config_name: &str,
        options: &ChineseConversionOptions,
    ) {
        let lang_tag = options
            .target_lang_tag
            .as_deref()
            .or_else(|| self.deduce_lang_tag_from_config(config_name));

        let Some(target_lang_tag) = lang_tag else {
            warn!(
                "无法确定 target_lang_tag (未提供且无法从 '{}' 推断)。跳过简繁转换。",
                config_name
            );
            return;
        };

        for line in lines.iter_mut() {
            let already_exists = line
                .translations
                .iter()
                .any(|t| t.lang.as_deref() == Some(target_lang_tag));

            if !already_exists {
                let main_text = if !line.main_syllables.is_empty() {
                    line.main_syllables
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect()
                } else {
                    line.line_text.clone().unwrap_or_default()
                };

                if !main_text.is_empty() {
                    let converted_text = convert(&main_text, config_name);
                    line.translations.push(TranslationEntry {
                        text: converted_text,
                        lang: Some(target_lang_tag.to_string()),
                    });
                }
            }
        }
    }

    fn replace(&self, lines: &mut [LyricLine], config_name: &str) {
        for line in lines.iter_mut() {
            if !line.main_syllables.is_empty() {
                let original_syllable_texts: Vec<String> =
                    line.main_syllables.iter().map(|s| s.text.clone()).collect();
                let full_line_text = original_syllable_texts.join("");

                if full_line_text.is_empty() {
                    continue;
                }

                let converted_full_text = convert(&full_line_text, config_name);

                if pinyin_is_same(&full_line_text, &converted_full_text) {
                    let mut converted_chars = converted_full_text.chars();
                    for (i, original_text) in original_syllable_texts.iter().enumerate() {
                        let char_count = original_text.chars().count();
                        let new_syllable_text: String =
                            converted_chars.by_ref().take(char_count).collect();
                        if let Some(syllable) = line.main_syllables.get_mut(i) {
                            syllable.text = new_syllable_text;
                        }
                    }
                } else {
                    warn!(
                        "行转换失败 (字数或读音改变)，回退到逐音节转换。\n  原文: '{}' \n  转换后: '{}'",
                        full_line_text, converted_full_text,
                    );

                    for syllable in line.main_syllables.iter_mut() {
                        if syllable.text.is_empty() {
                            continue;
                        }

                        let original_text = &syllable.text;
                        let converted_text_syllable = convert(original_text, config_name);

                        if pinyin_is_same(original_text, &converted_text_syllable) {
                            syllable.text = converted_text_syllable;
                        } else {
                            let char_by_char_converted: String = original_text
                                .chars()
                                .map(|c| {
                                    let mut char_str = [0u8; 4];
                                    convert(c.encode_utf8(&mut char_str), config_name)
                                })
                                .collect();

                            if pinyin_is_same(original_text, &char_by_char_converted) {
                                syllable.text = char_by_char_converted;
                            } else {
                                warn!(
                                    "音节 '{}' 转换后读音改变，逐字转换也无效。保留原文。",
                                    original_text
                                );
                            }
                        }
                    }
                }

                if line.line_text.is_some() {
                    line.line_text = Some(
                        line.main_syllables
                            .iter()
                            .map(|s| s.text.as_str())
                            .collect(),
                    );
                }
            } else if let Some(text) = line.line_text.as_mut()
                && !text.is_empty()
            {
                let converted_text = convert(text, config_name);
                if pinyin_is_same(text, &converted_text) {
                    *text = converted_text;
                } else {
                    let char_by_char_converted: String = text
                        .chars()
                        .map(|c| {
                            let mut char_str = [0u8; 4];
                            convert(c.encode_utf8(&mut char_str), config_name)
                        })
                        .collect();

                    if pinyin_is_same(text, &char_by_char_converted) {
                        *text = char_by_char_converted;
                    } else {
                        warn!("行 '{}' 转换后读音改变。保留原文。", text);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::converter::types::{ChineseConversionMode, LyricLine, LyricSyllable};

    fn new_line(text: &str) -> LyricLine {
        LyricLine {
            line_text: Some(text.to_string()),
            ..Default::default()
        }
    }

    fn new_syllable_line(syllables: Vec<&str>) -> LyricLine {
        LyricLine {
            main_syllables: syllables
                .into_iter()
                .map(|s| LyricSyllable {
                    text: s.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_convert_function_simple_and_fallback() {
        let text = "简体中文";
        let valid_config = "s2t.json";
        let invalid_config = "non_existent_config.json";
        let converted_text = convert(text, valid_config);
        assert_eq!(converted_text, "簡體中文");
        let fallback_text = convert(text, invalid_config);
        assert_eq!(fallback_text, text);
    }

    #[test]
    fn test_replace_mode_for_simple_line() {
        let processor = ChineseConversionProcessor::new();
        let mut lines = vec![new_line("我是简体字。")];
        let options = ChineseConversionOptions {
            config_name: Some("s2t.json".to_string()),
            mode: ChineseConversionMode::Replace,
            ..Default::default()
        };
        processor.process(&mut lines, &options);
        assert_eq!(lines[0].line_text.as_deref(), Some("我是簡體字。"));
    }

    #[test]
    fn test_replace_mode_syllables_count_unchanged() {
        let processor = ChineseConversionProcessor::new();
        let mut lines = vec![new_syllable_line(vec!["简体", "中文"])];
        let options = ChineseConversionOptions {
            config_name: Some("s2t.json".to_string()),
            mode: ChineseConversionMode::Replace,
            ..Default::default()
        };
        processor.process(&mut lines, &options);
        let syllables: Vec<String> = lines[0]
            .main_syllables
            .iter()
            .map(|s| s.text.clone())
            .collect();
        assert_eq!(syllables, vec!["簡體".to_string(), "中文".to_string()]);
    }

    #[test]
    fn test_replace_mode_syllables_count_changed_fallback() {
        let processor = ChineseConversionProcessor::new();
        let mut lines = vec![new_syllable_line(vec!["我的", "内存"])];
        let options = ChineseConversionOptions {
            config_name: Some("s2twp.json".to_string()),
            mode: ChineseConversionMode::Replace,
            ..Default::default()
        };

        processor.process(&mut lines, &options);

        let syllables: Vec<String> = lines[0]
            .main_syllables
            .iter()
            .map(|s| s.text.clone())
            .collect();

        // 验证不会被错误地转换为 “記憶體”
        assert_eq!(syllables, vec!["我的".to_string(), "內存".to_string()]);
    }

    #[test]
    fn test_add_translation_mode_success() {
        let processor = ChineseConversionProcessor::new();
        let mut lines = vec![new_line("鼠标和键盘")];
        let options = ChineseConversionOptions {
            config_name: Some("s2twp.json".to_string()),
            mode: ChineseConversionMode::AddAsTranslation,
            target_lang_tag: None,
        };

        processor.process(&mut lines, &options);

        assert_eq!(lines[0].translations.len(), 1);
        let translation = &lines[0].translations[0];

        assert_eq!(translation.text, "滑鼠和鍵盤");
        assert_eq!(translation.lang.as_deref(), Some("zh-Hant-TW"));
    }

    #[test]
    fn test_add_translation_mode_skip_if_exists() {
        let processor = ChineseConversionProcessor::new();
        let mut line = new_line("简体");
        line.translations.push(TranslationEntry {
            text: "預設繁體".to_string(),
            lang: Some("zh-Hant".to_string()),
        });
        let mut lines = vec![line];
        let options = ChineseConversionOptions {
            config_name: Some("s2t.json".to_string()),
            mode: ChineseConversionMode::AddAsTranslation,
            target_lang_tag: None,
        };
        processor.process(&mut lines, &options);
        assert_eq!(lines[0].translations.len(), 1);
        assert_eq!(lines[0].translations[0].text, "預設繁體");
    }

    #[test]
    fn test_add_translation_mode_skip_if_lang_cannot_be_deduced() {
        let processor = ChineseConversionProcessor::new();
        let mut lines = vec![new_line("一些文字")];
        let options = ChineseConversionOptions {
            config_name: Some("一个无效的配置名".to_string()),
            mode: ChineseConversionMode::AddAsTranslation,
            target_lang_tag: None,
        };
        processor.process(&mut lines, &options);
        assert!(lines[0].translations.is_empty());
    }
}
