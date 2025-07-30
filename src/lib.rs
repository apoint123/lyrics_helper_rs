#![warn(missing_docs)]

//! # Lyrics Helper RS
//!
//! 一个强大的 Rust 库，用于从多个在线音乐服务搜索歌曲、获取歌词，并进行格式转换。
//!
//! ## 主要功能
//!
//! - **歌词获取**: 从 QQ音乐、网易云音乐、酷狗音乐和 AMLL TTML Database 搜索和获取歌词。
//! - **歌词转换**:
//!   - 支持多种输入格式 (LRC, QRC, KRC, YRC, TTML...)。
//!   - 能够合并主歌词、多个翻译文件、多个罗马音文件。
//!   - 支持输出为多种目标格式，如 LRC, KRC, TTML 等。
//!
//! ## 获取歌词
//!
//! ```rust,no_run
//! use lyrics_helper_rs::model::track::Track;
//! use lyrics_helper_rs::{LyricsHelper, SearchMode};
//!
//! async {
//!     let mut helper = LyricsHelper::new();
//!     helper.load_providers().await.unwrap();
//!
//!     let track_to_search = Track {
//!         title: Some("灯火通明"),
//!         artists: Some(&["小蓝背心"]),
//!         album: None,
//!     };
//!     match helper.search_lyrics(&track_to_search, SearchMode::Ordered).await {
//!         Ok(Some(lyrics)) => println!("获取歌词成功！共 {} 行。", lyrics.parsed.lines.len()),
//!         Ok(None) => println!("未找到任何可用的歌词。"),
//!         Err(e) => eprintln!("发生错误: {}", e),
//!     }
//! };
//! ```
//!
//! ## 格式转换
//!
//! ```rust
//! use lyrics_helper_rs::converter::types::{ConversionInput, InputFile, ConversionOptions, LyricFormat};
//! use lyrics_helper_rs::LyricsHelper;
//!
//! let helper = LyricsHelper::new();
//!
//! let main_lyric = InputFile::new(
//!     "[00:01.00]Hello\n[00:02.00]World".to_string(),
//!     LyricFormat::Lrc, None, None
//! );
//! let translation = InputFile::new(
//!     "[00:01.00]你好\n[00:02.00]世界".to_string(),
//!     LyricFormat::Lrc, Some("zh-Hans".to_string()), None
//! );
//!
//! let input = ConversionInput {
//!     main_lyric,
//!     translations: vec![translation],
//!     romanizations: vec![],
//!     target_format: LyricFormat::Ttml,
//!     user_metadata_overrides: None
//! };
//!
//! let options = ConversionOptions::default();
//!
//! match helper.convert_lyrics(input, &options) {
//!     Ok(ttml_output) => {
//!         println!("转换成功！TTML 内容:\n{:?}", ttml_output);
//!     }
//!     Err(e) => {
//!         eprintln!("转换失败: {}", e);
//!     }
//! }
//! ```
pub mod config;
pub mod converter;
pub mod error;
pub mod model;
pub mod providers;
pub mod search;
#[cfg(target_arch = "wasm32")]
pub mod wasm;

use std::{collections::HashSet, pin::Pin};

use futures::future;

pub use crate::{
    error::{LyricsHelperError, Result},
    model::track::{SearchResult, Track},
};

use crate::{
    converter::types::{
        ConversionInput, ConversionOptions, FullConversionResult, ParsedSourceData,
    },
    model::track::FullLyricsResult,
    providers::{
        Provider, amll_ttml_database::AmllTtmlDatabase, kugou::KugouMusic, netease::NeteaseClient,
        qq::QQMusic,
    },
};

// ==========================================================
//  顶层 API
// ==========================================================

/// 顶层歌词助手客户端，封装了所有提供商，为用户提供统一、简单的接口。
///
/// 这是与本库交互的主要入口点。
pub struct LyricsHelper {
    providers: Vec<Box<dyn Provider + Send + Sync>>,
}

/// 定义歌词的搜索策略。
#[derive(Debug, Clone)]
pub enum SearchMode {
    /// 按预设顺序依次搜索提供商。
    ///
    /// 按 `providers` 列表的顺序，逐个尝试，
    /// 并返回从第一个找到可用歌词的提供商处获取的结果。
    Ordered,
    /// 并发搜索所有提供商。
    ///
    /// 同时向所有提供商发起搜索请求，聚合所有结果，
    /// 然后为最高的匹配项获取歌词。这通常能找到最准确的匹配，但开销也最大。
    Parallel,
    /// 只搜索一个特定的提供商。
    ///
    /// 参数是提供商的名称 (例如, "netease", "qqmusic")。
    Specific(String),
    /// 在指定的一个提供商子集中并行搜索。
    Subset(Vec<String>),
}

