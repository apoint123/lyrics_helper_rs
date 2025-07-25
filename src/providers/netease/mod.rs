//! 此模块实现了与网易云音乐平台进行交互的 `Provider`。
//! API 来源于 https://github.com/NeteaseCloudMusicApiReborn/api

use async_trait::async_trait;
use chrono::Utc;
use reqwest::{
    Client,
    header::{CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue, REFERER, USER_AGENT},
};
use serde::Serialize;
use serde_json::json;

use crate::{
    converter::{
        self, LyricFormat,
        types::{ConversionInput, InputFile},
    },
    error::{LyricsHelperError, Result},
    model::{
        generic::{self, CoverSize},
        track::{FullLyricsResult, RawLyrics, SearchResult},
    },
    providers::Provider,
};

mod crypto;
pub mod models;

const BASE_URL_NETEASE: &str = "https://music.163.com";

// TODO: 允许选择设备类型
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientType {
    PC,
    Android,
    IPhone,
}

#[derive(Debug, Clone)]
struct ClientConfig {
    client_type: ClientType,
    device_id: String,
    os_version: Option<String>,
    app_version: Option<String>,
    version_code: Option<String>,
    mobile_name: Option<String>,
    channel: Option<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            client_type: ClientType::PC,
            device_id: uuid::Uuid::new_v4().to_string(),
            os_version: Some("10".to_string()),
            app_version: Some("3.0.18".to_string()),
            version_code: None,
            mobile_name: None,
            channel: Some("netease".to_string()),
        }
    }
}
/// 网易云音乐的客户端实现。
///
/// 内部持有了为 WEAPI 请求生成的、一次性的随机密钥。
#[derive(Debug, Clone)]
pub struct NeteaseClient {
    /// 为 WEAPI 生成的 16 位随机对称密钥。
    weapi_secret_key: String,
    /// 使用 RSA 公钥加密 `weapi_secret_key` 后得到的结果，作为请求的一部分发送。
    weapi_enc_sec_key: String,
    http_client: Client,
    config: ClientConfig,
}

impl NeteaseClient {
    /// 创建一个新的 NeteaseClient 实例。
    async fn new(config: ClientConfig) -> Result<Self> {
        let weapi_secret_key = crypto::create_secret_key(16);
        let weapi_enc_sec_key = crypto::rsa_encode(
            &weapi_secret_key,
            crypto::PUBKEY_STR_API,
            crypto::MODULUS_STR_API,
        )?;
        let http_client = Client::new();
        Ok(Self {
            weapi_secret_key,
            weapi_enc_sec_key,
            http_client,
            config,
        })
    }

    /// 一个便捷的默认构造函数
    pub async fn new_default() -> Result<Self> {
        Self::new(ClientConfig::default()).await
    }

