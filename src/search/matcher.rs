//! 匹配算法模块，简单实现

use crate::model::track::{MatchType, SearchResult, Track};
use strsim::normalized_levenshtein;

/// 比较音轨元数据和搜索结果，返回匹配类型
pub fn compare_track(track: &Track, result: &SearchResult) -> MatchType {
    // 字符串相似度阈值，高于此值则认为匹配
    const SIMILARITY_THRESHOLD: f64 = 0.85;

    // 检查标题是否匹配
    let title_match = match track.title {
        Some(t) => normalized_levenshtein(t, &result.title) >= SIMILARITY_THRESHOLD,
        None => false,
    };

    // 如果标题不匹配，直接返回 None
    if !title_match {
        return MatchType::None;
    }

    // 检查艺术家是否匹配
    let artist_match = match track.artists {
        Some(artists) => artists.iter().any(|&artist| {
            result
                .artists
                .iter()
                .any(|r_artist| r_artist.contains(artist))
        }),
        None => true, // 如果没有提供艺术家，则认为匹配
    };

    // 检查专辑是否匹配
    let album_match = match (track.album, &result.album) {
        (Some(t_album), Some(r_album)) => {
            normalized_levenshtein(t_album, r_album) >= SIMILARITY_THRESHOLD
        }
        (None, _) => true, // 如果没有提供专辑，则认为匹配
        _ => false,
    };

    // 根据匹配情况返回最终的 MatchType
    match (artist_match, album_match) {
        (true, true) => MatchType::Full,
        (true, false) => MatchType::TitleAndArtist,
        (false, true) => MatchType::TitleAndAlbum,
        (false, false) => MatchType::Title,
    }
}
