//! 此模块实现了与 Musixmatch API 进行交互的 `Provider`。
//!
//! 不好用。建议不要用。经常提示 captcha 错误，
//! 但用浏览器打开又正常，我不知道这是为什么。
//!
//! API 来源于 https://github.com/Strvm/musicxmatch-api

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use regex::Regex;
use reqwest::Client as ReqwestClient;
use serde::de::DeserializeOwned;
use sha2::Sha256;
use tracing::{debug, error, info, instrument, trace, warn};
use wreq::{Client as WreqClient, header};
use wreq_util::Emulation;

use crate::{
    converter::{
        self,
        types::{ConversionInput, InputFile, LyricFormat},
    },
    error::{LyricsHelperError, Result},
    model::{
        generic,
        track::{FullLyricsResult, RawLyrics, SearchResult},
    },
    providers::Provider,
};

pub mod models;

const BASE_URL: &str = "https://www.musixmatch.com/ws/1.1";
const APP_ID: &str = "web-desktop-app-v1.0";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

static APP_JS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"src="([^"]*/_next/static/chunks/pages/_app-[^"]+\.js)"#).unwrap()
});
static SECRET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"from\(\s*"(.*?)"\s*\.split"#).unwrap());

/// 用于与 Musixmatch API 交互的客户端。
#[derive(Debug, Clone)]
pub struct MusixmatchClient {
    secret_key: Arc<OnceLock<String>>,
    wreq_client: WreqClient,       // 用于大部分 API
    reqwest_client: ReqwestClient, // 用于 richsync API
}

impl Default for MusixmatchClient {
    fn default() -> Self {
        Self::new_sync()
    }
}

fn to_generic_song(track_data: &models::Track) -> generic::Song {
    let cover_url = if !track_data.album_coverart_800x800.is_empty() {
        Some(track_data.album_coverart_800x800.clone())
    } else if !track_data.album_coverart_500x500.is_empty() {
        Some(track_data.album_coverart_500x500.clone())
    } else if !track_data.album_coverart_350x350.is_empty() {
        Some(track_data.album_coverart_350x350.clone())
    } else if !track_data.album_coverart_100x100.is_empty() {
        Some(track_data.album_coverart_100x100.clone())
    } else {
        None
    };

    generic::Song {
        id: track_data.commontrack_id.to_string(),
        name: track_data.track_name.clone(),
        artists: vec![generic::Artist {
            id: track_data.artist_id.to_string(),
            name: track_data.artist_name.clone(),
        }],
        duration: Some(Duration::from_secs(track_data.track_length as u64)),
        album: Some(track_data.album_name.clone()),
        cover_url,
        provider_id: track_data.commontrack_id.to_string(),
        album_id: Some(track_data.album_id.to_string()),
    }
}

impl MusixmatchClient {
    /// 创建一个新的 `MusixmatchClient` 实例。
    pub fn new_sync() -> Self {
        // 许多 API 需要 TLS 指纹伪装
        let headers = header::HeaderMap::new();
        let wreq_client = WreqClient::builder()
            .emulation(Emulation::Chrome131)
            .default_headers(headers)
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap();

        // 然而，逐字歌词接口只能用普通的 reqwest 客户端
        let reqwest_client = ReqwestClient::new();

        Self {
            secret_key: Arc::new(Default::default()),
            wreq_client,
            reqwest_client,
        }
    }

    /// 创建一个新的 `MusixmatchClient` 实例。
    pub async fn new() -> Result<Self> {
        Ok(Self::new_sync())
    }

    #[instrument(skip(self))]
    async fn fetch_secret_key(&self) -> Result<String> {
        info!("正在获取签名密钥...");

        let html_content = self
            .wreq_client
            .get("https://www.musixmatch.com/search")
            .header("user-agent", USER_AGENT)
            .header("Cookie", "mxm_bab=AB")
            .send()
            .await?
            .text()
            .await?;

        let app_js_url = APP_JS_RE
            .captures(&html_content)
            .and_then(|caps| caps.get(1).map(|m| m.as_str()))
            .ok_or_else(|| {
                LyricsHelperError::ApiError("无法在 HTML 中找到 _app.js URL".to_string())
            })?;

        let js_content = self
            .wreq_client
            .get(app_js_url)
            .send()
            .await?
            .text()
            .await?;

        let encoded_string = SECRET_RE
            .captures(&js_content)
            .and_then(|caps| caps.get(1).map(|m| m.as_str()))
            .ok_or_else(|| {
                LyricsHelperError::ApiError("无法在 JS 文件中找到密钥字符串".to_string())
            })?;

        let reversed_string: String = encoded_string.chars().rev().collect();
        let decoded_bytes = base64::engine::general_purpose::STANDARD.decode(reversed_string)?;
        let secret = String::from_utf8(decoded_bytes)?;

        info!("成功获取签名密钥");
        Ok(secret)
    }