impl Default for LyricsHelper {
    fn default() -> Self {
        Self::new()
    }
}

impl LyricsHelper {
    /// 创建一个新的、空的 `LyricsHelper` 实例。
    ///
    /// 此时实例只适用于歌词转换功能。
    /// 若要使用歌词搜索和下载功能，必须先调用 `load_providers()` 方法。
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// 初始化并加载所有歌词提供商。
    ///
    /// 这个方法会执行网络请求来准备提供商。
    /// 在使用搜索功能之前，须先调用此方法。
    ///
    /// # 返回
    /// 如果所有提供商都成功或部分成功初始化，则返回 `Ok(())`。
    pub async fn load_providers(&mut self) -> Result<()> {
        type Initializer<'a> = Pin<
            Box<
                dyn Future<Output = (&'static str, Result<Box<dyn Provider + Send + Sync>>)>
                    + Send
                    + 'a,
            >,
        >;

        let initializers: Vec<Initializer<'_>> = vec![
            Box::pin(async {
                (
                    "QQMusic",
                    QQMusic::new().await.map(|p| Box::new(p) as Box<_>),
                )
            }),
            Box::pin(async {
                (
                    "NeteaseClient",
                    NeteaseClient::new_default()
                        .await
                        .map(|p| Box::new(p) as Box<_>),
                )
            }),
            Box::pin(async {
                (
                    "KugouMusic",
                    KugouMusic::new().await.map(|p| Box::new(p) as Box<_>),
                )
            }),
            // Box::pin(async {
            //     (
            //         "MusixmatchClient",
            //         MusixmatchClient::new()
            //             .await
            //             .map(|p| Box::new(p) as Box<dyn Provider>),
            //     )
            // }),
            Box::pin(async {
                (
                    "AmllTtmlDatabase",
                    AmllTtmlDatabase::new().await.map(|p| Box::new(p) as Box<_>),
                )
            }),
        ];

        let results = future::join_all(initializers).await;

        let providers = results
            .into_iter()
            .filter_map(|(name, result)| match result {
                Ok(provider) => {
                    tracing::info!("[Main] Provider '{}' 初始化成功。", name);
                    Some(provider)
                }
                Err(e) => {
                    tracing::error!("[Main] Provider '{}' 初始化失败: {}", name, e);
                    None
                }
            })
            .collect();

        self.providers = providers;
        Ok(())
    }

