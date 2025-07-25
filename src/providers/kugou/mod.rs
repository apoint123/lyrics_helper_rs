//! 实现了与酷狗音乐平台进行交互的 `Provider`，
//!
//! API 来源于 https://github.com/MakcRe/KuGouMusicApi
//!
//! # 使用流程
//!
//! 1. 使用 `search_songs` 搜索歌曲，从返回的 `SearchResult` 列表中找到目标歌曲，并获取其 `hash`。
//! 2. 将获取到的 `hash` 作为参数，调用其他函数以执行后续操作：
//!    - 调用 `get_full_lyrics(hash)` 获取歌词。
//!    - 调用 `get_song_info(hash)` 获取该歌曲的详细信息。
//!    - 调用 `get_song_link(hash)` 获取该歌曲的播放链接。

use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use md5::{Digest, Md5};
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use tracing::{info, instrument, warn};

use crate::{
    config::{KugouConfig, load_kugou_config, save_kugou_config},
    converter::{
        self, LyricFormat,
        types::{ConversionInput, ConversionOptions, InputFile},
    },
    error::{LyricsHelperError, Result},
    model::{
        generic::{self, CoverSize},
        track::{FullLyricsResult, RawLyrics, SearchResult},
    },
    providers::Provider,
};

pub mod decrypter;
pub mod models;
pub mod signature;

const KUGOU_ANDROID_USER_AGENT: &str = "Android15-1070-11083-46-0-DiscoveryDRADProtocol-wifi";
const KUGOU_API_GATEWAY: &str = "https://gateway.kugou.com";
const APP_ID: &str = "1005";
const CLIENT_VER: &str = "12569";
const REGISTER_APP_ID: &str = "1014";
const KG_TID: &str = "255";

const X_ROUTER_OPENAPI: &str = "openapi.kugou.com";
const X_ROUTER_COMPLEX_SEARCH: &str = "complexsearch.kugou.com";
const X_ROUTER_PUB_SONGS: &str = "pubsongs.kugou.com";
const X_ROUTER_MEDIA_STORE: &str = "media.store.kugou.com";
const X_ROUTER_TRACKER: &str = "tracker.kugou.com";

/// 酷狗音乐的 Provider 实现
#[derive(Debug, Clone)]
pub struct KugouMusic {
    dfid: String,
    mid: String,
    uuid: String,
    http_client: Client,
}

/// 用于解析注册响应的结构体
#[derive(Deserialize)]
struct RegisterResponse {
    data: RegisterData,
}

#[derive(Deserialize)]
struct RegisterData {
    dfid: String,
}

fn get_current_timestamp_sec_str() -> Result<String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| {
            LyricsHelperError::Internal(format!(
                "你的时间比 1970 年 1 月 1 日还早！请检查一下你的时间: {e}"
            ))
        })
        .map(|d| d.as_secs().to_string())
}

fn strip_artist_from_title<'a>(full_title: &'a str, artists_str: &str) -> &'a str {
    if !artists_str.is_empty()
        && let Some(stripped) = full_title.strip_prefix(&format!("{artists_str} - "))
    {
        return stripped;
    }
    full_title
}

impl KugouMusic {
    fn from_dfid(dfid: String) -> Self {
        let mid = format!("{:x}", Md5::digest(dfid.as_bytes()));
        let uuid_str = format!("{dfid}{mid}");
        let uuid = format!("{:x}", Md5::digest(uuid_str.as_bytes()));
        let http_client = Client::new();

        Self {
            dfid,
            mid,
            uuid,
            http_client,
        }
    }

