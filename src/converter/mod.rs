//! 歌词转换器核心模块

pub mod generators;
pub mod parsers;
pub mod processors;
pub mod types;
pub mod utils;

use std::collections::HashMap;

pub use types::{LyricFormat, LyricLine, LyricSyllable};

use crate::converter::{
    processors::{
        batch_processor, chinese_conversion_processor::ChineseConversionProcessor,
        metadata_processor::MetadataStore,
    },
    types::{
        ConversionInput, ConversionOptions, ConversionResult, ConversionTask, ConvertError,
        FullConversionResult, InputFile, ParsedSourceData,
    },
};
use tracing::{debug, warn};

// ==========================================================
//  顶级转换入口
// ==========================================================

/// 处理一个转换任务，可以是单文件转换或批量转换。
///
/// # 参数
///
/// * `task` - 一个 `ConversionTask` 枚举，描述了要执行的具体操作（单个或批量）。
/// * `options` - 一个 `ConversionOptions` 引用，包含应用于所有转换的配置选项。
///
/// # 返回
///
/// * `Result<ConversionResult, ConvertError>` - 成功时返回包含转换结果的 `ConversionResult`，
///   失败时返回具体的 `ConvertError`。
pub fn process_conversion_task(
    task: ConversionTask,
    options: &ConversionOptions,
) -> Result<ConversionResult, ConvertError> {
    match task {
        ConversionTask::Single(input) => {
            let full_result = convert_single_lyric(&input, options)?;
            Ok(ConversionResult::Single(full_result.output_lyrics))
        }
        ConversionTask::Batch(batch_input) => {
            let file_groups = batch_processor::discover_and_pair_files(&batch_input.input_dir)?;
            let (mut tasks, file_lookup) =
                batch_processor::create_batch_tasks(file_groups, batch_input.target_format);

            if tasks.is_empty() {
                warn!("在输入目录中未找到可执行的转换任务。");
                return Ok(ConversionResult::Batch(Vec::new()));
            }

            batch_processor::execute_batch_conversion(
                &mut tasks,
                &file_lookup,
                &batch_input.output_dir,
                options,
            )?;

            Ok(ConversionResult::Batch(tasks))
        }
    }
}

// ==========================================================
//  核心转换与生成逻辑
// ==========================================================

/// 将一个 `ConversionInput` 转换为指定格式的字符串。
///
/// # 参数
///
/// * `input` - 包含所有源文件信息和目标格式的 `ConversionInput`。
/// * `options` - 转换过程中的配置选项。
///
/// # 返回
///
/// * `Result<String, ConvertError>` - 成功时返回生成的目标格式字符串。
pub fn convert_single_lyric(
    input: &ConversionInput,
    options: &ConversionOptions,
) -> Result<FullConversionResult, ConvertError> {
    let mut source_data = parse_and_merge(input, options)?;
    let mut processed_data = source_data.clone();

    let mut metadata_store = MetadataStore::new();

    if let Some(overrides) = &input.user_metadata_overrides {
        for (key, values) in overrides {
            for value in values {
                let _ = metadata_store.add(key, value.clone());
            }
        }
        source_data.raw_metadata = overrides.clone();
    } else {
        for (key, values) in processed_data.raw_metadata.iter() {
            for value in values {
                if metadata_store.add(key, value.clone()).is_err() {
                    warn!(
                        "元数据键 '{}' 无法被规范化，但其值 '{}' 仍被添加。",
                        key, value
                    );
                }
            }
        }
    }
    metadata_store.deduplicate_values();

    let chinese_processor = ChineseConversionProcessor::new();
    chinese_processor.process(&mut processed_data.lines, &options.chinese_conversion);

    let output_lyrics = match input.target_format {
        LyricFormat::Lrc => generators::lrc_generator::generate_lrc(
            &processed_data.lines,
            &metadata_store,
            &options.lrc,
        ),
        LyricFormat::EnhancedLrc => generators::enhanced_lrc_generator::generate_enhanced_lrc(
            &processed_data.lines,
            &metadata_store,
            &options.lrc,
        ),
        LyricFormat::Ass => generators::ass_generator::generate_ass(
            &processed_data.lines,
            &metadata_store,
            false,
            &options.ass,
        ),
        LyricFormat::Ttml => generators::ttml_generator::generate_ttml(
            &processed_data.lines,
            &metadata_store,
            &options.ttml,
        ),
        LyricFormat::AppleMusicJson => {
            generators::apple_music_json_generator::generate_apple_music_json(
                &processed_data.lines,
                &metadata_store,
                options,
            )
        }
        LyricFormat::Qrc => {
            generators::qrc_generator::generate_qrc(&processed_data.lines, &metadata_store)
        }
        LyricFormat::Lqe => generators::lqe_generator::generate_lqe(
            &processed_data.lines,
            &metadata_store,
            &options.lqe,
        ),
        LyricFormat::Krc => {
            generators::krc_generator::generate_krc(&processed_data.lines, &metadata_store)
        }
        LyricFormat::Yrc => {
            generators::yrc_generator::generate_yrc(&processed_data.lines, &metadata_store)
        }
        LyricFormat::Lys => {
            generators::lys_generator::generate_lys(&processed_data.lines, &metadata_store)
        }
        LyricFormat::Spl => {
            generators::spl_generator::generate_spl(&processed_data.lines, &metadata_store)
        }
        LyricFormat::Lyl => generators::lyricify_lines_generator::generate_lyl(
            &processed_data.lines,
            &metadata_store,
        ),
        // LyricFormat::Musixmatch => Err(ConvertError::Internal(format!(
        //     "目前还不支持目标格式 '{:?}'",
        //     input.target_format
        // ))),
    }?;

    Ok(FullConversionResult {
        output_lyrics,
        source_data,
    })
}