    /// 在所有支持的音乐平台中并发地搜索歌曲。
    ///
    /// # 参数
    /// * `track_meta` - 一个包含歌曲标题、艺术家等信息的 `Track` 结构体引用。
    ///
    /// # 返回
    /// 一个 `Result`，成功时包含一个 `Vec<SearchResult>`，该向量已按匹配度从高到低排序。
    pub async fn search_track<'a>(&self, track_meta: &Track<'a>) -> Result<Vec<SearchResult>> {
        if self.providers.is_empty() {
            return Err(LyricsHelperError::ProvidersNotInitialized);
        }

        // 为每个提供商创建一个异步的搜索任务。
        let search_futures = self
            .providers
            .iter()
            .map(|provider| search::search_track(provider.as_ref(), track_meta, true));

        let all_results: Vec<SearchResult> = future::join_all(search_futures)
            .await
            .into_iter()
            .filter_map(Result::ok) // 忽略掉在搜索过程中出错的提供商
            .flatten()
            .collect();

        let mut sorted_results = all_results;
        sorted_results.sort_by(|a, b| b.match_type.cmp(&a.match_type));

        let mut unique_results = Vec::new();
        let mut seen_keys = HashSet::new();

        for result in sorted_results {
            let key = (result.provider_name.clone(), result.provider_id.clone());
            if seen_keys.insert(key) {
                unique_results.push(result);
            }
        }

        Ok(unique_results)
    }

    /// 根据提供商名称和歌曲 ID 获取歌词。
    ///
    /// 这些参数通常来自于 `search_track` 方法返回的 `SearchResult` 对象。
    ///
    /// # 参数
    /// * `provider_name` - 提供商的唯一名称, 例如 "qq" 或 "netease"。
    /// * `song_id` - 在该提供商平台上的歌曲ID。
    ///
    /// # 返回
    /// `Result<ParsedSourceData>` - 成功时返回已解析和合并好的歌词数据。
    pub async fn get_lyrics(&self, provider_name: &str, song_id: &str) -> Result<ParsedSourceData> {
        if self.providers.is_empty() {
            return Err(LyricsHelperError::ProvidersNotInitialized);
        }

        if let Some(provider) = self.providers.iter().find(|p| p.name() == provider_name) {
            provider.get_lyrics(song_id).await
        } else {
            Err(LyricsHelperError::ProviderNotSupported(
                provider_name.to_string(),
            ))
        }
    }

    /// 根据提供商名称和歌曲 ID 获取完整的歌词数据。
    ///
    /// 此方法返回包含原始数据和已解析数据的完整结果。
    /// 这些参数通常来自于 `search_track` 方法返回的 `SearchResult` 对象。
    ///
    /// # 参数
    /// * `provider_name` - 提供商的唯一名称, 例如 "qq" 或 "netease"。
    /// * `song_id` - 在该提供商平台上的歌曲ID。
    ///
    /// # 返回
    /// `Result<FullLyricsResult>` - 成功时返回包含原始数据和已解析歌词数据的完整结果。
    pub async fn get_full_lyrics(
        &self,
        provider_name: &str,
        song_id: &str,
    ) -> Result<FullLyricsResult> {
        if self.providers.is_empty() {
            return Err(LyricsHelperError::ProvidersNotInitialized);
        }

        if let Some(provider) = self.providers.iter().find(|p| p.name() == provider_name) {
            provider.get_full_lyrics(song_id).await
        } else {
            Err(LyricsHelperError::ProviderNotSupported(
                provider_name.to_string(),
            ))
        }
    }

    /// 执行一次完整的、多文件的歌词转换。
    ///
    /// # 参数
    /// * `input` - 包含所有源文件内容和格式的 `ConversionInput`。
    /// * `options` - 控制转换过程和输出格式的 `ConversionOptions`。
    ///
    /// # 返回
    /// `Result<String>` - 成功时返回包含目标格式内容的字符串。
    pub fn convert_lyrics(
        &self,
        input: ConversionInput,
        options: &ConversionOptions,
    ) -> Result<FullConversionResult> {
        let options = options.clone();
        Ok(converter::convert_single_lyric(&input, &options)
            .expect("转换任务 panic 了！这不应该发生。"))
    }

    /// 根据指定的策略搜索并获取歌词。
    ///
    /// # 参数
    /// * `track_meta` - 包含歌曲标题、艺术家等信息的 `Track` 结构体引用。
    /// * `mode` - `SearchMode` 枚举，用于定义搜索策略。
    ///
    /// # 返回
    /// * `Ok(Some(ParsedSourceData))` - 如果成功找到并获取了歌词。
    /// * `Ok(None)` - 如果按照指定策略搜索后，未找到任何可用的歌词。
    /// * `Err(LyricsHelperError)` - 如果在过程中发生不可恢复的错误。
    pub async fn search_lyrics(
        &self,
        track_meta: &Track<'_>,
        mode: SearchMode,
    ) -> Result<Option<FullLyricsResult>> {
        if self.providers.is_empty() {
            return Err(LyricsHelperError::ProvidersNotInitialized);
        }

        let providers_to_search = get_providers_for_mode(self, &mode)?;
        if providers_to_search.is_empty() {
            return Ok(None);
        }

        tracing::info!(
            "使用 [{:?}] 模式在 {} 个提供商中搜索歌词...",
            mode,
            providers_to_search.len()
        );

        match mode {
            SearchMode::Ordered | SearchMode::Specific(_) => {
                search_ordered(&providers_to_search, track_meta).await
            }
            SearchMode::Parallel | SearchMode::Subset(_) => {
                search_lyrics_parallel(self, &providers_to_search, track_meta).await
            }
        }
    }
}

/// 对单个提供商执行搜索并获取操作。
async fn search_and_fetch_from_provider(
    provider: &dyn Provider,
    track_meta: &Track<'_>,
) -> Result<Option<FullLyricsResult>> {
    let search_results = search::search_track(provider, track_meta, true).await?;
    if let Some(best_match) = search_results.first() {
        tracing::info!(
            "在提供商 '{}' 中找到匹配项: '{}' (ID: {}), 正在尝试获取歌词...",
            provider.name(),
            best_match.title,
            best_match.provider_id
        );
        return match provider.get_full_lyrics(&best_match.provider_id).await {
            Ok(lyrics_data) => Ok(Some(lyrics_data)),
            Err(LyricsHelperError::LyricNotFound) => {
                tracing::info!(
                    "找到歌曲 '{}'，但提供商 '{}' 没有提供歌词。",
                    best_match.title,
                    provider.name()
                );
                Ok(None)
            }
            Err(e) => {
                tracing::warn!(
                    "从提供商 '{}' 获取歌曲 ID '{}' 的歌词时失败: {}",
                    provider.name(),
                    best_match.provider_id,
                    e
                );
                Err(e)
            }
        };
    }
    Ok(None)
}

