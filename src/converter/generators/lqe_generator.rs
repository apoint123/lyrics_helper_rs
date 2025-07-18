//! # Lyricify Quick Export 格式生成器

use crate::converter::{
    generators,
    processors::metadata_processor::MetadataStore,
    types::{ConvertError, LqeGenerationOptions, LyricFormat, LyricLine},
};

/// LQE 生成的主入口函数。
pub fn generate_lqe(
    lines: &[LyricLine],
    metadata_store: &MetadataStore,
    options: &LqeGenerationOptions,
) -> Result<String, ConvertError> {
    let mut writer = String::new();

    writer.push_str("[Lyricify Quick Export]\n");
    writer.push_str("[version:1.0]\n");

    let lrc_header = metadata_store.generate_lrc_header();
    writer.push_str(&lrc_header);

    if !lrc_header.is_empty() {
        writer.push('\n');
    }

    let main_lang =
        metadata_store.get_single_value(&crate::converter::types::CanonicalMetadataKey::Language);

    let lang_attr = main_lang.map_or("und", |s| s.as_str());

    writer.push_str(&format!(
        "[lyrics: format@{}, language@{}]\n",
        options.main_lyric_format.to_extension_str(),
        lang_attr
    ));

    let main_content = generate_sub_format(lines, metadata_store, options.main_lyric_format)?;
    writer.push_str(&main_content);
    writer.push_str("\n\n");

    let all_translations: Vec<_> = lines.iter().flat_map(|l| l.translations.iter()).collect();
    if !all_translations.is_empty() {
        let trans_lang = all_translations
            .iter()
            .find_map(|t| t.lang.as_ref())
            .map_or("und", |s| s.as_str());
        writer.push_str(&format!(
            "[translation: format@{}, language@{}]\n",
            options.auxiliary_format.to_extension_str(),
            trans_lang
        ));

        let translation_lines: Vec<LyricLine> = lines
            .iter()
            .map(|line| {
                let trans_text = line.translations.first().map(|t| t.text.clone());
                LyricLine {
                    line_text: trans_text,
                    ..line.clone()
                }
            })
            .filter(|l| l.line_text.is_some())
            .collect();

        let translation_content =
            generate_sub_format(&translation_lines, metadata_store, options.auxiliary_format)?;
        writer.push_str(&translation_content);
        writer.push_str("\n\n");
    }

    let all_romanizations: Vec<_> = lines.iter().flat_map(|l| l.romanizations.iter()).collect();
    if !all_romanizations.is_empty() {
        let roma_lang = all_romanizations
            .iter()
            .find_map(|r| r.lang.as_ref())
            .map_or("romaji", |s| s.as_str());
        writer.push_str(&format!(
            "[pronunciation: format@{}, language@{}]\n",
            options.auxiliary_format.to_extension_str(),
            roma_lang
        ));

        let romanization_lines: Vec<LyricLine> = lines
            .iter()
            .map(|line| {
                let roma_text = line.romanizations.first().map(|r| r.text.clone());
                LyricLine {
                    line_text: roma_text,
                    ..line.clone()
                }
            })
            .filter(|l| l.line_text.is_some())
            .collect();

        let romanization_content = generate_sub_format(
            &romanization_lines,
            metadata_store,
            options.auxiliary_format,
        )?;
        writer.push_str(&romanization_content);
    }

    Ok(writer.trim_end().to_string())
}

fn generate_sub_format(
    lines: &[LyricLine],
    metadata_store: &MetadataStore,
    format: LyricFormat,
) -> Result<String, ConvertError> {
    let dummy_options = crate::converter::types::ConversionOptions::default();

    match format {
        LyricFormat::Lrc => {
            generators::lrc_generator::generate_lrc(lines, metadata_store, &dummy_options.lrc)
        }
        LyricFormat::EnhancedLrc => generators::enhanced_lrc_generator::generate_enhanced_lrc(
            lines,
            metadata_store,
            &dummy_options.lrc,
        ),
        LyricFormat::Lys => generators::lys_generator::generate_lys(lines, metadata_store),
        _ => Err(ConvertError::Internal(format!(
            "LQE 生成器不支持将内部区块格式化为 '{format:?}'"
        ))),
    }
}