/// 解析并合并一个包含主歌词、翻译和罗马音的完整输入。
///
/// # 参数
/// * `input` - 包含所有源文件信息的 `ConversionInput`。
/// * `options` - 转换过程中的配置选项，主要用于元数据处理。
///
/// # 返回
/// * `Result<ParsedSourceData, ConvertError>` - 成功时返回一个包含所有合并信息的 `ParsedSourceData`。
///
pub fn parse_and_merge(
    input: &ConversionInput,
    options: &ConversionOptions,
) -> Result<ParsedSourceData, ConvertError> {
    let mut main_parsed = parse_input_file(&input.main_lyric)?;

    let translation_parsed_data: Vec<_> = input
        .translations
        .iter()
        .map(parse_input_file_with_lang)
        .collect::<Result<_, _>>()?;

    let romanization_parsed_data: Vec<_> = input
        .romanizations
        .iter()
        .map(parse_input_file_with_lang)
        .collect::<Result<_, _>>()?;

    for (parsed, _lang) in translation_parsed_data
        .iter()
        .chain(romanization_parsed_data.iter())
    {
        main_parsed.raw_metadata.extend(parsed.raw_metadata.clone());
    }

    merge_lyric_lines(
        &mut main_parsed.lines,
        &translation_parsed_data,
        &romanization_parsed_data,
    );

    processors::metadata_stripper::strip_descriptive_metadata_lines(
        &mut main_parsed.lines,
        &options.metadata_stripper,
    );

    Ok(main_parsed)
}

// ==========================================================
//  辅助函数
// ==========================================================

/// 根据指定的格式解析单个歌词文件内容。
///
/// 这是一个底层的分派函数。
fn parse_input_file(file: &InputFile) -> Result<ParsedSourceData, ConvertError> {
    debug!("正在解析文件，格式为: {:?}", file.format);
    match file.format {
        LyricFormat::Lrc => parsers::lrc_parser::parse_lrc(&file.content),
        LyricFormat::EnhancedLrc => parsers::enhanced_lrc_parser::parse_enhanced_lrc(&file.content),
        LyricFormat::Krc => parsers::krc_parser::parse_krc(&file.content),
        LyricFormat::Ass => parsers::ass_parser::parse_ass(&file.content),
        LyricFormat::Ttml => parsers::ttml_parser::parse_ttml(&file.content, &Default::default()),
        LyricFormat::AppleMusicJson => {
            parsers::apple_music_json_parser::parse_apple_music_json(&file.content)
        }
        LyricFormat::Qrc => parsers::qrc_parser::parse_qrc(&file.content),
        LyricFormat::Yrc => parsers::yrc_parser::parse_yrc(&file.content),
        LyricFormat::Lys => parsers::lys_parser::parse_lys(&file.content),
        LyricFormat::Spl => parsers::spl_parser::parse_spl(&file.content),
        LyricFormat::Lqe => parsers::lqe_parser::parse_lqe(&file.content),
        LyricFormat::Lyl => parsers::lyricify_lines_parser::parse_lyl(&file.content),
        // LyricFormat::Musixmatch => parsers::musixmatch_parser::parse(&file.content),
    }
}

/// `parse_input_file` 的一个包装，同时返回解析数据和文件本身的语言标签。
fn parse_input_file_with_lang(
    file: &InputFile,
) -> Result<(ParsedSourceData, Option<String>), ConvertError> {
    Ok((parse_input_file(file)?, file.language.clone()))
}

/// 合并主歌词行与翻译、罗马音数据，将翻译和罗马音按时间戳插入到主歌词行中。
///
/// # 参数
/// * `main_lines` - 主歌词行的可变引用，将被插入翻译和罗马音。
/// * `translations` - 包含翻译歌词数据及其语言标签的元组切片。
/// * `romanizations` - 包含罗马音歌词数据及其语言标签的元组切片。
fn merge_lyric_lines(
    main_lines: &mut [LyricLine],
    translations: &[(ParsedSourceData, Option<String>)],
    romanizations: &[(ParsedSourceData, Option<String>)],
) {
    if translations.is_empty() && romanizations.is_empty() {
        return;
    }

    let mut translations_map: HashMap<u64, Vec<types::TranslationEntry>> = HashMap::new();
    for (trans_data, lang) in translations {
        for trans_line in &trans_data.lines {
            if let Some(text) = trans_line.line_text.clone() {
                let entry = types::TranslationEntry {
                    text,
                    lang: lang.clone(),
                };
                translations_map
                    .entry(trans_line.start_ms)
                    .or_default()
                    .push(entry);
            }
        }
    }

    let mut romanizations_map: HashMap<u64, Vec<types::RomanizationEntry>> = HashMap::new();
    for (roma_data, lang) in romanizations {
        for roma_line in &roma_data.lines {
            if let Some(text) = roma_line.line_text.clone() {
                let entry = types::RomanizationEntry {
                    text,
                    lang: lang.clone(),
                    scheme: None,
                };
                romanizations_map
                    .entry(roma_line.start_ms)
                    .or_default()
                    .push(entry);
            }
        }
    }

    for main_line in main_lines.iter_mut() {
        if let Some(trans_entries) = translations_map.get(&main_line.start_ms) {
            main_line.translations.extend_from_slice(trans_entries);
        }
        if let Some(roma_entries) = romanizations_map.get(&main_line.start_ms) {
            main_line.romanizations.extend_from_slice(roma_entries);
        }
    }
}