async fn search_lyrics_parallel(
    helper: &LyricsHelper,
    providers: &[&(dyn Provider + Send + Sync)],
    track_meta: &Track<'_>,
) -> Result<Option<FullLyricsResult>> {
    let search_futures = providers
        .iter()
        .map(|provider| search::search_track(*provider, track_meta, true));

    let all_results: Vec<SearchResult> = future::join_all(search_futures)
        .await
        .into_iter()
        .filter_map(Result::ok)
        .flatten()
        .collect();
    let mut sorted_results = all_results;
    sorted_results.sort_by(|a, b| b.match_type.cmp(&a.match_type));

    if let Some(best_match) = sorted_results.first() {
        tracing::info!(
            "并行搜索完成。最佳匹配项来自 '{}': '{}' (ID: {}), 正在获取歌词...",
            best_match.provider_name,
            best_match.title,
            best_match.provider_id
        );
        match helper
            .get_full_lyrics(&best_match.provider_name, &best_match.provider_id)
            .await
        {
            Ok(lyrics_data) => Ok(Some(lyrics_data)),
            Err(LyricsHelperError::LyricNotFound) => {
                tracing::info!("最佳匹配项 '{}' 无歌词。", best_match.title);
                Ok(None)
            }
            Err(e) => Err(e),
        }
    } else {
        tracing::info!("并行搜索未找到任何结果。");
        Ok(None)
    }
}

/// 根据 SearchMode 筛选出要使用的提供商列表。
fn get_providers_for_mode<'a>(
    helper: &'a LyricsHelper,
    mode: &SearchMode,
) -> Result<Vec<&'a (dyn Provider + Send + Sync)>> {
    match mode {
        SearchMode::Ordered | SearchMode::Parallel => {
            Ok(helper.providers.iter().map(|p| p.as_ref()).collect())
        }
        SearchMode::Specific(name) => {
            if let Some(provider) = helper.providers.iter().find(|p| p.name() == *name) {
                Ok(vec![provider.as_ref()])
            } else {
                Err(LyricsHelperError::ProviderNotSupported(name.clone()))
            }
        }
        SearchMode::Subset(names) => {
            let selected_providers: Vec<_> = helper
                .providers
                .iter()
                .filter(|p| names.contains(&p.name().to_string()))
                .map(|p| p.as_ref())
                .collect();

            if selected_providers.is_empty() && !names.is_empty() {
                tracing::warn!("在 Subset 模式下，没有找到任何一个指定的、已初始化的提供商。");
            }
            Ok(selected_providers)
        }
    }
}

