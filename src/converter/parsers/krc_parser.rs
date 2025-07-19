//! # KRC 格式解析器

use std::collections::HashMap;

use base64::{Engine, engine::general_purpose};
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

use crate::converter::{
    types::{
        ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData, RomanizationEntry,
        TranslationEntry,
    },
    utils::{normalize_text_whitespace, parse_and_store_metadata, process_syllable_text},
};

/// 匹配 KRC 行级时间戳 `[start,duration]`
static KRC_LINE_TIMESTAMP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[(?P<start>\d+),(?P<duration>\d+)\]")
        .expect("编译 KRC_LINE_TIMESTAMP_REGEX 失败")
});

/// 匹配 KRC 音节级时间戳和文本 `<offset,duration,pitch>text`
static KRC_SYLLABLE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<(?P<offset>\d+),(?P<duration>\d+),(?P<pitch>0)>(?P<text>[^<]*)")
        .expect("编译 KRC_SYLLABLE_REGEX 失败")
});

// 匹配内嵌翻译的 language 标签
static KRC_TRANSLATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[language:(?P<base64>[A-Za-z0-9+/=]+)\]")
        .expect("编译 KRC_TRANSLATION_REGEX 失败")
});

/// 解析 KRC 格式内容到 `ParsedSourceData` 结构。
pub fn parse_krc(content: &str) -> Result<ParsedSourceData, ConvertError> {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut raw_metadata: HashMap<String, Vec<String>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();

    let aux_data = extract_auxiliary_data_from_krc(content)?;

    let mut aux_line_index = 0;

    for (i, line_str) in content.lines().enumerate() {
        let line_num = i + 1;
        let trimmed_line = line_str.trim();

        if trimmed_line.is_empty() {
            continue;
        }

        if trimmed_line.starts_with("[language:") {
            continue;
        }

        if parse_and_store_metadata(trimmed_line, &mut raw_metadata) {
            continue;
        }

        if let Some(line_caps) = KRC_LINE_TIMESTAMP_REGEX.captures(trimmed_line) {
            let line_start_ms: u64 = line_caps["start"].parse().map_err(ConvertError::ParseInt)?;
            let line_duration_ms: u64 = line_caps["duration"]
                .parse()
                .map_err(ConvertError::ParseInt)?;

            let content_after_line_ts = if let Some(m) = line_caps.get(0) {
                &trimmed_line[m.end()..]
            } else {
                warnings.push(format!("第 {line_num} 行: 正则表达式捕获意外失败。"));
                continue;
            };

            let mut syllables: Vec<LyricSyllable> = Vec::new();

            for syl_caps in KRC_SYLLABLE_REGEX.captures_iter(content_after_line_ts) {
                let raw_text = &syl_caps["text"];

                if let Some((clean_text, ends_with_space)) =
                    process_syllable_text(raw_text, &mut syllables)
                {
                    let offset_ms: u64 =
                        syl_caps["offset"].parse().map_err(ConvertError::ParseInt)?;
                    let duration_ms: u64 = syl_caps["duration"]
                        .parse()
                        .map_err(ConvertError::ParseInt)?;
                    let absolute_start_ms = line_start_ms + offset_ms;

                    syllables.push(LyricSyllable {
                        text: clean_text,
                        start_ms: absolute_start_ms,
                        end_ms: absolute_start_ms + duration_ms,
                        duration_ms: Some(duration_ms),
                        ends_with_space,
                    });
                }
            }

            if !syllables.is_empty() {
                let mut translation_entries: Vec<TranslationEntry> = Vec::new();
                if let Some(raw_text) = aux_data.translations.get(aux_line_index) {
                    let normalized_text = normalize_text_whitespace(raw_text);
                    if !normalized_text.is_empty() {
                        translation_entries.push(TranslationEntry {
                            text: normalized_text,
                            lang: Some("zh-Hans".to_string()),
                        });
                    }
                }

                let mut romanization_entries: Vec<RomanizationEntry> = Vec::new();
                if let Some(raw_text) = aux_data.romanizations.get(aux_line_index) {
                    let normalized_text = normalize_text_whitespace(raw_text);
                    if !normalized_text.is_empty() {
                        romanization_entries.push(RomanizationEntry {
                            text: normalized_text,
                            lang: Some("ja-Latn".to_string()),
                            scheme: None,
                        });
                    }
                }

                let full_line_text = syllables
                    .iter()
                    .map(|s| {
                        if s.ends_with_space {
                            format!("{} ", s.text)
                        } else {
                            s.text.clone()
                        }
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_string();

                lines.push(LyricLine {
                    start_ms: line_start_ms,
                    end_ms: line_start_ms + line_duration_ms,
                    line_text: Some(full_line_text),
                    translations: translation_entries,
                    romanizations: romanization_entries,
                    main_syllables: syllables,
                    agent: Some("v1".to_string()),
                    ..Default::default()
                });

                aux_line_index += 1;
            } else {
                warnings.push(format!("第 {line_num} 行: 未找到任何有效的音节。"));
            }
        } else {
            warnings.push(format!("第 {line_num} 行: 未能识别的行格式。"));
        }
    }

    Ok(ParsedSourceData {
        lines,
        raw_metadata,
        warnings,
        source_format: LyricFormat::Krc,
        is_line_timed_source: false,
        ..Default::default()
    })
}

#[derive(Default, Debug)]
struct KrcAuxiliaryData {
    translations: Vec<String>,
    romanizations: Vec<String>,
}

#[derive(Deserialize)]
struct KrcJson {
    content: Vec<KrcContentEntry>,
}

#[derive(Deserialize)]
struct KrcContentEntry {
    #[serde(rename = "lyricContent")]
    lyric_content: Vec<Vec<String>>,
    #[serde(rename = "type")]
    content_type: u8,
}

/// 从 KRC 内容中提取辅助内容。
fn extract_auxiliary_data_from_krc(content: &str) -> Result<KrcAuxiliaryData, ConvertError> {
    let mut aux_data = KrcAuxiliaryData::default();

    if let Some(caps) = KRC_TRANSLATION_REGEX.captures(content) {
        let base64_str = &caps["base64"];
        let decoded_bytes = general_purpose::STANDARD
            .decode(base64_str)
            .map_err(ConvertError::Base64Decode)?;
        let decoded_text = String::from_utf8(decoded_bytes).map_err(ConvertError::FromUtf8)?;

        // 将解码后的文本作为 JSON 解析
        let parsed_json: KrcJson = serde_json::from_str(&decoded_text)
            .map_err(|e| ConvertError::json_parse(e, "KRC 内嵌 JSON".to_string()))?;

        for entry in parsed_json.content {
            let lines: Vec<String> = entry
                .lyric_content
                .iter()
                // 每行翻译本身也是一个数组，将数组内的字符串连接起来
                .map(|line_parts| line_parts.join(""))
                .collect();

            match entry.content_type {
                1 => aux_data.translations = lines,
                0 => aux_data.romanizations = lines,
                _ => {}
            }
        }
    }
    Ok(aux_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_krc_parses_translation_and_romanization() {
        const KRC_WITH_ROMANIZATION: &str = r#"
            [language:eyJjb250ZW50IjpbeyJsYW5ndWFnZSI6MCwibHlyaWNDb250ZW50IjpbWyIgIl0sWyIgIl0sWyIgIl0sWyJcdTUxNzNcdTRFOEVcdTRGNjBcdTY2RkVcdTRFQTRcdTVGODBcdThGQzdcdTc2ODRcdTkwQTNcdTRFMkFcdTRFQkEiXSxbIlx1NUY1M1x1NEY2MFx1NUJGOVx1NjIxMVx1NTE2OFx1NzZEOFx1NTAzRVx1OEJDOVx1NjVGNiJdLFsiXHU2MjExXHU2NzJBXHU4MEZEXHU1NzY2XHU3Mzg3XHU1NzMwXHU5NzU5XHU5NzU5XHU4MDQ2XHU1NDJDIl0sWyJcdTRFMDBcdTVCOUFcdThCQTlcdTRGNjBcdTYxMUZcdTg5QzlcdTUyMzBcdTVCQzJcdTVCREVcdTRFODZcdTU0MjciXSxbIlx1NTJBOFx1NEUwRFx1NTJBOFx1NUMzMVx1NEYxQVx1NTQwM1x1OTE4QiJdLFsiXHU2NjBFXHU2NjBFXHU1QzMxXHU3N0U1XHU5MDUzIl0sWyJcdThGRDlcdTY2MkZcdTYyMTFcdTc2ODRcdTU3NEZcdTRFNjBcdTYwRUYiXSxbIlx1NEVGQlx1NjAyN1x1NzY4NFx1NjBGM1x1NkNENSJdLFsiXHU1MzE2XHU0RjVDXHU2QjhCXHU5MTc3XHU3Njg0XHU4QkREXHU4QkVEIl0sWyJcdTRGMjRcdTVCQjNcdTUyMzBcdTRFODZcdTRGNjAiXSxbIlx1NTNFQVx1NjYyRlx1NUMzMVx1NkI2NFx1NzZGOFx1NEYzNCJdLFsiXHU1M0VBXHU2NjJGXHU1RjdDXHU2QjY0XHU1M0NDXHU2MjRCXHU3NkY4XHU3Mjc1Il0sWyJcdTRGQkZcdTU5ODJcdTZCNjRcdTdGOEVcdTU5N0QiXSxbIlx1NEU1Rlx1OEJCOFx1OEQ4QVx1OTFDRFx1ODk4MVx1NzY4NFx1NEU4Qlx1OEQ4QVx1NEYxQVx1NTcyOFx1NzE5Rlx1NjA4OVx1NzY4NFx1NTczMFx1NjVCOSJdLFsiXHU1MTQwXHU4MUVBXHU1NzMwXHU5NUVBXHU4MDAwXHU3NzQwXHU1MTQ5XHU4MjkyXHU1NDI3Il0sWyJcdTYwRjNcdTg5ODFcdTdEMjdcdTdEMjdcdTU3MzBcdTYyRTVcdTRGNEZcdTRGNjAiXSxbIlx1NTNFQVx1NEUzQVx1NEU4Nlx1ODFFQVx1NURGMSJdLFsiXHU4MDBDXHU2RDNCXHU3Njg0XHU0RUJBIl0sWyJcdTU5N0RcdTUwQ0ZcdTUxNjhcdTgwNUFcdTk2QzZcdTU3MjhcdThGRDlcdTVFQTdcdTU3Q0VcdTVFMDJcdTkxQ0MiXSxbIlx1NzUzMVx1ODg3N1x1NjAxRFx1NUZGNVx1Nzc0MFx1NjdEMFx1NEVCQVx1NzY4NFx1NUU3OFx1Nzk4RiJdLFsiXHU2MjExXHU2QzM4XHU4RkRDXHU5MEZEXHU0RTBEXHU2MTNGXHU1RkQ4XHU4QkIwIl0sWyJcdThGREVcdTRGNjBcdThGRDlcdTc5Q0RcdTUyQThcdTRFMERcdTUyQThcdTU0MDNcdTkxOEJcdTc2ODRcdTU3MzBcdTY1QjkiXSxbIlx1NjIxMVx1OTBGRFx1NUY4OFx1NTU5Q1x1NkIyMiBcdTRGNjBcdTdCMTFcdTc3NDBcdThDMDNcdTRGODNcdTYyMTEiXSxbIlx1NjIxMVx1NUY4OFx1NEY5RFx1OEQ1Nlx1OTBBM1x1NjgzN1x1NzY4NFx1NEY2MCJdLFsiXHU3M0IwXHU1NzI4XHU5QTZDXHU0RTBBXHU1QzMxXHU2MEYzXHU3NTI4Il0sWyJcdThCRERcdThCRURcdTRFRTVcdTU5MTZcdTc2ODRcdTY1QjlcdTZDRDUiXSxbIlx1NUMwNlx1NzIzMVx1NjEwRlx1NEYyMFx1OEZCRVx1N0VEOVx1NEY2MCJdLFsiXHU0RTBEXHU4QkJBXHU2NjJGXHU0RjYwXHU1RkFFXHU3QjExXHU3Njg0XHU5NzYyXHU1RTlFIl0sWyJcdThGRDhcdTY2MkZcdTc1MUZcdTZDMTRcdTc2ODRcdTY4MzdcdTVCNTAiXSxbIlx1OTBGRFx1OEJBOVx1NjIxMVx1ODlDOVx1NUY5N1x1NTNFRlx1NzIzMVx1NTIzMFx1NjVFMFx1NTNFRlx1NjU1MVx1ODM2RiJdLFsiXHU0RTBEXHU4QkJBXHU2NjJGXHU2NkZFXHU1NDExXHU2MjExXHU1NzY2XHU3NjdEXHU3Njg0XHU4RkM3XHU1M0JCIl0sWyJcdThGRDhcdTY2MkZcdTRFMjRcdTRFQkFcdTUxNzFcdTU0MENcdTRFRjBcdTY3MUJcdThGQzdcdTc2ODRcdTg0RERcdTU5MjkgXHU2MjExXHU5MEZEXHU0RTBEXHU0RjFBXHU1RkQ4XHU4QkIwIl0sWyJcdTUzRUFcdTY2MkZcdTVDMzFcdTZCNjRcdTc2RjhcdTRGMzQiXSxbIlx1NTNFQVx1NjYyRlx1NUY3Q1x1NkI2NFx1NTNDQ1x1NjI0Qlx1NzZGOFx1NzI3NSJdLFsiXHU0RkJGXHU1OTgyXHU2QjY0XHU3RjhFXHU1OTdEIl0sWyJcdTRFNUZcdThCQjhcdThEOEFcdTkxQ0RcdTg5ODFcdTc2ODRcdTRFOEJcdThEOEFcdTRGMUFcdTU3MjhcdTcxOUZcdTYwODlcdTc2ODRcdTU3MzBcdTY1QjkiXSxbIlx1NTE0MFx1ODFFQVx1NTczMFx1OTVFQVx1ODAwMFx1Nzc0MFx1NTE0OVx1ODI5Mlx1NTQyNyJdLFsiXHU0RTBEXHU4QkJBXHU2NjJGXHU0RjYwXHU1RkFFXHU3QjExXHU3Njg0XHU5NzYyXHU1RTlFIl0sWyJcdThGRDhcdTY2MkZcdTc1MUZcdTZDMTRcdTc2ODRcdTY4MzdcdTVCNTAiXSxbIlx1OTBGRFx1OEJBOVx1NjIxMVx1ODlDOVx1NUY5N1x1NTNFRlx1NzIzMVx1NTIzMFx1NjVFMFx1NTNFRlx1NjU1MVx1ODM2RiJdLFsiXHU2MjExXHU1NTlDXHU2QjIyXHU0RjYwXHU3Njg0XHU0RTAwXHU1MjA3Il0sWyJcdTRFQ0VcdTRFQ0FcdTVGODBcdTU0MEUiXSxbIlx1NEU1Rlx1NjBGM1x1NkMzOFx1OEZEQ1x1NUMwNlx1NEY2MFx1N0QyN1x1NjJFNSJdLFsiXHU2MEYzXHU4OTgxXHU3RDI3XHU3RDI3XHU1NzMwXHU2MkU1XHU0RjRGXHU0RjYwIl1dLCJ0eXBlIjoxfSx7Imxhbmd1YWdlIjowLCJseXJpY0NvbnRlbnQiOltbInRhIGthICAiLCJoYSBzaGkgICIsIiAgICIsIi0gICIsInlhICAiLCJraSAgIiwibW8gICIsImNoaSAgICIsIihkbyBtbyAgIiwic3UgKSJdLFsic2Ega3UgICIsInNoaSA6ICIsInRhIGthICAiLCJoYSBzaGkgICIsIiAiXSxbInNhIGtreW8gICIsImt1IDogIiwidGEga2EgICIsImhhIHNoaSAgIiwiICJdLFsia2kgbWkgICIsImdhICAiLCJtYSBlICAiLCJuaSAgIiwidHN1ICAiLCJraSAgIiwiYSAgIiwiJ3QgICIsInRlICAiLCJpICAiLCJ0YSAgIiwiaGkgdG8gICIsIm5vICAiLCJrbyAgIiwidG8gIl0sWyJibyBrdSAgIiwibmkgICIsInUgICIsImNoaSAgIiwiYSAgIiwia2UgICIsInRlICAiLCJrdSAgIiwicmUgICIsInRhICAiLCJ0byAgIiwia2kgIl0sWyJzdSBuYSAgIiwibyAgIiwibmkgICIsImtpICAiLCJpICAiLCJ0ZSAgIiwiYSAgIiwiZ2UgICIsInJhICAiLCJyZSAgIiwienUgICIsIm5pICJdLFsic2EgYmkgICIsInNoaSAgIiwiaSAgIiwibyBtbyAgIiwiaSAgIiwid28gICIsInNhICAiLCJzZSAgIiwidGUgICIsInNoaSAgIiwibWEgICIsIid0ICAiLCJ0YSAgIiwibmUgIl0sWyJzdSAgIiwiZ3UgICIsIm5pICAiLCJ5YSAgIiwia2kgICIsIm1vICAiLCJjaGkgICIsInlhICAiLCJrdSAgIiwibm8gICIsImdhICJdLFsiYm8ga3UgICIsIm5vICAiLCJ3YSBydSAgIiwiaSAgIiwia3Ugc2UgICIsImRhICAiLCJ0dGUgICIsIiAiXSxbIndhICAiLCJrYSAgIiwiJ3QgICIsInRlICAiLCJpICAiLCJ0YSAgIiwiaGEgenUgICIsIm5hICAiLCJubyAgIiwibmkgIl0sWyJqaSBidSAgIiwibiAgIiwia2EgICIsInR0ZSAgIiwibmEgICIsIm8gbW8gICIsImkgICIsImdhICJdLFsiemEgbiAgIiwia28ga3UgICIsIm5hICAiLCJrbyB0byAgIiwiYmEgICIsIm5pICAiLCJuYSAgIiwiJ3QgICIsInRlICJdLFsia2kgbWkgICIsIndvICAiLCJraSB6dSAgIiwidHN1ICAiLCJrZSAgIiwidGUgICIsInRhICJdLFsiaSAgIiwic3NobyAgIiwibmkgICIsImkgICIsInJhICAiLCJyZSAgIiwicnUgICIsImRhICAiLCJrZSAgIiwiZGUgIl0sWyJ0ZSAgIiwidG8gICIsInRlICAiLCJ3byAgIiwia2Egc2EgICIsIm5lICAiLCJhICAiLCJlICAiLCJydSAgIiwiZGEgICIsImtlICAiLCJkZSAiXSxbInlvICAiLCJrYSAgIiwidHN1ICAiLCJ0YSAgIiwibmUgIl0sWyJ0YSBpICAiLCJzZSB0c3UgICIsIm5hICAiLCJrbyB0byAgIiwiaG8gICIsImRvICAiLCJtaSAgIiwibmEgICIsInJlICAiLCJ0YSAgIiwiYmEgICIsInNobyAgIiwiZGUgIl0sWyJrYSBnYSB5YSAgIiwia3UgICIsIm5vICAiLCJrYSAgIiwibW8gICIsInNoaSAgIiwicmUgICIsIm5hICAiLCJpICJdLFsia2kgbWkgICIsIndvICAiLCJ0c3UgeW8gICIsImt1ICAiLCJkYSAgIiwia2kgICIsInNoaSAgIiwibWUgICIsInRhICAiLCJpICJdLFsiamkgYnUgICIsIm4gICIsIm5vICAiLCJ0YSAgIiwibWUgICIsImRhICAiLCJrZSAgIiwibmkgIl0sWyJpICAiLCJraSAgIiwidGUgICIsImkgICIsInJ1ICAiLCJoaSB0byAgIiwiZ2EgIl0sWyJhIHRzdSAgIiwibWUgICIsInJhICAiLCJyZSAgIiwidGEgICIsInlvICAiLCJ1ICAiLCJuYSAgIiwia28gICIsIm5vICAiLCJtYSBjaGkgICIsImRlICJdLFsiZGEgcmUgICIsImthICAiLCJ3byAgIiwia28ga28gcm8gICIsImthICAiLCJyYSAgIiwibyBtbyAgIiwiZSAgIiwicnUgICIsInNoaSBhICAiLCJ3YSBzZSAgIiwid28gIl0sWyJpICAiLCJ0c3UgICIsIm1hICAiLCJkZSAgIiwibW8gICIsIndhIHN1ICAiLCJyZSAgIiwidGEgICIsImt1ICAiLCJuYSAgIiwiaSAiXSxbInN1ICAiLCJndSAgIiwibmkgICIsInlhICAiLCJraSAgIiwibW8gICIsImNoaSAgIiwieWEgICIsImt1ICAiLCJ0byAgIiwia28gICIsIm1vICJdLFsic3UgICIsImtpICAiLCJkYSAgIiwieW8gICIsIid0ICAiLCJ0ZSAgIiwia2EgICIsInJhICAiLCJrYSAgIiwiJ3QgICIsInRlICJdLFsid2EgcmEgICIsInUgICIsImtpIG1pICAiLCJuaSAgIiwiYSBtYSAgIiwiZSAgIiwidGUgICIsImkgICIsInRhICJdLFsiYSBpICAiLCJzaGkgICIsInRlICAiLCJpICAiLCJydSAgIiwia28gICIsInRvICAiLCJ3byAiXSxbImtvIHRvICAiLCJiYSAgIiwiaSBnYSAgIiwiaSAgIiwibm8gICIsImhvIHUgICIsImhvIHUgICIsImRlICJdLFsiaSBtYSAgIiwic3UgICIsImd1ICAiLCJuaSAgIiwidHN1IHRhICAiLCJlICAiLCJ0YSAgIiwiaSAiXSxbImhvIGhvICAiLCJlICAiLCJuICAiLCJkZSAgIiwia3UgICIsInJlICAiLCJ0YSAgIiwia2EgbyAgIiwibW8gIl0sWyJvIGtvICAiLCIndCAgIiwidGEgICIsImthIG8gICIsIm1vICJdLFsiaSB0byAgIiwic2hpICAiLCJrdSAgIiwidGUgICIsInNoaSBrYSAgIiwidGEgICIsIm5hICAiLCJrYSAgIiwidHN1ICAiLCJ0YSAgIiwieW8gIl0sWyJ1ICAiLCJjaGkgICIsImEgICIsImtlICAiLCJ0ZSAgIiwia3UgICIsInJlICAiLCJ0YSAgIiwia2EgICIsImtvICAiLCJtbyAiXSxbImZ1IHRhICAiLCJyaSAgIiwiZ2EgICIsIm1pICAiLCJ0YSAgIiwiYSBvICAiLCJ6byByYSAgIiwibW8gICIsIndhIHN1ICAiLCJyZSAgIiwibmEgICIsImkgIl0sWyJpICAiLCJzc2hvICAiLCJuaSAgIiwiaSAgIiwicmEgICIsInJlICAiLCJydSAgIiwiZGEgICIsImtlICAiLCJkZSAiXSxbInRlICAiLCJ0byAgIiwidGUgICIsIndvICAiLCJrYSBzYSAgIiwibmUgICIsImEgICIsImUgICIsInJ1ICAiLCJkYSAgIiwia2UgICIsImRlICJdLFsieW8gICIsImthICAiLCJ0c3UgICIsInRhICAiLCJuZSAiXSxbInRhIGkgICIsInNlIHRzdSAgIiwibmEgICIsImtvIHRvICAiLCJobyAgIiwiZG8gICIsIm1pICAiLCJuYSAgIiwicmUgICIsInRhICAiLCJiYSAgIiwic2hvICAiLCJkZSAiXSxbImthIGdhIHlhICAiLCJrdSAgIiwibm8gICIsImthICAiLCJtbyAgIiwic2hpICAiLCJyZSAgIiwibmEgICIsImkgIl0sWyJobyBobyAgIiwiZSAgIiwibiAgIiwiZGUgICIsImt1ICAiLCJyZSAgIiwidGEgICIsImthIG8gICIsIm1vICJdLFsibyBrbyAgIiwiJ3QgICIsInRhICAiLCJrYSBvICAiLCJtbyAiXSxbImkgdG8gICIsInNoaSAgIiwia3UgICIsInRlICAiLCJzaGkga2EgICIsInRhICAiLCJuYSAgIiwia2EgICIsInRzdSAgIiwidGEgICIsInlvICJdLFsia2kgbWkgICIsIm5vICAiLCJrbyB0byAgIiwiZ2EgICIsInN1ICAiLCJraSAgIiwiZGEgICIsInlvICJdLFsia28gICIsInJlICAiLCJrYSAgIiwicmEgICIsIm1vICJdLFsienUgICIsInR0byAgIiwiICAiLCJraSBtaSAgIiwid28gICIsImRhICAiLCJraSAgIiwic2hpICAiLCJtZSAgIiwidGEgICIsImkgIl0sWyJraSBtaSAgIiwid28gICIsInRzdSB5byAgIiwia3UgICIsImRhICAiLCJraSAgIiwic2hpICAiLCJtZSAgIiwidGEgICIsImkgIl1dLCJ0eXBlIjowfV0sInZlcnNpb24iOjF9]
            [0,1168]<0,116,0>高<116,116,0>橋<232,116,0>優 <348,116,0>- <464,116,0>ヤ<580,116,0>キ<696,116,0>モ<812,116,0>チ <928,116,0>(吃<1044,116,0>醋)
            [1168,707]<202,76,0>词<177,76,0>：<354,101,0>高<455,100,0>橋<555,152,0>優
            [1875,810]<101,51,0>曲<101,51,0>：<202,152,0>高<355,152,0>橋<507,303,0>優
            [25573,5862]<0,706,0>君<706,608,0>が<1314,555,0>前<1869,406,0>に<2275,505,0>付<2780,352,0>き<3132,303,0>合<3435,303,0>っ<3738,205,0>て<3943,455,0>い<4398,252,0>た<4650,302,0>人<4952,152,0>の<5104,303,0>こ<5407,455,0>と
            [31637,5705]<0,858,0>僕<858,554,0>に<1412,253,0>打<1665,404,0>ち<2069,403,0>明<2472,354,0>け<2826,406,0>て<3232,556,0>く<3788,203,0>れ<3991,402,0>た<4393,353,0>と<4746,959,0>き
            [37998,5151]<0,555,0>素<555,655,0>直<1210,354,0>に<1564,405,0>聴<1969,302,0>い<2271,404,0>て<2675,454,0>あ<3129,505,0>げ<3634,303,0>ら<3937,304,0>れ<4241,404,0>ず<4645,506,0>に
        "#;

        let parsed_data = parse_krc(KRC_WITH_ROMANIZATION).expect("解析KRC文件失败");

        let target_line = parsed_data
            .lines
            .iter()
            .find(|line| !line.translations.is_empty())
            .unwrap();

        assert_eq!(target_line.translations.len(), 1);
        let translation = &target_line.translations[0];
        assert_eq!(translation.text, "关于你曾交往过的那个人");

        assert_eq!(target_line.romanizations.len(), 1);
        let romanization = &target_line.romanizations[0];
        assert_eq!(
            romanization.text,
            "ki mi ga ma e ni tsu ki a 't te i ta hi to no ko to"
        );
    }
}