    /// 辅助函数，用于发送加密的 WEAPI 请求。
    async fn post_weapi<T: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        payload: &T,
    ) -> Result<R> {
        let raw_json_data = serde_json::to_string(payload)?;

        // 第一轮 AES-CBC 加密
        let params_first_pass_base64 =
            crypto::aes_cbc_encrypt_base64(&raw_json_data, crypto::NONCE_STR, crypto::VI_STR)?;

        // 第二轮 AES-CBC 加密
        let final_params_base64 = crypto::aes_cbc_encrypt_base64(
            &params_first_pass_base64,
            &self.weapi_secret_key,
            crypto::VI_STR,
        )?;

        // 构建请求表单
        let form_data = [
            ("params", final_params_base64),
            ("encSecKey", self.weapi_enc_sec_key.clone()),
        ];

        let user_agent = get_user_agent(self.config.client_type);

        let cookie_str = format!(
            "os=pc; osver={}; appver={}; __remember_me=true",
            self.config.os_version.as_deref().unwrap_or(""),
            self.config.app_version.as_deref().unwrap_or("")
        );
        let cookie_value = cookie_str
            .parse::<HeaderValue>()
            .map_err(|e| LyricsHelperError::ApiError(format!("无法解析 WEAPI COOKIE: {e}")))?;

        // 发送 POST 请求
        let response_text = self
            .http_client
            .post(url)
            .header(USER_AGENT, user_agent)
            .header(REFERER, BASE_URL_NETEASE)
            .header(COOKIE, cookie_value)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&form_data)
            .send()
            .await?
            .text()
            .await?;

        if response_text.is_empty() {
            return Err(LyricsHelperError::ApiError(
                "WEAPI 接口返回了空响应。".to_string(),
            ));
        }

        match serde_json::from_str::<R>(&response_text) {
            Ok(data) => Ok(data),
            Err(e) => {
                if response_text.contains("\"code\":400") && response_text.contains("参数错误")
                {
                    Err(LyricsHelperError::ApiError(
                        "网易云 API 返回 '参数错误' (code 400)。".to_string(),
                    ))
                } else {
                    Err(LyricsHelperError::from(e))
                }
            }
        }
    }

    /// 辅助函数，用于发送加密的 EAPI 请求。
    async fn post_eapi<T: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        url_path_segment: &str,
        full_url: &str,
        payload: &T,
    ) -> Result<R> {
        let encrypted_params_hex = crypto::prepare_eapi_params(url_path_segment, payload)?;
        let form_data = [("params", encrypted_params_hex)];

        let user_agent = get_user_agent(self.config.client_type);

        let header_obj = self.build_eapi_header();
        let cookie_str = header_obj.as_object().map_or_else(
            || "".to_string(),
            |map| {
                map.iter()
                    .filter_map(|(k, v)| {
                        v.as_str().and_then(|s| {
                            if !s.is_empty() {
                                Some(format!("{k}={s}"))
                            } else {
                                None
                            }
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            },
        );

        let cookie_value = cookie_str
            .parse::<HeaderValue>()
            .map_err(|e| LyricsHelperError::ApiError(format!("无法解析 EAPI COOKIE: {e}")))?;

        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(user_agent));
        headers.insert(COOKIE, cookie_value);
        headers.insert(REFERER, HeaderValue::from_static(BASE_URL_NETEASE));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );

        let response_text = self
            .http_client
            .post(full_url)
            .headers(headers)
            .form(&form_data)
            .send()
            .await?
            .text()
            .await?;

        serde_json::from_str::<R>(&response_text).map_err(LyricsHelperError::from)
    }

    fn build_eapi_header(&self) -> serde_json::Value {
        let current_time_ms = Utc::now().timestamp_millis();
        let config = &self.config;

        json!({
            "os": match config.client_type {
                ClientType::PC => "pc",
                ClientType::Android => "android",
                ClientType::IPhone => "iPhone OS",
            },
            "appver": config.app_version.as_deref().unwrap_or("8.0.0"),
            "versioncode": config.version_code.as_deref().unwrap_or("140"),
            "osver": config.os_version.as_deref().unwrap_or(""),
            "deviceId": &config.device_id,
            "mobilename": config.mobile_name.as_deref().unwrap_or(""),
            "buildver": Utc::now().timestamp().to_string(),
            "resolution": "1920x1080",
            "channel": config.channel.as_deref().unwrap_or(""),
            "requestId": format!("{}_{:04}", current_time_ms, rand::random::<u16>() % 1000),
            "__csrf": "",
            "MUSIC_U": "",
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[async_trait]
impl Provider for NeteaseClient {
    fn name(&self) -> &'static str {
        "netease"
    }

    async fn search_songs(
        &self,
        track: &crate::model::track::Track<'_>,
    ) -> Result<Vec<SearchResult>> {
        let title = track.title.unwrap_or_default();
        if title.is_empty() {
            return Ok(vec![]);
        }

        let keyword = if let Some(artists) = track.artists.and_then(|a| a.first()) {
            format!("{title} {artists}")
        } else {
            title.to_string()
        };

        // 使用 EAPI 搜索接口
        let url_path = "/api/cloudsearch/pc";
        let full_url = "https://interface.music.163.com/eapi/cloudsearch/pc";
        let payload = json!({
            "s": keyword,
            "type": "1", // 1 代表搜索单曲
            "limit": 30,
            "offset": 0,
            "total": true
        });

        let resp: models::SearchResult = self.post_eapi(url_path, full_url, &payload).await?;

        let search_results = resp
            .result
            .songs
            .into_iter()
            .map(|song| SearchResult {
                title: song.name,
                artists: song.artist_info.into_iter().map(|a| a.name).collect(),
                album: Some(song.album_info.name),
                duration: Some(song.duration), // 网易云的 duration 单位是毫秒
                provider_id: song.id.to_string(),
                provider_name: self.name().to_string(),
                provider_id_num: Some(song.id),
                ..Default::default()
            })
            .collect();

        Ok(search_results)
    }

    async fn get_full_lyrics(&self, id: &str) -> Result<FullLyricsResult> {
        let url_path = "/api/song/lyric/v1";
        let full_url = "https://interface3.music.163.com/eapi/song/lyric/v1";

        let header = self.build_eapi_header();

        let mut payload = json!({ "id": id, "cp": "false", "lv": "0", "kv": "0", "tv": "0", "rv": "0", "yv": "0", "ytv": "0", "yrv": "0", "csrf_token": "" });
        payload["header"] = header;

        let resp: models::LyricResult = self.post_eapi(url_path, full_url, &payload).await?;

        if resp.code != 200 {
            return Err(LyricsHelperError::ApiError(format!(
                "网易云歌词接口返回非 200 状态码: {}",
                resp.code
            )));
        }

        let yrc_content = resp.yrc.as_ref().and_then(|d| {
            if d.lyric.is_empty() {
                None
            } else {
                Some(d.lyric.clone())
            }
        });
        let lrc_content = resp.lrc.as_ref().and_then(|d| {
            if d.lyric.is_empty() {
                None
            } else {
                Some(d.lyric.clone())
            }
        });
        let tlyric_content = resp.tlyric.as_ref().and_then(|d| {
            if d.lyric.is_empty() {
                None
            } else {
                Some(d.lyric.clone())
            }
        });
        let romalrc_content = resp.romalrc.as_ref().and_then(|d| {
            if d.lyric.is_empty() {
                None
            } else {
                Some(d.lyric.clone())
            }
        });

        let (main_format, main_content) = if let Some(content) = yrc_content {
            (LyricFormat::Yrc, content)
        } else if let Some(content) = lrc_content {
            (LyricFormat::Lrc, content)
        } else {
            return Err(LyricsHelperError::LyricNotFound);
        };

        let main_lyric_input = InputFile {
            content: main_content.clone(),
            format: main_format,
            language: None,
            filename: None,
        };

        let mut translations = Vec::new();
        if let Some(content) = &tlyric_content {
            translations.push(InputFile {
                content: content.clone(),
                format: LyricFormat::Lrc,
                language: Some("zh-Hans".to_string()),
                filename: None,
            });
        }

        let mut romanizations = Vec::new();
        if let Some(content) = romalrc_content {
            romanizations.push(InputFile {
                content,
                format: LyricFormat::Lrc,
                language: Some("ja-Latn".to_string()),
                filename: None,
            });
        }

        let conversion_input = ConversionInput {
            main_lyric: main_lyric_input,
            translations,
            romanizations,
            target_format: LyricFormat::Lrc,
            user_metadata_overrides: None,
        };

        let mut parsed_data = converter::parse_and_merge(&conversion_input, &Default::default())?;
        parsed_data.source_name = "netease".to_string();

        let raw_lyrics = RawLyrics {
            format: main_format.to_string(),
            content: main_content,
            translation: tlyric_content,
        };

        Ok(FullLyricsResult {
            parsed: parsed_data,
            raw: raw_lyrics,
        })
    }

    async fn get_album_info(&self, album_id: &str) -> Result<generic::Album> {
        let url = format!("https://music.163.com/weapi/v1/album/{album_id}?csrf_token=");
        let payload = json!({ "csrf_token": "" });

        let response: models::AlbumResult = self.post_weapi(&url, &payload).await?;

        if response.code != 200 {
            let error_message = match response.code {
                -460 => "请求被拒绝 (错误码-460)，可能为 IP 限制".to_string(),
                _ => format!("获取专辑信息失败，接口返回错误码: {}", response.code),
            };
            return Err(LyricsHelperError::ApiError(error_message));
        }

        let netease_album = response.album.ok_or_else(|| {
            LyricsHelperError::ApiError(format!("未能找到ID为 '{album_id}' 的专辑信息。"))
        })?;

        let songs_to_map = if !response.songs.is_empty() {
            response.songs
        } else {
            netease_album.songs
        };

        let tracks = songs_to_map.into_iter().map(Into::into).collect();

        let album_info = generic::Album {
            id: netease_album.id.to_string(),
            name: netease_album.name,
            artists: Some(netease_album.artists.into_iter().map(Into::into).collect()),
            songs: Some(tracks),
            description: None,
            release_date: None,
            cover_url: netease_album.pic_url,
            provider_id: netease_album.id.to_string(),
        };

        Ok(album_info)
    }

    async fn get_playlist(&self, playlist_id: &str) -> Result<generic::Playlist> {
        let url = "https://music.163.com/weapi/v6/playlist/detail";
        let payload = json!({ "id": playlist_id, "n": 1000, "s": "8", "csrf_token": "" });

        let resp: models::PlaylistResult = self.post_weapi(url, &payload).await?;

        let netease_playlist = resp.playlist;
        let generic_playlist = generic::Playlist {
            id: netease_playlist.id.to_string(),
            name: netease_playlist.name,
            cover_url: Some(netease_playlist.cover_img_url),
            creator_name: Some(netease_playlist.creator.nickname),
            description: netease_playlist.description,
            songs: Some(
                netease_playlist
                    .tracks
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ),
        };

        Ok(generic_playlist)
    }

    async fn get_song_info(&self, song_id: &str) -> Result<generic::Song> {
        let url = "https://music.163.com/weapi/v3/song/detail";
        let c_field = json!([{"id": song_id}]).to_string();
        let payload = json!({ "c": c_field, "csrf_token": "" });

        let resp: models::DetailResult = self.post_weapi(url, &payload).await?;

        let netease_song = resp
            .songs
            .into_iter()
            .next()
            .ok_or(LyricsHelperError::LyricNotFound)?;

        Ok(netease_song.into())
    }

    async fn get_song_link(&self, song_id: &str) -> Result<String> {
        let url = "https://music.163.com/weapi/song/enhance/player/url";
        let payload = json!({ "ids": format!("[{}]", song_id), "br": 999000, "csrf_token": "" });

        let resp: models::SongUrlResult = self.post_weapi(url, &payload).await?;

        let song_url_data = resp
            .data
            .into_iter()
            .find(|d| d.id.to_string() == song_id)
            .ok_or(LyricsHelperError::LyricNotFound)?;

        if song_url_data.code == 200 {
            song_url_data.url.ok_or_else(|| {
                LyricsHelperError::ApiError(
                    "获取播放链接失败，可能因 VIP 或版权问题无链接。".into(),
                )
            })
        } else {
            Err(LyricsHelperError::ApiError(format!(
                "获取播放链接失败，接口返回状态码: {}",
                song_url_data.code
            )))
        }
    }

    /// 根据专辑 ID 获取专辑封面的 URL。
    ///
    /// # 参数
    /// * `album_id` - 网易云音乐的专辑 ID。
    /// * `size` - 期望的封面尺寸。
    ///
    /// # 返回
    /// 一个 `Result`，成功时包含一个封面图片的 URL 字符串。
    async fn get_album_cover_url(&self, album_id: &str, size: CoverSize) -> Result<String> {
        let album_info = self.get_album_info(album_id).await?;

        let base_cover_url = album_info.cover_url.ok_or_else(|| {
            LyricsHelperError::ApiError(format!("专辑 (ID: {album_id}) 信息中未找到封面URL。"))
        })?;

        let size_param = match size {
            CoverSize::Thumbnail => "150y150",
            CoverSize::Medium => "400y400",
            CoverSize::Large => "800y800",
        };

        Ok(format!("{base_cover_url}?param={size_param}"))
    }

    async fn get_singer_songs(
        &self,
        singer_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<generic::Song>> {
        let url = "https://music.163.com/weapi/v1/artist/songs";

        let payload = json!({
            "id": singer_id,
            "private_cloud": "true",
            "work_type": 1,
            "order": "hot", // 默认为热门排序，可根据需求设为"time"
            "offset": offset,
            "limit": limit,
            "csrf_token": ""
        });

        let resp: models::ArtistSongsResult = self.post_weapi(url, &payload).await?;

        if resp.code != 200 {
            return Err(LyricsHelperError::ApiError(format!(
                "获取歌手歌曲失败，接口返回错误码: {}",
                resp.code
            )));
        }

        let songs = resp.songs.into_iter().map(Into::into).collect();

        Ok(songs)
    }

    async fn get_album_songs(
        &self,
        album_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<generic::Song>> {
        let url = format!("https://music.163.com/weapi/v1/album/{album_id}");
        let payload = json!({ "csrf_token": "" });

        let resp: models::AlbumContentResult = self.post_weapi(&url, &payload).await?;

        if resp.code != 200 {
            return Err(LyricsHelperError::ApiError(format!(
                "获取专辑歌曲失败，接口返回错误码: {}",
                resp.code
            )));
        }

        // 这个接口不支持服务端分页
        let songs = resp
            .songs
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(Into::into)
            .collect();

        Ok(songs)
    }
}

impl From<models::Song> for generic::Song {
    fn from(song: models::Song) -> Self {
        generic::Song {
            id: song.id.to_string(),
            name: song.name,
            artists: song
                .artist_info
                .into_iter()
                .map(generic::Artist::from)
                .collect(),
            duration: Some(std::time::Duration::from_millis(song.duration)),
            album: Some(song.album_info.name),
            cover_url: song.album_info.pic_url,
            provider_id: song.id.to_string(),
            album_id: Some(song.album_info.id.to_string()),
        }
    }
}

impl From<models::Artist> for generic::Artist {
    fn from(artist: models::Artist) -> Self {
        generic::Artist {
            id: artist.id.to_string(),
            name: artist.name,
        }
    }
}

fn get_user_agent(client_type: ClientType) -> &'static str {
    match client_type {
        ClientType::PC => {
            "Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Safari/537.36 Chrome/91.0.4472.164 NeteaseMusicDesktop/3.0.18.203152"
        }
        ClientType::Android => {
            "NeteaseMusic/9.1.65.240927161425(9001065);Dalvik/2.1.0 (Linux; U; Android 14; 23013RK75C Build/UKQ1.230804.001)"
        }
        ClientType::IPhone => "NeteaseMusic 9.0.90/5038 (iPhone; iOS 16.2; zh_CN)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::track::Track;

    const TEST_SONG_NAME: &str = "明天见";
    const TEST_SINGER_NAME: &str = "小蓝背心";
    const TEST_SONG_ID: &str = "2116402049";
    const TEST_ALBUM_ID: &str = "182985259";

    #[tokio::test]
    #[ignore]
    async fn test_search_songs() {
        let provider = NeteaseClient::new_default().await.unwrap();

        let search_track = Track {
            title: Some(TEST_SONG_NAME),
            artists: Some(&[TEST_SINGER_NAME]),
            album: None,
        };
        let results = provider.search_songs(&search_track).await.unwrap();

        println!(
            "为 '{}({})' 找到 {} 首歌曲",
            TEST_SONG_NAME,
            TEST_SINGER_NAME,
            results.len()
        );
        assert!(!results.is_empty(), "搜索结果不应为空");

        let target_found = results
            .iter()
            .any(|s| s.title == TEST_SONG_NAME && s.artists.iter().any(|a| a == TEST_SINGER_NAME));
        assert!(
            target_found,
            "在搜索结果中未找到目标歌曲 '{} - {}'",
            TEST_SONG_NAME, TEST_SINGER_NAME
        );
        println!("✅ 测试 search_songs 通过");
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_lyrics() {
        let provider = NeteaseClient::new_default().await.unwrap();
        let lyrics = provider.get_lyrics(TEST_SONG_ID).await.unwrap();

        assert!(!lyrics.lines.is_empty(), "解析后的歌词行列表不应为空");
        println!("✅ 测试 get_lyrics 通过，格式: {:?}", lyrics.source_format);
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_album_info() {
        let provider = NeteaseClient::new_default().await.unwrap();
        let album_info = provider.get_album_info(TEST_ALBUM_ID).await.unwrap();

        assert_eq!(album_info.name, "明天见");
        let artists = album_info.artists.expect("专辑应有歌手信息");
        assert_eq!(artists[0].name, TEST_SINGER_NAME);
        println!("✅ 测试 get_album_info 通过: 专辑为 '{}'", album_info.name);
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_playlist() {
        const NEW_SONGS_PLAYLIST_ID: &str = "3779629";
        let provider = NeteaseClient::new_default().await.unwrap();
        let playlist = provider.get_playlist(NEW_SONGS_PLAYLIST_ID).await.unwrap();

        let songs = playlist.songs.as_ref().expect("歌单应包含歌曲列表");
        println!(
            "✅ 测试 get_playlist 通过: 歌单 '{}' 包含 {} 首歌曲。",
            playlist.name,
            songs.len()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_song_info() {
        let provider = NeteaseClient::new_default().await.unwrap();
        let song = provider.get_song_info(TEST_SONG_ID).await.unwrap();

        assert_eq!(song.name, TEST_SONG_NAME);
        assert_eq!(song.artists[0].name, TEST_SINGER_NAME);
        println!(
            "✅ 测试 get_song_info 通过: 歌曲为 '{}' - '{}'",
            song.name, song.artists[0].name
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_song_link() {
        let provider = NeteaseClient::new_default().await.unwrap();
        let link_result = provider.get_song_link(TEST_SONG_ID).await;

        match link_result {
            Ok(link) => {
                assert!(link.starts_with("http"), "返回的链接应以 http/https 开头");
                println!("✅ 测试 get_song_link 通过: 获取到链接: {}", link);
            }
            Err(e) => {
                if let LyricsHelperError::ApiError(msg) = e {
                    println!(
                        "✅ 测试 get_song_link 通过: 已正确处理接口错误 (如VIP或版权问题): {}",
                        msg
                    );
                } else {
                    panic!("测试 get_song_link 因意外的非接口错误而失败: {:?}", e);
                }
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_album_cover_url() {
        let provider = NeteaseClient::new_default().await.unwrap();
        let album_id = "182985259";

        let medium_cover_url = provider
            .get_album_cover_url(album_id, CoverSize::Medium)
            .await
            .expect("获取中等尺寸封面失败");

        assert!(medium_cover_url.contains("?param=400y400"));
        println!("✅ 中等尺寸封面URL正确: {}", medium_cover_url);

        let large_cover_url = provider
            .get_album_cover_url(album_id, CoverSize::Large)
            .await
            .expect("获取大尺寸封面失败");

        assert!(large_cover_url.contains("?param=800y800"));
        println!("✅ 大尺寸封面URL正确: {}", large_cover_url);

        let invalid_id_result = provider
            .get_album_cover_url("99999999999999999999999999999", CoverSize::Medium)
            .await;
        assert!(invalid_id_result.is_err(), "无效的专辑ID应该返回错误");
        if let Err(e) = invalid_id_result {
            assert!(matches!(e, LyricsHelperError::ApiError(_)));
            println!("✅ 成功捕获到预期的 ApiError 错误: {}", e);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_singer_songs() {
        let provider = NeteaseClient::new_default().await.unwrap();

        let singer_id = "12138269";
        let limit = 5;
        let offset = 0;

        let songs_result = provider.get_singer_songs(singer_id, limit, offset).await;

        assert!(songs_result.is_ok(), "获取歌手歌曲列表不应失败");
        let songs = songs_result.unwrap();

        println!(
            "✅ 测试 get_singer_songs 通过: 为歌手ID '{}' 获取到 {} 首歌曲。",
            singer_id,
            songs.len()
        );

        assert_eq!(
            songs.len(),
            limit as usize,
            "返回的歌曲数量应与请求的limit一致"
        );

        // 验证第一页和第二页的内容不同
        let offset_page2 = 5;
        let songs_page2_result = provider
            .get_singer_songs(singer_id, limit, offset_page2)
            .await;

        assert!(songs_page2_result.is_ok(), "获取歌手歌曲列表第二页不应失败");
        let songs_page2 = songs_page2_result.unwrap();

        assert!(!songs_page2.is_empty(), "第二页应有歌曲");
        assert_ne!(
            songs.first().unwrap().id,
            songs_page2.first().unwrap().id,
            "第一页和第二页的歌曲内容应该不同"
        );
        println!("✅ 歌手歌曲分页(offset)测试通过。");
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_album_songs() {
        let provider = NeteaseClient::new_default().await.unwrap();

        let album_id = "182985259";

        let limit1 = 2;
        let offset1 = 0;
        let songs1_result = provider.get_album_songs(album_id, limit1, offset1).await;

        assert!(songs1_result.is_ok(), "获取专辑歌曲第一页不应失败");
        let songs1 = songs1_result.unwrap();

        assert_eq!(songs1.len(), limit1 as usize, "第一页应返回2首歌曲");
        println!(
            "✅ 测试 get_album_songs 通过: 成功获取专辑 '{}' 的前 {} 首歌曲。",
            album_id,
            songs1.len()
        );

        let limit2 = 2;
        let offset2 = 2;
        let songs2_result = provider.get_album_songs(album_id, limit2, offset2).await;

        assert!(songs2_result.is_ok(), "获取专辑歌曲第二页不应失败");
        let songs2 = songs2_result.unwrap();

        assert_eq!(songs2.len(), 1, "第二页应返回剩下的1首歌曲");

        assert_ne!(
            songs1.first().unwrap().id,
            songs2.first().unwrap().id,
            "分页功能验证失败"
        );
        println!("✅ 专辑分页测试通过。");
    }
}