    fn generate_signature(&self, url: &str, secret: &str) -> String {
        let current_date = Utc::now().format("%Y%m%d").to_string();
        let message = format!("{url}{current_date}");

        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC-SHA256 应该能接受任意长度的密钥");
        mac.update(message.as_bytes());

        let hash_output = mac.finalize().into_bytes();
        let b64_signature = base64::engine::general_purpose::STANDARD.encode(hash_output);

        format!(
            "&signature={}&signature_protocol=sha256",
            urlencoding::encode(&b64_signature)
        )
    }

    #[instrument(skip(self))]
    async fn request_get<T: DeserializeOwned>(&self, method: &str, params: &str) -> Result<T> {
        if self.secret_key.get().is_none() {
            let secret = self.fetch_secret_key().await?;
            let _ = self.secret_key.set(secret);
        }
        // unwrap 是安全的，因为刚刚保证了值一定已被设置
        let secret = self.secret_key.get().unwrap();

        let base_request_url = format!("{BASE_URL}/{method}?app_id={APP_ID}&format=json&{params}");

        let url_for_request_and_signature = base_request_url.replace("%20", "+");

        let signature = self.generate_signature(&url_for_request_and_signature, secret);
        let final_url = format!("{url_for_request_and_signature}{signature}");

        trace!(final_url = %final_url, "发送最终的 Musixmatch 请求");

        let resp_text = match method {
            "track.richsync.get" => {
                self.reqwest_client
                    .get(&final_url)
                    .send()
                    .await?
                    .text()
                    .await?
            }
            _ => {
                self.wreq_client
                    .get(&final_url)
                    .header("User-Agent", USER_AGENT)
                    .header("Accept", "*/*")
                    .header("Connection", "keep-alive")
                    .send()
                    .await?
                    .text()
                    .await?
            }
        };

        trace!(response_text = %resp_text, "原始 JSON 响应");

        let json_value: serde_json::Value = match serde_json::from_str(&resp_text) {
            Ok(val) => val,
            Err(e) => {
                error!(response_text = %resp_text, "无法解析 Musixmatch 的 JSON 响应: {}", e);
                return Err(LyricsHelperError::JsonParse(e));
            }
        };

        let header_val = &json_value["message"]["header"];
        let status_code = header_val["status_code"].as_i64().unwrap_or(0) as i32;
        let hint = header_val["hint"].as_str();

        match status_code {
            200 => {
                let api_resp: models::ApiResponse<T> =
                    serde_json::from_value(json_value).map_err(|e| {
                        error!("无法将成功的 Musixmatch 响应解析为目标结构: {}", e);
                        LyricsHelperError::JsonParse(e)
                    })?;

                if let Some(body) = api_resp.message.body {
                    Ok(body)
                } else {
                    error!("Musixmatch API 返回 200 但 body 为空");
                    Err(LyricsHelperError::LyricNotFound)
                }
            }
            401 => {
                let hint_str = hint.unwrap_or("unknown");
                error!(status = 401, hint = hint_str, "Musixmatch API 错误");
                Err(LyricsHelperError::ApiError(format!(
                    "Musixmatch API 错误 (401): {hint_str}"
                )))
            }
            404 => Err(LyricsHelperError::LyricNotFound),
            _ => {
                error!(status=%status_code, hint=?hint, "Musixmatch API 错误");
                Err(LyricsHelperError::ApiError(format!(
                    "Musixmatch API 错误 - 状态码: {status_code}, 提示: {hint:?}"
                )))
            }
        }
    }

