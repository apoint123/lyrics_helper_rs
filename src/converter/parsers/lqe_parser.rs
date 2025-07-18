//! # LQE 解析器

use std::collections::HashMap;
use tracing::warn;

use crate::converter::{
    merge_lyric_lines,
    types::{ConvertError, InputFile, LyricFormat, ParsedSourceData},
};

#[derive(Clone, Copy)]
enum ParseState {
    Header,
    Lyrics,
    Translation,
    Pronunciation,
}

/// 解析 LQR 格式内容到 `ParsedSourceData` 结构。
pub fn parse_lqe(content: &str) -> Result<ParsedSourceData, ConvertError> {
    if !content.trim_start().starts_with("[Lyricify Quick Export]") {
        return Err(ConvertError::InvalidLyricFormat(
            "文件缺少 [Lyricify Quick Export] 头部标记。".to_string(),
        ));
    }

    let mut main_parsed_vec: Vec<(ParsedSourceData, Option<String>)> = Vec::new();
    let mut translation_parsed_vec: Vec<(ParsedSourceData, Option<String>)> = Vec::new();
    let mut pronunciation_parsed_vec: Vec<(ParsedSourceData, Option<String>)> = Vec::new();
    let mut raw_metadata: HashMap<String, Vec<String>> = HashMap::new();

    let mut current_state = ParseState::Header;
    let mut current_block_content = String::new();
    let mut current_block_format = LyricFormat::Lrc;
    let mut current_block_lang: Option<String> = None;

    let lines = content.lines();

    for line in lines {
        if let Some(captures) = line
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .and_then(|s| s.split_once(':'))
        {
            let key = captures.0.trim();
            if ["lyrics", "translation", "pronunciation"].contains(&key) {
            } else if ["Lyricify Quick Export", "version"].contains(&key) {
                continue;
            } else {
                let value = captures.1.trim().to_string();
                raw_metadata.entry(key.to_string()).or_default().push(value);
                continue;
            }
        }

        if line.starts_with("[lyrics:")
            || line.starts_with("[translation:")
            || line.starts_with("[pronunciation:")
        {
            let parsed_block_result = process_block(
                &current_block_content,
                current_block_format,
                current_block_lang,
            )?;

            if let Some(result) = parsed_block_result {
                match current_state {
                    ParseState::Lyrics => main_parsed_vec.push(result),
                    ParseState::Translation => translation_parsed_vec.push(result),
                    ParseState::Pronunciation => pronunciation_parsed_vec.push(result),
                    ParseState::Header => {}
                }
            }

            current_block_content.clear();

            let (format, lang) = parse_section_header(line);
            current_block_format = format;
            current_block_lang = lang;

            if line.starts_with("[lyrics:") {
                current_state = ParseState::Lyrics;
            } else if line.starts_with("[translation:") {
                current_state = ParseState::Translation;
            } else if line.starts_with("[pronunciation:") {
                current_state = ParseState::Pronunciation;
            }
        } else {
            current_block_content.push_str(line);
            current_block_content.push('\n');
        }
    }

    if let Some(result) = process_block(
        &current_block_content,
        current_block_format,
        current_block_lang,
    )? {
        match current_state {
            ParseState::Lyrics => main_parsed_vec.push(result),
            ParseState::Translation => translation_parsed_vec.push(result),
            ParseState::Pronunciation => pronunciation_parsed_vec.push(result),
            ParseState::Header => {}
        }
    }

    let (mut result, _main_lang) = main_parsed_vec
        .into_iter()
        .next()
        .unwrap_or_else(|| (ParsedSourceData::default(), None));
    result.source_format = LyricFormat::Lqe;
    result.raw_metadata.extend(raw_metadata);

    merge_lyric_lines(
        &mut result.lines,
        &translation_parsed_vec,
        &pronunciation_parsed_vec,
    );

    for (data, _) in translation_parsed_vec
        .iter()
        .chain(pronunciation_parsed_vec.iter())
    {
        result.raw_metadata.extend(data.raw_metadata.clone());
    }

    Ok(result)
}

fn parse_section_header(header_line: &str) -> (LyricFormat, Option<String>) {
    let mut format = LyricFormat::Lrc;
    let mut lang = None;

    if let Some(params_str) = header_line
        .split_once(':')
        .and_then(|(_, rest)| rest.strip_suffix(']'))
    {
        let params: Vec<&str> = params_str.split(',').map(|s| s.trim()).collect();
        for param in params {
            if let Some((key, value)) = param.split_once('@') {
                match key.trim() {
                    "format" => {
                        format = LyricFormat::from_string(value.trim()).unwrap_or_else(|| {
                            warn!("未知的 LQE 区块格式 '{}', 将回退到 LRC", value);
                            LyricFormat::Lrc
                        });
                    }
                    "language" => lang = Some(value.trim().to_string()),
                    _ => {}
                }
            }
        }
    }
    (format, lang)
}

fn process_block(
    content: &str,
    format: LyricFormat,
    lang: Option<String>,
) -> Result<Option<(ParsedSourceData, Option<String>)>, ConvertError> {
    if content.trim().is_empty() {
        return Ok(None);
    }

    let input_file = InputFile::new(content.to_string(), format, lang.clone(), None);

    let parsed_data = crate::converter::parse_input_file(&input_file)?;

    Ok(Some((parsed_data, lang)))
}
