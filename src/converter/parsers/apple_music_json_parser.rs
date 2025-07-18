//! Apple Music JSON 格式解析器。
//!
//! 这个 JSON 内嵌有 Apple Music 样式的 TTML 文件。

use crate::converter::{
    parsers::ttml_parser,
    types::{ConvertError, ParsedSourceData},
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Root {
    data: Vec<DataItem>,
}

#[derive(Debug, Deserialize)]
struct DataItem {
    id: String,
    attributes: Attributes,
}

#[derive(Debug, Deserialize)]
struct Attributes {
    ttml: String,
}

/// 解析 Apple Music JSON 格式的字符串内容。
pub fn parse_apple_music_json(content: &str) -> Result<ParsedSourceData, ConvertError> {
    // 解析JSON结构
    let root: Root = serde_json::from_str(content)
        .map_err(|e| ConvertError::json_parse(e, "解析 Apple Music JSON 失败".to_string()))?;

    // 提取TTML字符串和ID
    let (ttml_string, apple_music_id) = root
        .data
        .into_iter()
        .next()
        .map(|item| (item.attributes.ttml, item.id))
        .ok_or_else(|| {
            ConvertError::InvalidJsonStructure(
                "Apple Music JSON 中 “data” 数组为空或格式错误".to_string(),
            )
        })?;

    let mut parsed_data = ttml_parser::parse_ttml(&ttml_string, &Default::default())?;

    parsed_data
        .raw_metadata
        .entry("AppleMusicId".to_string())
        .or_default()
        .push(apple_music_id);

    parsed_data.source_format = crate::converter::types::LyricFormat::AppleMusicJson;
    parsed_data.raw_ttml_from_input = Some(ttml_string);

    Ok(parsed_data)
}