    #[instrument(skip(self))]
    async fn get_translations(&self, track_id: &str) -> Result<Option<models::Translation>> {
        let method = "crowd.track.translations.get";
        let params =
            format!("translation_fields_set=minimal&selected_language=zh&track_id={track_id}");

        match self
            .request_get::<models::GetTranslationsBody>(method, &params)
            .await
        {
            Ok(body) => {
                let translation = body
                    .translations_list
                    .into_iter()
                    .next()
                    .map(|item| item.translation);
                if translation.is_some() {
                    info!("找到曲目 ID: {} 的中文翻译。", track_id);
                } else {
                    info!("未找到曲目 ID: {} 的中文翻译。", track_id);
                }
                Ok(translation)
            }
            Err(LyricsHelperError::LyricNotFound) => {
                debug!("未找到翻译 (API 返回 404)。");
                Ok(None)
            }
            Err(LyricsHelperError::JsonParse(_)) => {
                debug!("无法解析翻译响应，假定不存在翻译。");
                Ok(None)
            }
            Err(e) => {
                error!("获取翻译时发生意外错误: {}", e);
                Err(e)
            }
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[async_trait]
impl Provider for MusixmatchClient {
    fn name(&self) -> &'static str {
        "musixmatch"
    }

    #[instrument(skip(self, track))]
    async fn search_songs(
        &self,
        track: &crate::model::track::Track<'_>,
    ) -> Result<Vec<SearchResult>> {
        let title = track.title.unwrap_or("");
        let artist_str = track.artists.unwrap_or(&[]).join(" ");

        let query_string = format!("{} {}", title, artist_str.trim());
        let q_query = urlencoding::encode(&query_string);

        let params = format!("q={q_query}&f_has_lyrics=true&page_size=5");

        match self
            .request_get::<models::SearchTrackBody>("track.search", &params)
            .await
        {
            Ok(result) => {
                let search_results = result
                    .track_list
                    .into_iter()
                    .map(|item| {
                        let track_data = item.track;
                        SearchResult {
                            title: track_data.track_name,
                            artists: vec![track_data.artist_name],
                            album: Some(track_data.album_name),
                            duration: Some((track_data.track_length as u64) * 1000),
                            provider_id: track_data.commontrack_id.to_string(),
                            provider_name: self.name().to_string(),
                            provider_id_num: Some(track_data.commontrack_id as u64),
                            match_type: Default::default(),
                            cover_url: None,
                            language: None,
                        }
                    })
                    .collect();
                Ok(search_results)
            }
            Err(e) => match e {
                LyricsHelperError::LyricNotFound => {
                    info!("搜索 '{}' 未找到结果 (API 返回 404)。", params);
                    Ok(vec![])
                }
                _ => Err(e),
            },
        }
    }

    async fn get_full_lyrics(&self, song_id: &str) -> Result<FullLyricsResult> {
        let (raw_main_content, raw_main_format): (String, LyricFormat) = {
            let params = format!("commontrack_id={song_id}");
            match self
                .request_get::<models::GetRichSyncBody>("track.richsync.get", &params)
                .await
            {
                Ok(response) if !response.richsync.richsync_body.is_empty() => {
                    info!("成功获取 RichSync (逐字) 歌词。");
                    (response.richsync.richsync_body, LyricFormat::Musixmatch)
                }
                result => {
                    if let Err(e) = result {
                        warn!("获取 RichSync 歌词失败 ({:?}), 正在回退到 LRC。", e);
                    } else {
                        warn!("获取到空的 RichSync，回退到 LRC。");
                    }

                    let lrc_input_file = get_lrc_input(self, song_id).await?;
                    info!("成功获取 LRC 歌词。");
                    (lrc_input_file.content, lrc_input_file.format)
                }
            }
        };

        let raw_translation_content =
            if let Ok(Some(translation)) = self.get_translations(song_id).await {
                info!("成功获取中文翻译。");
                Some(translation.description)
            } else {
                None
            };

        let main_lyric_input = InputFile {
            content: raw_main_content.clone(),
            format: raw_main_format,
            language: None,
            filename: None,
        };

        let mut translations = Vec::new();
        if let Some(content) = &raw_translation_content {
            translations.push(InputFile {
                content: content.clone(),
                format: LyricFormat::Lrc,
                language: Some("zh-Hans".to_string()),
                filename: None,
            });
        }

        let conversion_input = ConversionInput {
            main_lyric: main_lyric_input,
            translations,
            romanizations: Vec::new(),
            target_format: LyricFormat::Lrc,
        };

        info!("正在合并主歌词与翻译...");
        let parsed_data = converter::parse_and_merge(&conversion_input, &Default::default())?;

        let raw_lyrics = RawLyrics {
            format: raw_main_format.to_string(),
            content: raw_main_content,
            translation: raw_translation_content,
        };

        Ok(FullLyricsResult {
            parsed: parsed_data,
            raw: raw_lyrics,
        })
    }

    async fn get_song_info(&self, song_id: &str) -> Result<generic::Song> {
        let params = format!("commontrack_id={song_id}");

        let result = self
            .request_get::<models::GetTrackBody>("track.get", &params)
            .await?;

        if let Some(track_data) = result.track {
            Ok(to_generic_song(&track_data))
        } else {
            Err(LyricsHelperError::LyricNotFound)
        }
    }

    async fn get_album_info(&self, album_id: &str) -> Result<generic::Album> {
        let method = "album.get";
        let params = format!("album_id={album_id}");
        let result = self
            .request_get::<models::GetAlbumBody>(method, &params)
            .await?;

        if let Some(album_data) = result.album {
            let cover_url = if !album_data.album_coverart_800x800.is_empty() {
                Some(album_data.album_coverart_800x800)
            } else if !album_data.album_coverart_500x500.is_empty() {
                Some(album_data.album_coverart_500x500)
            } else if !album_data.album_coverart_350x350.is_empty() {
                Some(album_data.album_coverart_350x350)
            } else if !album_data.album_coverart_100x100.is_empty() {
                Some(album_data.album_coverart_100x100)
            } else {
                None
            };

            Ok(generic::Album {
                id: album_data.album_id.to_string(),
                name: album_data.album_name,
                artists: Some(vec![generic::Artist {
                    id: album_data.artist_id.to_string(),
                    name: album_data.artist_name,
                }]),
                songs: None,
                description: None,
                release_date: Some(album_data.album_release_date),
                cover_url,
                provider_id: album_data.album_id.to_string(),
            })
        } else {
            Err(LyricsHelperError::ApiError(format!(
                "获取专辑信息失败 (ID: {})",
                album_id
            )))
        }
    }

    async fn get_album_songs(
        &self,
        album_id: &str,
        page: u32,
        page_size: u32,
    ) -> Result<Vec<generic::Song>> {
        let method = "album.tracks.get";
        let params = format!("album_id={album_id}&page={page}&page_size={page_size}");
        let result = self
            .request_get::<models::GetAlbumTracksBody>(method, &params)
            .await?;

        let songs = result
            .track_list
            .iter()
            .map(|item| to_generic_song(&item.track))
            .collect();

        Ok(songs)
    }

    async fn get_album_cover_url(&self, album_id: &str, _: generic::CoverSize) -> Result<String> {
        let album_info = self.get_album_info(album_id).await?;

        let cover_url = album_info.cover_url.ok_or_else(|| {
            LyricsHelperError::ApiError(format!("专辑 (ID: {album_id}) 没有可用的封面图。"))
        })?;

        Ok(cover_url)
    }

    async fn get_singer_songs(
        &self,
        _singer_id: &str,
        _page: u32,
        _page_size: u32,
    ) -> Result<Vec<generic::Song>> {
        Err(LyricsHelperError::ProviderNotSupported(
            "Musixmatch 提供商不支持 `get_singer_songs`".to_string(),
        ))
    }
    async fn get_playlist(&self, _playlist_id: &str) -> Result<generic::Playlist> {
        Err(LyricsHelperError::ProviderNotSupported(
            "Musixmatch 提供商不支持 `get_playlist`".to_string(),
        ))
    }
    async fn get_song_link(&self, _song_id: &str) -> Result<String> {
        Err(LyricsHelperError::ProviderNotSupported(
            "Musixmatch 提供商不支持 `get_song_link`".to_string(),
        ))
    }
}

async fn get_lrc_input(client: &MusixmatchClient, id: &str) -> Result<InputFile> {
    let method = "macro.subtitles.get";
    let params = format!("namespace=lyrics_richsynched&subtitle_format=lrc&commontrack_id={id}");
    let response = client
        .request_get::<models::GetSubtitlesBody>(method, &params)
        .await?;
    let lyric_content = response
        .macro_calls
        .track_subtitles_get
        .message
        .body
        .ok_or(LyricsHelperError::LyricNotFound)?
        .subtitle_list
        .into_iter()
        .next()
        .map(|item| item.subtitle.subtitle_body)
        .ok_or(LyricsHelperError::LyricNotFound)?;

    if lyric_content.is_empty() || lyric_content.starts_with("*******") {
        return Err(LyricsHelperError::LyricNotFound);
    }

    info!("成功获取 LRC 歌词。");
    Ok(InputFile {
        content: lyric_content,
        format: LyricFormat::Lrc,
        language: None,
        filename: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{generic::CoverSize, track::Track};

    const TEST_TRACK_TITLE: &str = "ME!";
    const TEST_TRACK_ARTIST: &str = "Taylor Swift";

    fn init_tracing() {
        use tracing_subscriber::{EnvFilter, FmtSubscriber};
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("trace"));
        let _ = FmtSubscriber::builder()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_and_get_lyrics() {
        init_tracing();
        let client = MusixmatchClient::new_sync();

        let track_meta = Track {
            title: Some(TEST_TRACK_TITLE),
            artists: Some(&[TEST_TRACK_ARTIST]),
            album: None,
        };

        let results = client.search_songs(&track_meta).await.unwrap();
        assert!(!results.is_empty(), "应该至少找到一个结果");

        let song_result = results
            .iter()
            .find(|r| {
                r.title.contains(TEST_TRACK_TITLE)
                    && r.artists.iter().any(|a| a == TEST_TRACK_ARTIST)
            })
            .expect("在搜索结果中找不到预期的歌曲");

        assert!(song_result.title.contains(TEST_TRACK_TITLE));
        assert!(song_result.artists.iter().any(|a| a == TEST_TRACK_ARTIST));

        let lyrics_result = client.get_full_lyrics(&song_result.provider_id).await;
        assert!(
            lyrics_result.is_ok(),
            "获取歌词失败: {:?}",
            lyrics_result.err()
        );

        let full_lyrics = lyrics_result.unwrap();
        let lyrics = full_lyrics.parsed;
        assert!(!lyrics.lines.is_empty(), "解析的歌词不应该为空");

        let has_syllables = lyrics
            .lines
            .iter()
            .any(|line| !line.main_syllables.is_empty());
        assert!(has_syllables, "'{}' 应该包含逐字歌词", TEST_TRACK_TITLE);

        info!("✅ test_search_and_get_lyrics 测试通过！");
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_richsync_lyrics() {
        init_tracing();
        let client = MusixmatchClient::new_sync();
        let commontrack_id = "63145624";

        let method = "track.richsync.get";
        let params = format!("commontrack_id={}", commontrack_id);
        let richsync_response = client
            .request_get::<models::GetRichSyncBody>(method, &params)
            .await;

        assert!(
            richsync_response.is_ok(),
            "获取响应失败: {:?}",
            richsync_response.err()
        );
        let richsync_body = richsync_response.unwrap();

        let inner_json_str = richsync_body.richsync.richsync_body;
        assert!(!inner_json_str.is_empty(), "内嵌的 JSON 字符串不应该为空");
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_infos() {
        init_tracing();
        let client = MusixmatchClient::new_sync();

        let song_id = "63145624";
        let song_info = client.get_song_info(song_id).await.unwrap();
        assert_eq!(song_info.id, song_id);
        assert!(song_info.name.contains("Let's"));
        assert!(song_info.artists.iter().any(|a| a.name == "OneRepublic"));
        assert!(song_info.album_id.is_some());
        assert!(song_info.cover_url.is_some());

        let album_id = song_info.album_id.unwrap();

        let album_info = client.get_album_info(&album_id).await.unwrap();
        assert_eq!(album_info.id, album_id);
        assert!(album_info.cover_url.is_some());

        let album_songs = client.get_album_songs(&album_id, 1, 100).await.unwrap();
        assert!(!album_songs.is_empty());
        assert!(album_songs.iter().any(|s| s.id == song_id));

        info!("✅ test_get_infos 测试通过！");
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_album_cover_url() {
        init_tracing();
        let client = MusixmatchClient::new_sync();

        let track_meta = Track {
            title: Some("目及皆是你"),
            artists: Some(&["小蓝背心"]),
            album: None,
        };

        let search_results = client
            .search_songs(&track_meta)
            .await
            .expect("搜索歌曲失败");

        let song_info = client
            .get_song_info(&search_results[0].provider_id)
            .await
            .expect("获取歌曲详细信息失败");

        let album_id = song_info.album_id.expect("搜索到的歌曲应包含album_id");
        info!("获取到专辑ID: {}", album_id);

        let cover_url_result = client
            .get_album_cover_url(&album_id, CoverSize::Large)
            .await;

        assert!(
            cover_url_result.is_ok(),
            "获取专辑封面失败: {:?}",
            cover_url_result.err()
        );
        let cover_url = cover_url_result.unwrap();

        assert!(!cover_url.is_empty(), "封面URL不应为空");
        assert!(!cover_url.contains("nocover.png"), "返回的URL不应是占位图");
        info!("✅ 成功获取到有效的封面URL: {}", cover_url);

        let invalid_id_result = client.get_album_cover_url("0", CoverSize::Medium).await;
        assert!(invalid_id_result.is_err(), "无效的专辑ID应该返回错误");
        if let Err(e) = invalid_id_result {
            assert!(matches!(e, LyricsHelperError::LyricNotFound));
            info!("✅ 成功捕获到预期的 LyricNotFound 错误: {}", e);
        }
    }
}