    /// 创建一个新的 KugouMusic 提供商实例
    async fn register_via_network() -> Result<Self> {
        let http_client = Client::new();

        let clienttime = get_current_timestamp_sec_str()?;

        let register_payload_json = json!({
            "mid": "",
            "uuid": "",
            "appid": REGISTER_APP_ID,
            "userid": "0",
        });
        let encoded_payload = STANDARD.encode(register_payload_json.to_string());

        let mut params_for_sig = BTreeMap::new();
        params_for_sig.insert("appid".to_string(), REGISTER_APP_ID.to_string());
        params_for_sig.insert("clientver".to_string(), CLIENT_VER.to_string());
        params_for_sig.insert("clienttime".to_string(), clienttime.clone());
        params_for_sig.insert("dfid".to_string(), "-".to_string());
        params_for_sig.insert("mid".to_string(), "".to_string());
        params_for_sig.insert("uuid".to_string(), "".to_string());
        params_for_sig.insert("userid".to_string(), "0".to_string());
        params_for_sig.insert("platid".to_string(), "4".to_string());
        params_for_sig.insert("p.token".to_string(), "".to_string());

        let signature = signature::signature_register_params(&params_for_sig);

        let mut final_query_params = params_for_sig;
        final_query_params.insert("signature".to_string(), signature);

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("User-Agent", KUGOU_ANDROID_USER_AGENT.parse().unwrap());
        let header_mid = format!("{:x}", Md5::digest(b"-"));
        headers.insert("mid", header_mid.parse().unwrap());

        let register_url = "https://userservice.kugou.com/risk/v1/r_register_dev";
        let resp = http_client
            .post(register_url)
            .query(&final_query_params)
            .headers(headers)
            .body(encoded_payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(LyricsHelperError::ApiError(format!(
                "酷狗设备注册失败，HTTP状态码: {}",
                resp.status()
            )));
        }

        let response_text = resp
            .text()
            .await
            .map_err(|e| LyricsHelperError::ApiError(format!("读取酷狗注册响应体失败: {e}")))?;

        let json_value: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
            LyricsHelperError::ApiError(format!("解析酷狗注册响应 '{response_text}' 失败: {e}"))
        })?;

        if json_value["status"].as_i64().unwrap_or(0) != 1 {
            return Err(LyricsHelperError::ApiError(format!(
                "酷狗设备注册失败，服务器返回: {response_text}"
            )));
        }

        let register_info: RegisterResponse = serde_json::from_value(json_value).map_err(|e| {
            LyricsHelperError::ApiError(format!("解析酷狗注册响应的data字段失败: {e}"))
        })?;

        let dfid = register_info.data.dfid;

        Ok(Self::from_dfid(dfid))
    }

    /// 公共构造函数，集成了加载和注册逻辑
    pub async fn new() -> Result<Self> {
        if let Ok(config) = load_kugou_config() {
            return Ok(Self::from_dfid(config.dfid));
        }

        info!("未找到 DFID 缓存，正在注册...");
        let new_instance = Self::register_via_network().await?;

        let new_config = KugouConfig {
            dfid: new_instance.dfid.clone(),
        };
        if let Err(e) = save_kugou_config(&new_config) {
            warn!("保存新的 DFID 失败: {}", e);
        }

        Ok(new_instance)
    }

    /// 私有辅助函数，用于执行需要安卓签名的 GET 请求。
    async fn execute_signed_get<R>(
        &self,
        url: &str,
        mut business_params: BTreeMap<String, String>,
        x_router: Option<&str>,
    ) -> Result<R>
    where
        R: DeserializeOwned,
    {
        let clienttime = get_current_timestamp_sec_str()?;
        business_params.insert("appid".to_string(), APP_ID.to_string());
        business_params.insert("clientver".to_string(), CLIENT_VER.to_string());
        business_params.insert("clienttime".to_string(), clienttime);
        business_params.insert("dfid".to_string(), self.dfid.clone());
        business_params.insert("mid".to_string(), self.mid.clone());
        business_params.insert("uuid".to_string(), self.uuid.clone());
        business_params.insert("userid".to_string(), "0".to_string());

        let signature = signature::signature_android_params(&business_params, "", false);
        business_params.insert("signature".to_string(), signature);

        let mut request_builder = self.http_client.get(url).query(&business_params);
        if let Some(router) = x_router {
            request_builder = request_builder.header("x-router", router);
        }

        let response = request_builder
            .header("User-Agent", KUGOU_ANDROID_USER_AGENT)
            .header("kg-tid", KG_TID)
            .send()
            .await?;

        let response_text = response.text().await?;

        tracing::trace!(
            url = url,
            response.body = %response_text,
            "原始 JSON 响应"
        );

        let result: R = serde_json::from_str(&response_text)?;

        Ok(result)
    }

    /// 私有辅助函数，用于执行需要安卓签名的 POST 请求。
    async fn execute_signed_post<P, R>(
        &self,
        url: &str,
        body_payload: &P,
        x_router: Option<&str>,
    ) -> Result<R>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let body_str = serde_json::to_string(body_payload)?;
        let clienttime = get_current_timestamp_sec_str()?;
        let mut params: BTreeMap<String, String> = BTreeMap::new();
        params.insert("appid".to_string(), APP_ID.to_string());
        params.insert("clientver".to_string(), CLIENT_VER.to_string());
        params.insert("clienttime".to_string(), clienttime.clone());
        params.insert("dfid".to_string(), self.dfid.clone());
        params.insert("mid".to_string(), self.mid.clone());
        params.insert("uuid".to_string(), self.uuid.clone());
        params.insert("userid".to_string(), "0".to_string());

        let signature = signature::signature_android_params(&params, &body_str, false);
        params.insert("signature".to_string(), signature);

        let mut request_builder = self.http_client.post(url).query(&params);
        if let Some(router) = x_router {
            request_builder = request_builder.header("x-router", router);
        }
        let response = request_builder
            .header("User-Agent", KUGOU_ANDROID_USER_AGENT)
            .header("kg-tid", KG_TID)
            .header("Content-Type", "application/json")
            .body(body_str)
            .send()
            .await?;

        let response_text = response.text().await?;

        tracing::trace!(
            url = url,
            response.body = %response_text,
            "原始 JSON 响应"
        );

        let result: R = serde_json::from_str(&response_text)?;

        Ok(result)
    }

    /// 为 expendablekmr.kugou.com 域名下的 GET 请求执行签名和发送。
    /// 这个请求的签名和 Header 都很特殊，因此需要独立实现。
    async fn execute_expendable_kmr_get<R>(
        &self,
        mut business_params: BTreeMap<String, String>,
    ) -> Result<R>
    where
        R: DeserializeOwned,
    {
        // 构建用于签名的参数（不包含dfid, mid等）
        let mut params_for_sig = business_params.clone();
        params_for_sig.insert("appid".to_string(), APP_ID.to_string());
        params_for_sig.insert("clientver".to_string(), CLIENT_VER.to_string());

        let signature = signature::signature_android_params(&params_for_sig, "", false);

        // 构建最终请求参数（包含签名，但不包含身份信息）
        business_params.insert("appid".to_string(), APP_ID.to_string());
        business_params.insert("clientver".to_string(), CLIENT_VER.to_string());
        business_params.insert("signature".to_string(), signature);

        // 构建请求，并使用占位符身份设置 Header
        let url = "https://expendablekmr.kugou.com/container/v2/image";
        let mid = format!("{:x}", md5::Md5::digest(b"-"));

        let response = self
            .http_client
            .get(url)
            .query(&business_params)
            .header("User-Agent", KUGOU_ANDROID_USER_AGENT)
            .header("kg-tid", KG_TID)
            .header("dfid", "-")
            .header("mid", mid)
            .send()
            .await?;

        let response_text = response.text().await?;

        tracing::trace!(
            url = url,
            response.body = %response_text,
            "原始 JSON 响应"
        );

        let result: R = serde_json::from_str(&response_text)?;

        Ok(result)
    }

    /// 批量获取封面等图片信息。
    /// API: /container/v2/image
    #[instrument(skip(self, items))]
    pub async fn get_images_batch<'a>(
        &self,
        items: &'a [models::BatchImageDataItem<'a>],
    ) -> Result<models::BatchImageResponse> {
        let data_str = serde_json::to_string(items)?;
        let mut params = BTreeMap::new();
        params.insert("album_image_type".to_string(), "-3".to_string());
        params.insert("author_image_type".to_string(), "3,4,5".to_string());
        params.insert("count".to_string(), items.len().to_string());
        params.insert("data".to_string(), data_str);
        params.insert("isCdn".to_string(), "1".to_string());
        params.insert("publish_time".to_string(), "1".to_string());

        self.execute_expendable_kmr_get(params).await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[async_trait]
impl Provider for KugouMusic {
    fn name(&self) -> &'static str {
        "kugou"
    }

    /// 根据歌曲元数据搜索歌曲。
    #[instrument(skip(self, track))]
    async fn search_songs(
        &self,
        track: &crate::model::track::Track<'_>,
    ) -> Result<Vec<SearchResult>> {
        let title = track.title.unwrap_or_default();
        if title.is_empty() {
            return Ok(vec![]);
        }
        let keyword = if let Some(artists) = track.artists {
            if let Some(first_artist) = artists.first() {
                format!("{first_artist} - {title}")
            } else {
                title.to_string()
            }
        } else {
            title.to_string()
        };

        let mut business_params = BTreeMap::new();
        business_params.insert("iscorrection".to_string(), "1".to_string());
        business_params.insert("keyword".to_string(), keyword);
        business_params.insert("page".to_string(), "1".to_string());
        business_params.insert("pagesize".to_string(), "30".to_string());
        business_params.insert("platform".to_string(), "AndroidFilter".to_string());
        business_params.insert("albumhide".to_string(), "0".to_string());
        business_params.insert("nocollect".to_string(), "0".to_string());

        let url = format!("{KUGOU_API_GATEWAY}/v3/search/song");
        let resp: models::SearchSongResponse = self
            .execute_signed_get(&url, business_params, Some(X_ROUTER_COMPLEX_SEARCH))
            .await?;

        // 校验API状态码
        if resp.status != 1 || resp.err_code != Some(0) {
            let err_msg = resp.error.unwrap_or_else(|| "未知的 API 错误".to_string());
            return Err(LyricsHelperError::ApiError(format!(
                "酷狗搜索 API 错误: (status: {}, err_code: {:?}, message: {})",
                resp.status, resp.err_code, err_msg
            )));
        }

        if let Some(song_data) = resp.data {
            let results: Vec<SearchResult> = song_data
                .info
                .into_iter()
                .filter_map(|song| {
                    // 歌曲必须有 hash 才能被认为是有效结果，否则过滤掉
                    let song_hash = song.hash?;
                    Some(SearchResult {
                        title: song.song_name.unwrap_or_default(),
                        artists: song
                            .singer_name
                            .unwrap_or_default()
                            .split('、')
                            .map(String::from)
                            .collect(),
                        album: song.album_name,
                        duration: Some(song.duration * 1000),
                        provider_id: song_hash,
                        provider_name: self.name().to_string(),
                        ..Default::default()
                    })
                })
                .collect();
            Ok(results)
        } else {
            // data字段不存在，返回一个空列表
            Ok(vec![])
        }
    }

    /// 根据歌曲的 hash 值获取、解密并解析完整的 KRC 歌词。
    ///
    /// 步骤：
    /// 1. 调用 `/v1/search` 接口搜索歌词元数据，此接口需要安卓签名。
    /// 2. 从返回的候选列表中选取最佳匹配，获取其 `id` 和 `accesskey`。
    /// 3. 调用 `/download` 接口下载 Base64 编码的加密 KRC 歌词。
    /// 4. 解密 KRC 歌词。
    #[instrument(skip(self))]
    async fn get_full_lyrics(&self, song_hash: &str) -> Result<FullLyricsResult> {
        let search_lyrics_url = format!(
            "https://lyrics.kugou.com/search?ver=1&man=yes&client=pc&keyword=&hash={song_hash}"
        );
        let search_resp_text = self
            .http_client
            .get(&search_lyrics_url)
            .send()
            .await?
            .text()
            .await?;

        tracing::trace!(
            url = search_lyrics_url,
            response.body = %search_resp_text,
            "原始 JSON 响应"
        );

        let search_resp: models::SearchLyricsResponse = serde_json::from_str(&search_resp_text)?;

        if search_resp.status != 200 {
            return Err(LyricsHelperError::ApiError(format!(
                "酷狗歌词搜索 API 错误，状态码: {}",
                search_resp.status
            )));
        }

        let best_candidate = search_resp
            .candidates
            .first()
            .ok_or(LyricsHelperError::LyricNotFound)?;

        let download_url = format!(
            "https://lyrics.kugou.com/download?ver=1&client=pc&id={}&accesskey={}&fmt=krc&charset=utf8",
            best_candidate.id, best_candidate.accesskey
        );

        let download_resp_text = self
            .http_client
            .get(&download_url)
            .send()
            .await?
            .text()
            .await?;

        tracing::trace!(
            url = download_url,
            response.body = %download_resp_text,
            "原始 JSON 响应"
        );

        let download_resp: models::LyricDownloadResponse =
            serde_json::from_str(&download_resp_text)?;

        let krc_decrypted = decrypter::decrypt_krc(&download_resp.content)?;

        let conversion_input = ConversionInput {
            main_lyric: InputFile {
                content: krc_decrypted.clone(),
                format: LyricFormat::Krc,
                language: None,
                filename: None,
            },
            translations: Vec::new(),
            romanizations: Vec::new(),
            target_format: LyricFormat::Krc,
            user_metadata_overrides: None,
        };

        let options = ConversionOptions::default();
        let mut parsed_data = converter::parse_and_merge(&conversion_input, &options)?;
        parsed_data.source_name = "kugou".to_string();

        let raw_lyrics = RawLyrics {
            format: "krc".to_string(),
            content: krc_decrypted,
            translation: None,
        };

        Ok(FullLyricsResult {
            parsed: parsed_data,
            raw: raw_lyrics,
        })
    }

    #[instrument(skip(self, album_id))]
    async fn get_album_info(&self, album_id: &str) -> Result<generic::Album> {
        let payload = models::AlbumDetailRequestPayload {
            data: [models::AlbumId { album_id }],
            is_buy: 0,
        };

        let url = format!("{KUGOU_API_GATEWAY}/kmr/v2/albums");
        let resp: models::AlbumDetailResponse = self
            .execute_signed_post(&url, &payload, Some(X_ROUTER_OPENAPI))
            .await?;

        if resp.status != 1 || resp.error_code != 0 {
            return Err(LyricsHelperError::ApiError(format!(
                "酷狗专辑详情 API 错误 (status: {}, error_code: {})",
                resp.status, resp.error_code
            )));
        }

        if let Some(mut data_vec) = resp.data {
            if let Some(data) = data_vec.pop() {
                if data.album_id.is_none() || data.album_name.is_none() {
                    return Err(LyricsHelperError::LyricNotFound);
                }

                let id = data.album_id.unwrap_or_default();
                let name = data.album_name.unwrap_or_default();

                let artists = data
                    .singer_name
                    .unwrap_or_default()
                    .split('、')
                    .map(|name| generic::Artist {
                        id: String::new(),
                        name: name.to_string(),
                    })
                    .collect::<Vec<_>>();

                let cover_url = data.img_url.map(|url| url.replace("{size}", ""));

                Ok(generic::Album {
                    id: id.clone(),
                    name,
                    artists: Some(artists),
                    cover_url,
                    description: data.intro,
                    release_date: data.publish_time,
                    songs: None,
                    provider_id: id,
                })
            } else {
                Err(LyricsHelperError::LyricNotFound)
            }
        } else {
            Err(LyricsHelperError::ApiError(
                "酷狗专辑详情 API 未返回数据".to_string(),
            ))
        }
    }

    async fn get_album_songs(
        &self,
        album_id: &str,
        page: u32,
        page_size: u32,
    ) -> Result<Vec<generic::Song>> {
        let payload = models::AlbumSongsRequestPayload {
            album_id,
            page,
            pagesize: page_size,
            is_buy: "",
        };
        let url = format!("{KUGOU_API_GATEWAY}/v1/album_audio/lite");

        let resp: models::AlbumSongsResponse = self
            .execute_signed_post(&url, &payload, Some(X_ROUTER_OPENAPI))
            .await?;

        if resp.status != 1 || resp.error_code != 0 {
            return Err(LyricsHelperError::ApiError(format!(
                "酷狗专辑歌曲 API 错误 (status: {}, error_code: {})",
                resp.status, resp.error_code
            )));
        }

        if let Some(data) = resp.data {
            let songs = data
                .songs
                .into_iter()
                .filter_map(|song_info| {
                    let song_hash = song_info
                        .audio_info
                        .hash
                        .or(song_info.audio_info.hash_128)?;
                    let artists = song_info
                        .base
                        .singer_name
                        .split('、')
                        .map(|name| generic::Artist {
                            id: String::new(),
                            name: name.to_string(),
                        })
                        .collect();

                    Some(generic::Song {
                        id: song_hash.clone(),
                        name: song_info.base.song_name,
                        artists,
                        duration: song_info
                            .audio_info
                            .duration
                            .map(std::time::Duration::from_millis),
                        album: None,
                        cover_url: None,
                        provider_id: song_hash,
                        album_id: None,
                    })
                })
                .collect();
            Ok(songs)
        } else {
            Ok(vec![])
        }
    }

    async fn get_song_link(&self, song_hash: &str) -> Result<String> {
        let clienttime_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| LyricsHelperError::Internal(format!("时间错误: {e}")))?
            .as_millis();
        let userid = 0;

        let inner_key = signature::sign_key(song_hash, &self.mid, userid, APP_ID, true);

        let payload = models::SongUrlNewRequestPayload {
            area_code: "1",
            behavior: "play",
            qualities: [
                "128",
                "320",
                "flac",
                "high",
                "multitrack",
                "viper_atmos",
                "viper_tape",
                "viper_clear",
            ],
            resource: models::Resource {
                album_audio_id: None,
                collect_list_id: "3",
                collect_time: clienttime_ms,
                hash: song_hash,
                id: 0,
                page_id: 1,
                resource_type: "audio",
            },
            token: "",
            tracker_param: models::TrackerParam {
                all_m: 1,
                auth: "",
                is_free_part: 0,
                key: inner_key,
                module_id: 0,
                need_climax: 1,
                need_xcdn: 1,
                open_time: "",
                pid: "411",
                pidversion: "3001",
                priv_vip_type: "6",
                viptoken: "",
            },
            userid: userid.to_string(),
            vip_type: 0,
        };

        let url = "http://tracker.kugou.com/v6/priv_url";

        let resp: models::SongUrlNewResponse = self
            .execute_signed_post(url, &payload, Some(X_ROUTER_TRACKER))
            .await?;

        for data_item in &resp.data {
            if let Some(goods) = data_item.relate_goods.first()
                && let Some(url) = goods.info.climax_info.url.first()
                && !url.is_empty()
            {
                return Ok(url.clone());
            }
        }

        for data_item in &resp.data {
            if let Some(url) = data_item.info.encrypted_urls.first()
                && !url.is_empty()
            {
                warn!("只找到了加密的 mgg 格式链接，返回此链接。");
                return Ok(url.clone());
            }
        }

        Err(LyricsHelperError::LyricNotFound)
    }

    /// 根据歌手ID获取其单曲列表。
    /// API: /kmr/v1/audio_group/author
    #[instrument(skip(self))]
    async fn get_singer_songs(
        &self,
        singer_id: &str,
        page: u32,
        page_size: u32,
    ) -> Result<Vec<generic::Song>> {
        // 此 API 需要一个特殊的 `key` 字段，该字段依赖 `clienttime`
        let clienttime = get_current_timestamp_sec_str()?;

        let key = signature::sign_params_key(APP_ID, CLIENT_VER, &clienttime);

        let payload = models::KmrSingerSongsRequestPayload {
            appid: APP_ID,
            clientver: CLIENT_VER,
            mid: &self.mid,
            clienttime: &clienttime,
            key,
            author_id: singer_id,
            pagesize: page_size,
            page,
            sort: 1,
            area_code: "all",
        };

        let url = "https://openapi.kugou.com/kmr/v1/audio_group/author";
        let resp: models::KmrSingerSongsResponse = self
            .execute_signed_post(url, &payload, Some(X_ROUTER_OPENAPI))
            .await?;

        if resp.status != 1 || resp.error_code != 0 {
            return Err(LyricsHelperError::ApiError(format!(
                "酷狗歌手歌曲 API 错误 (status: {}, error_code: {})",
                resp.status, resp.error_code
            )));
        }

        let songs = resp
            .data
            .into_iter()
            .map(|song_info| {
                // author_name 是一个字符串，需要分割
                let artists = song_info
                    .author_name
                    .split('、')
                    .map(|name| generic::Artist {
                        id: String::new(),
                        name: name.to_string(),
                    })
                    .collect();

                generic::Song {
                    id: song_info.hash.clone(),
                    name: song_info.audio_name,
                    artists,
                    duration: Some(std::time::Duration::from_secs(song_info.timelength)),
                    album: None,
                    cover_url: None,
                    provider_id: song_info.hash,
                    album_id: None,
                }
            })
            .collect();
        Ok(songs)
    }

    /// 此函数通过调用两个不同的API来组合完整的歌单信息。
    /// 注意：此处的 playlist_id 必须是 global_collection_id (或称 gid)。
    #[instrument(skip(self))]
    async fn get_playlist(&self, playlist_id: &str) -> Result<generic::Playlist> {
        // API 1: 获取歌单元数据 (/v3/get_list_info)
        let meta_resp: models::PlaylistDetailResponse = {
            let payload = models::PlaylistDetailRequestPayload {
                data: vec![models::PlaylistIdObject {
                    global_collection_id: playlist_id,
                }],
                userid: "0",
                token: "",
            };
            let url = format!("{KUGOU_API_GATEWAY}/v3/get_list_info");
            self.execute_signed_post(&url, &payload, Some(X_ROUTER_PUB_SONGS))
                .await?
        };

        let meta_data = meta_resp
            .data
            .into_iter()
            .next()
            .ok_or(LyricsHelperError::LyricNotFound)?;

        // API 2: 获取歌单歌曲列表 (/pubsongs/v2/get_other_list_file_nofilt)
        let songs_resp: models::PlaylistSongsResponse = {
            let page = 1;
            let pagesize = 100;
            let begin_idx = (page - 1) * pagesize;

            let mut business_params = BTreeMap::new();
            business_params.insert("global_collection_id".to_string(), playlist_id.to_string());
            business_params.insert("pagesize".to_string(), pagesize.to_string());
            business_params.insert("begin_idx".to_string(), begin_idx.to_string());
            business_params.insert("area_code".to_string(), "1".to_string());
            business_params.insert("plat".to_string(), "1".to_string());
            business_params.insert("type".to_string(), "1".to_string());
            business_params.insert("mode".to_string(), "1".to_string());

            let url = format!("{KUGOU_API_GATEWAY}/pubsongs/v2/get_other_list_file_nofilt");

            self.execute_signed_get(&url, business_params, None).await?
        };

        let songs = songs_resp.data.map_or(vec![], |d| {
            d.songs
                .into_iter()
                .map(|song_info| {
                    let artists: Vec<generic::Artist> = song_info
                        .singerinfo
                        .into_iter()
                        .map(|a| generic::Artist {
                            id: a.id.to_string(),
                            name: a.name,
                        })
                        .collect();

                    // 从 "歌手 - 歌名" 的格式中分离出真实的歌名
                    let title = strip_artist_from_title(&song_info.name, &song_info.name);

                    generic::Song {
                        id: song_info.hash.clone(),
                        name: title.to_string(),
                        artists,
                        duration: Some(std::time::Duration::from_millis(song_info.timelen)),
                        album: None,
                        cover_url: None,
                        provider_id: song_info.hash,
                        album_id: None,
                    }
                })
                .collect()
        });

        Ok(generic::Playlist {
            id: meta_data.global_collection_id,
            name: meta_data.name,
            cover_url: Some(meta_data.pic.replace("{size}", "150")),
            description: Some(meta_data.intro),
            songs: Some(songs),
            creator_name: Some(meta_data.list_create_username),
        })
    }

    /// 根据歌曲Hash获取歌曲的详细信息。
    /// API: /v2/get_res_privilege/lite
    #[instrument(skip(self))]
    async fn get_song_info(&self, song_id: &str) -> Result<generic::Song> {
        let payload = models::SongDetailRequestPayload {
            appid: APP_ID,
            clientver: CLIENT_VER,
            area_code: "1",
            behavior: "play",
            need_hash_offset: 1,
            relate: 1,
            support_verify: 1,
            resource: vec![models::SongDetailResource {
                resource_type: "audio",
                page_id: 0,
                hash: song_id,
                album_id: 0,
            }],
            qualities: [
                "128",
                "320",
                "flac",
                "high",
                "viper_atmos",
                "viper_tape",
                "viper_clear",
            ],
        };

        let url = format!("{KUGOU_API_GATEWAY}/v2/get_res_privilege/lite");
        let resp: models::SongDetailResponse = self
            .execute_signed_post(&url, &payload, Some(X_ROUTER_MEDIA_STORE))
            .await?;

        if resp.status != 1 || resp.error_code != 0 {
            return Err(LyricsHelperError::ApiError(format!(
                "酷狗歌曲详情 API 错误 (status: {}, error_code: {})",
                resp.status, resp.error_code
            )));
        }

        let song_data = resp
            .data
            .into_iter()
            .next()
            .ok_or(LyricsHelperError::LyricNotFound)?;

        let artists = song_data
            .singername
            .split('、')
            .map(|name| generic::Artist {
                id: String::new(), // API 不返回歌手 ID
                name: name.to_string(),
            })
            .collect();

        // 从 "歌手 - 歌名" 的格式中分离出真实的歌名
        let title = strip_artist_from_title(&song_data.name, &song_data.singername);

        Ok(generic::Song {
            id: song_data.hash.clone(),
            name: title.to_string(),
            artists,
            duration: Some(std::time::Duration::from_millis(song_data.info.duration)),
            album: None,
            cover_url: Some(song_data.info.image.replace("{size}", "")),
            provider_id: song_data.hash,
            album_id: None,
        })
    }

    async fn get_album_cover_url(&self, album_id: &str, size: CoverSize) -> Result<String> {
        let album_info = self.get_album_info(album_id).await?;

        let cover_url_template = album_info
            .cover_url
            .ok_or(LyricsHelperError::LyricNotFound)?;

        if cover_url_template.contains("{size}") {
            let size_str = match size {
                CoverSize::Thumbnail => "150",
                CoverSize::Medium => "240",
                CoverSize::Large => "",
            };
            Ok(cover_url_template.replace("{size}", size_str))
        } else {
            Ok(cover_url_template)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::model::track::Track;
    use tokio::sync::OnceCell;

    use super::*;

    static KUGOU_PROVIDER: OnceCell<KugouMusic> = OnceCell::const_new();

    static TEST_DFID: OnceCell<String> = OnceCell::const_new();

    async fn get_dfid() -> &'static str {
        TEST_DFID
            .get_or_init(|| async {
                let new_instance = KugouMusic::register_via_network()
                    .await
                    .expect("获取 DFID 失败");

                let dfid = new_instance.dfid;

                info!("成功获取 DFID: {}", dfid);
                dfid
            })
            .await
    }

    async fn get_test_provider() -> KugouMusic {
        let dfid = get_dfid().await;
        KugouMusic::from_dfid(dfid.to_string())
    }

    const TEST_SONG_NAME: &str = "这人生所有的美好";
    const TEST_SINGER_NAME: &str = "小蓝背心";

    async fn get_provider() -> &'static KugouMusic {
        KUGOU_PROVIDER
            .get_or_init(|| async {
                KugouMusic::new()
                    .await
                    .expect("初始化 KugouMusic Provider 失败")
            })
            .await
    }

    fn init_tracing() {
        use tracing_subscriber::{EnvFilter, FmtSubscriber};
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,lyrics_helper_rs=trace"));
        let _ = FmtSubscriber::builder()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    }

    #[tokio::test]
    #[ignore]
    async fn test_full_flow_kugou() {
        init_tracing();
        let provider = get_provider().await;

        info!(
            "[INFO] 正在搜索歌曲 '{}({})'",
            TEST_SONG_NAME, TEST_SINGER_NAME
        );
        let search_track = Track {
            title: Some(TEST_SONG_NAME),
            artists: Some(&[TEST_SINGER_NAME]),
            album: None,
        };
        let search_results = provider.search_songs(&search_track).await.unwrap();
        assert!(!search_results.is_empty(), "搜索应返回结果。");

        let best_match = search_results
            .into_iter()
            .find(|s| {
                s.title.contains(TEST_SONG_NAME) && s.artists.join("").contains(TEST_SINGER_NAME)
            })
            .expect("在搜索结果中未能找到目标歌曲。");

        info!("[INFO] 找到目标歌曲，Hash: {}", best_match.provider_id);

        info!("[INFO] 正在使用 hash 获取完整歌词...");
        let lyrics = provider
            .get_full_lyrics(&best_match.provider_id)
            .await
            .unwrap();

        assert!(!lyrics.parsed.lines.is_empty(), "KRC 解析结果不应为空。");
        assert!(
            !lyrics.parsed.lines[0].main_syllables.is_empty(),
            "KRC 歌词应包含音节。"
        );

        info!("✅ test_full_flow_kugou 测试通过！");
    }

    #[tokio::test]
    #[ignore]
    async fn test_integration_get_album_info() {
        init_tracing();
        let provider = get_test_provider().await;

        let album_id = "146986426";

        info!("[INFO] 正在请求专辑 ID: {} 的信息...", album_id);
        let album_info = provider
            .get_album_info(album_id)
            .await
            .expect("获取专辑信息失败");

        assert_eq!(album_info.id, album_id);
        assert!(!album_info.name.is_empty(), "专辑名不应为空");
        assert!(album_info.artists.is_some(), "歌手名不应为空");

        info!("✅ 成功获取并解析了专辑信息");
    }

    #[tokio::test]
    #[ignore]
    async fn test_integration_get_album_info_invalid_id() {
        init_tracing();
        let provider = get_test_provider().await;

        let invalid_album_id = "000000000";

        let result = provider.get_album_info(invalid_album_id).await;

        assert!(result.is_err(), "请求无效ID应该返回一个错误");
        if let Err(e) = result {
            assert!(
                matches!(e, LyricsHelperError::LyricNotFound),
                "错误类型应为 LyricNotFound"
            );
            info!("✅ 成功捕获到 LyricNotFound 错误");
        }
    }
    #[tokio::test]
    #[ignore]
    async fn test_integration_get_album_songs() {
        init_tracing();
        let provider = get_test_provider().await;

        let album_id = "146986426";

        info!("[INFO] 正在请求专辑 ID: {} 的歌曲列表...", album_id);
        let songs = provider
            .get_album_songs(album_id, 1, 30)
            .await
            .expect("获取专辑歌曲失败");

        assert!(!songs.is_empty(), "返回的歌曲列表不应为空");

        info!("✅ 成功获取了 {} 首歌曲", songs.len());
    }

    #[tokio::test]
    #[ignore]
    async fn test_integration_get_song_link() {
        init_tracing();
        let provider = get_test_provider().await;

        let song_hash = "69D45D31ADB5B9D58A70E4B7F9A4AA0B";

        info!("[INFO] 正在请求 HASH: {} 的播放链接...", song_hash);
        let song_link_result = provider.get_song_link(song_hash).await;

        assert!(
            song_link_result.is_ok(),
            "获取播放链接应该成功，返回的却是错误: {:?}",
            song_link_result.err()
        );

        let song_link = song_link_result.unwrap();
        info!("✅ 成功获取到播放链接: {}", &song_link);

        assert!(!song_link.is_empty(), "播放链接不应为空字符串");
        assert!(
            song_link.starts_with("http://") || song_link.starts_with("https://"),
            "链接应以 http:// 或 https:// 开头"
        );
        assert!(
            song_link.contains(".mp3") || song_link.contains("?"),
            "链接格式缺少 .mp3 或查询参数"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_integration_get_singer_songs() {
        init_tracing();
        let provider = get_test_provider().await;

        let singer_id = "5579497";

        info!("[INFO] 正在请求歌手 ID: {} 的歌曲列表...", singer_id);
        let songs_result = provider.get_singer_songs(singer_id, 1, 5).await;

        let songs = songs_result.expect("获取歌手歌曲失败");

        assert!(!songs.is_empty(), "返回的歌手歌曲列表不应为空");

        info!("✅ 成功获取了 {} 首歌曲", songs.len());
        if let Some(first_song) = songs.first() {
            let artists_str = first_song
                .artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            info!("第一首歌: {} - {}", artists_str, first_song.name);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_integration_get_playlist() {
        init_tracing();
        let provider = get_test_provider().await;

        let playlist_id = "collection_3_2132040296_8_0";

        info!("[INFO] 正在请求歌单 ID: {} 的完整信息...", playlist_id);
        let playlist = provider
            .get_playlist(playlist_id)
            .await
            .expect("获取歌单完整信息失败");

        assert_eq!(playlist.id, playlist_id);
        assert!(!playlist.name.is_empty(), "歌单名不应为空");

        let songs = playlist.songs.expect("歌单歌曲列表不应为 None");
        assert!(!songs.is_empty(), "歌单歌曲列表不应为空");

        info!(
            "✅ 成功获取了歌单 '{}'，包含 {} 首歌曲。",
            playlist.name,
            songs.len()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_integration_get_song_info() {
        init_tracing();
        let provider = get_test_provider().await;

        let song_hash = "DBE68B72F69025954B1E2EC0D06D7C9E";

        info!("[INFO] 正在请求歌曲 HASH: {} 的详情...", song_hash);
        let song_info_result = provider.get_song_info(song_hash).await;

        let song_info = song_info_result.expect("获取歌曲详情失败");

        assert_eq!(song_info.id, song_hash);
        assert!(song_info.name.contains("人鱼"), "歌曲名不匹配");

        info!("✅ 成功获取并解析了歌曲详情");
    }

    #[tokio::test]
    #[ignore]
    async fn test_integration_get_album_cover_url() {
        init_tracing();
        let provider = get_test_provider().await;
        let album_id = "146986426";

        let large_cover_url = provider
            .get_album_cover_url(album_id, CoverSize::Large)
            .await
            .expect("获取封面应该成功");

        assert!(!large_cover_url.is_empty(), "Large 封面 URL 不应为空");
        assert!(large_cover_url.starts_with("http"), "URL 应该以 http 开头");

        assert!(
            !large_cover_url.contains("{size}"),
            "URL 中的 size 占位符应被替换"
        );

        info!("✅ 成功获取封面: {}", large_cover_url);
    }

    #[tokio::test]
    #[ignore]
    async fn test_integration_get_images_batch() {
        init_tracing();
        let provider = get_test_provider().await;
        let items_to_fetch = [
            models::BatchImageDataItem {
                hash: "DBE68B72F69025954B1E2EC0D06D7C9E",
                album_id: 0,
                album_audio_id: 0,
            },
            models::BatchImageDataItem {
                hash: "69D45D31ADB5B9D58A70E4B7F9A4AA0B",
                album_id: 146986426,
                album_audio_id: 0,
            },
        ];

        let response = provider
            .get_images_batch(&items_to_fetch)
            .await
            .expect("批量获取图片失败");

        assert_eq!(response.status, 1, "API 状态码不为 1");
        assert_eq!(
            response.data.len(),
            items_to_fetch.len(),
            "返回的数据项数量与请求不匹配"
        );

        let first_item = response.data.first().unwrap();
        let first_album_info = first_item.album.first().unwrap();
        assert_eq!(first_album_info.album_name, "人鱼");
        assert!(first_album_info.sizable_cover.contains("{size}"));

        info!("✅ 批量获取图片成功");
    }
}
