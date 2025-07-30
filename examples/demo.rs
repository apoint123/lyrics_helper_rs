//! 用于演示 `lyrics-helper` 库的核心功能。
//!
//! ## 如何运行
//!
//! ```bash
//! cargo run --package lyrics_helper_rs --example demo
//! ```

use std::io::{self, Write};

use lyrics_helper_rs::converter::processors::agent_recognizer;
use lyrics_helper_rs::error::Result;
use lyrics_helper_rs::model::track::Track;
use lyrics_helper_rs::providers::{
    Provider, kugou::KugouMusic, netease::NeteaseClient, qq::QQMusic,
};
use lyrics_helper_rs::search;
use lyrics_helper_rs::{
    converter::{self, processors::metadata_processor::MetadataStore, types::ConversionOptions},
    providers::amll_ttml_database::AmllTtmlDatabase,
};

use tracing::{Level, error, info};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    info!("正在初始化所有音乐提供商...");

    let providers: Vec<Box<dyn Provider>> = vec![
        Box::new(QQMusic::new().await?),
        Box::new(NeteaseClient::new_default().await?),
        Box::new(KugouMusic::new().await?),
        Box::new(AmllTtmlDatabase::new().await?),
    ];
    info!("提供商初始化完成，共 {} 个。", providers.len());

    // 这里硬编码了一首歌作为示例。在实际应用中，这些信息来自用户输入或文件元数据。
    let track_to_search = Track {
        title: Some("有点甜"),
        artists: Some(&["汪苏泷"]),
        album: Some("万有引力"),
    };
    info!(
        "准备搜索歌曲: '{}' - '{}'",
        track_to_search.title.unwrap_or_default(),
        track_to_search.artists.unwrap_or_default().join(", ")
    );

    let search_results =
        search::search_track_in_providers(&providers, &track_to_search, true).await?;

    if search_results.is_empty() {
        error!("在所有提供商中均未找到相关的歌词，程序退出。");
        return Ok(());
    }

    let chosen_index = prompt_user_for_selection(&search_results)?;

    let selected_result = &search_results[chosen_index];
    info!(
        "选择了来自 '{}' 的歌词 '{}'。正在获取歌词内容...",
        selected_result.provider_name, selected_result.title
    );
    let chosen_provider = providers
        .iter()
        .find(|p| p.name() == selected_result.provider_name)
        .ok_or_else(|| {
            lyrics_helper_rs::error::LyricsHelperError::Internal(
                "致命错误：找不到对应的提供商实例".into(),
            )
        })?;
    let song_id_for_lyrics = selected_result
        .provider_id_num
        .map(|id| id.to_string())
        .unwrap_or_else(|| selected_result.provider_id.clone());
    let mut parsed_lyrics = chosen_provider.get_full_lyrics(&song_id_for_lyrics).await?;
    agent_recognizer::recognize_agents(&mut parsed_lyrics.parsed.lines);
    info!("歌词获取并解析成功！");

    // 从解析的歌词文件内部创建基础元数据存储。
    let mut metadata_store = MetadataStore::from(&parsed_lyrics.parsed);

    // 使用从 API 获取的 `SearchResult` 信息来覆盖元数据。
    info!("设置标题: {}", &selected_result.title);
    metadata_store.set_single("title", selected_result.title.clone());

    info!("设置艺术家: {:?}", &selected_result.artists);
    metadata_store.set_multiple("artist", selected_result.artists.clone());

    if let Some(album) = &selected_result.album {
        info!("设置专辑: {}", album);
        metadata_store.set_single("album", album.clone());
    }

    info!("正在将歌词转换为 TTML 格式...");

    let ttml_options = ConversionOptions::default().ttml;
    let ttml_output = converter::generators::ttml_generator::generate_ttml(
        &parsed_lyrics.parsed.lines,
        &metadata_store,
        &ttml_options,
    )?;

    let output_filename = "lyrics.ttml";
    tokio::fs::write(output_filename, &ttml_output).await?;

    info!("转换成功！TTML 歌词已保存到文件: {}", output_filename);
    Ok(())
}

/// 将搜索结果打印到控制台，并提示用户进行选择。
fn prompt_user_for_selection(
    search_results: &[lyrics_helper_rs::model::track::SearchResult],
) -> Result<usize> {
    println!(
        "找到了 {} 条歌词，请选择一个进行转换：\n",
        search_results.len()
    );

    for (index, result) in search_results.iter().enumerate() {
        // 核心信息
        let album_display = result.album.as_deref().unwrap_or("(无专辑信息)");
        println!(
            "  [{:2}] 来源: {:<10} | 匹配度: {:<15} | 标题: {} | 艺术家: {} | 专辑: {}",
            index + 1,
            result.provider_name,
            format!("{:?}", result.match_type),
            result.title,
            result.artists.join(", "),
            album_display
        );

        // 详细 ID 信息
        let numeric_id_display = result
            .provider_id_num
            .map(|id| id.to_string())
            .unwrap_or_else(|| "N/A".to_string());
        println!(
            "       详细信息: Provider ID: {} | 数字 ID: {}",
            result.provider_id, numeric_id_display
        );

        if index < search_results.len() - 1 {
            println!();
        }
    }

    loop {
        print!(
            "请输入你想下载并转换的歌词编号 (1-{}): ",
            search_results.len()
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        match input.trim().parse::<usize>() {
            Ok(num) if num > 0 && num <= search_results.len() => {
                break Ok(num - 1);
            }
            _ => {
                eprintln!("\n输入无效，请输入一个列表中的有效编号。\n");
            }
        }
    }
}