/// 在提供商上按顺序执行搜索和获取。
/// 返回从第一个找到歌词的提供商处获取的结果。
async fn search_ordered(
    providers: &[&(dyn Provider + Send + Sync)],
    track_meta: &Track<'_>,
) -> Result<Option<FullLyricsResult>> {
    for provider in providers {
        tracing::debug!("正在尝试提供商: '{}'", provider.name());
        match search_and_fetch_from_provider(*provider, track_meta).await? {
            Some(lyrics_result) => {
                tracing::info!("在 '{}' 成功获取到歌词，搜索结束。", provider.name());
                return Ok(Some(lyrics_result));
            }
            None => continue,
        }
    }
    tracing::info!("所有指定提供商都未能找到歌词。");
    Ok(None)
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::converter::generators::ttml_generator;
    use crate::converter::processors::metadata_processor::MetadataStore;
    use crate::converter::types::ConversionOptions;

    fn init_tracing() {
        use tracing_subscriber::{EnvFilter, FmtSubscriber};
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,lyrics_helper_rs=debug"));
        let _ = FmtSubscriber::builder()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    }

    /// 一个完整的端到端测试用例：
    /// 1. 使用网易云音乐搜索一首包含翻译和罗马音的歌曲。
    /// 2. 获取并打印已合并的歌词数据。
    /// 3. 将解析出的数据转换为 TTML 格式。
    #[tokio::test]
    #[ignore]
    async fn test_netease_full_flow() {
        init_tracing();

        let mut helper = LyricsHelper::new();
        helper.load_providers().await.unwrap();

        let track_to_search = Track {
            title: Some("内向都是作曲家"),
            artists: Some(&["Yunomi"]),
            album: None,
        };

        let search_results = helper
            .search_track(&track_to_search)
            .await
            .expect("搜索歌曲失败");

        let netease_match = search_results
            .iter()
            .find(|r| r.provider_name == "netease")
            .expect("在搜索结果中未找到来自网易云音乐的匹配项");

        println!(
            "[INFO] 找到目标匹配: '{}' (Provider: {}, ID: {})",
            netease_match.title, netease_match.provider_name, netease_match.provider_id
        );
        assert_eq!(netease_match.provider_name, "netease");

        let parsed_data = helper
            .get_lyrics(&netease_match.provider_name, &netease_match.provider_id)
            .await
            .expect("获取歌词失败");

        let has_main_lyric = !parsed_data.lines.is_empty();
        let has_translation = parsed_data.lines.iter().any(|l| !l.translations.is_empty());
        let has_romanization = parsed_data
            .lines
            .iter()
            .any(|l| !l.romanizations.is_empty());

        assert!(has_main_lyric, "解析后的主歌词行不应为空");
        assert!(
            has_translation || has_romanization,
            "获取到的歌词既没有翻译也没有罗马音"
        );

        for line in parsed_data.lines.iter().take(5) {
            let main_text = line.line_text.as_deref().unwrap_or("");
            let translation_text = line.translations.first().map_or("N/A", |t| &t.text);
            let romanization_text = line.romanizations.first().map_or("N/A", |r| &r.text);
            println!(
                "  - 主歌词: {main_text}\n    翻译: {translation_text}\n    罗马音: {romanization_text}"
            );
        }

        println!("\n[INFO] 步骤 3: 将解析出的歌词数据转换为 TTML 格式...");
        let metadata_store = MetadataStore::new();
        // 暂时省略元数据部分
        // for (key, values) in &parsed_data.raw_metadata { ... }

        let ttml_options = ConversionOptions {
            ttml: converter::types::TtmlGenerationOptions {
                format: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let ttml_output =
            ttml_generator::generate_ttml(&parsed_data.lines, &metadata_store, &ttml_options.ttml)
                .expect("生成 TTML 失败");

        assert!(ttml_output.starts_with("<tt"), "输出应为有效的 TTML 字符串");

        // println!("{}", ttml_output);
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_lyrics_ordered_mode() {
        init_tracing();
        let mut helper = LyricsHelper::new();
        helper.load_providers().await.unwrap();

        let track_to_search = Track {
            title: Some("风来的夏天"),
            artists: Some(&["小蓝背心"]),
            album: None,
        };

        let result = helper
            .search_lyrics(&track_to_search, SearchMode::Ordered)
            .await
            .expect("顺序搜索模式不应返回错误");

        assert!(result.is_some(), "顺序搜索应找到至少一个结果");
        let lyrics_data = result.unwrap();
        assert!(!lyrics_data.parsed.lines.is_empty(), "获取到的歌词不应为空");
        println!("成功获取到歌词！");
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_lyrics_parallel_mode() {
        init_tracing();
        let mut helper = LyricsHelper::new();
        helper.load_providers().await.unwrap();

        let track_to_search = Track {
            title: Some("人海经过"),
            artists: Some(&["小蓝背心"]),
            album: None,
        };

        let result = helper
            .search_lyrics(&track_to_search, SearchMode::Parallel)
            .await
            .expect("并行搜索模式不应返回错误");

        assert!(result.is_some(), "并行搜索应找到至少一个结果");
        let lyrics_data = result.unwrap();
        assert!(!lyrics_data.parsed.lines.is_empty(), "获取到的歌词不应为空");
        println!("成功获取到歌词！");
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_lyrics_specific_provider_mode() {
        init_tracing();
        let mut helper = LyricsHelper::new();
        helper.load_providers().await.unwrap();

        let track_to_search = Track {
            title: Some("星夏"),
            artists: Some(&["小蓝背心"]),
            album: Some("星夏"),
        };

        let provider_to_test = "amll-ttml-database".to_string();

        println!(
            "\n[INFO] 正在测试 [Specific] 模式 (提供商: {})",
            provider_to_test
        );
        let result = helper
            .search_lyrics(&track_to_search, SearchMode::Specific(provider_to_test))
            .await
            .expect("指定源搜索模式不应返回错误");

        assert!(result.is_some(), "在 AMLL TTML DB 中应能找到《星夏》");
        let lyrics_data = result.unwrap();
        assert!(!lyrics_data.parsed.lines.is_empty(), "获取到的歌词不应为空");
        println!("成功获取到歌词！");
    }
}
