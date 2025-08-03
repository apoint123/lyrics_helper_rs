//! 匹配算法模块。参考隔壁的 Lyricify Lyrics Helper

use crate::converter::processors::chinese_conversion_processor::convert;
use crate::model::match_type::MatchScorable;
use crate::model::match_type::{ArtistMatchType, DurationMatchType, NameMatchType};
use crate::model::track::{MatchType, SearchResult, Track};
use ferrous_opencc::config::BuiltinConfig;
use std::collections::HashSet;

pub(crate) fn compare_track(track: &Track, result: &SearchResult) -> MatchType {
    let title_match = compare_name(track.title, Some(&result.title));
    let artist_match = compare_artists(track.artists, Some(result.artists.as_slice()));
    let album_match = compare_name(track.album, result.album.as_deref());
    let duration_match = compare_duration(track.duration, result.duration);

    let mut total_score = 0.0;
    total_score += title_match.get_score() as f64;
    total_score += artist_match.get_score() as f64;
    total_score += album_match.get_score() as f64 * 0.4;
    total_score += duration_match.get_score() as f64 * 1.0;

    const MAX_SINGLE_SCORE: f64 = 7.0;
    let mut possible_score = MAX_SINGLE_SCORE * 2.0;
    if album_match.is_some() {
        possible_score += MAX_SINGLE_SCORE * 0.4;
    }
    if duration_match.is_some() {
        possible_score += MAX_SINGLE_SCORE * 1.0;
    }

    let full_score_base = MAX_SINGLE_SCORE * (1.0 + 1.0 + 0.4 + 1.0);

    let mut normalized_score = total_score;
    if possible_score > 0.0 && possible_score < full_score_base {
        normalized_score = total_score * (full_score_base / possible_score);
    }

    match normalized_score {
        s if s > 21.0 => MatchType::Perfect,
        s if s > 19.0 => MatchType::VeryHigh,
        s if s > 17.0 => MatchType::High,
        s if s > 15.0 => MatchType::PrettyHigh,
        s if s > 11.0 => MatchType::Medium,
        s if s > 8.0 => MatchType::Low,
        s if s > 3.0 => MatchType::VeryLow,
        _ => MatchType::None,
    }
}

fn normalize_name_for_comparison(name: &str) -> String {
    name.replace('’', "'")
        .replace('，', ",")
        .replace(['（', '【', '['], " (")
        .replace(['）', '】', ']'], ") ")
        .replace("  ", " ")
        .replace("acoustic version", "acoustic")
        .trim()
        .to_string()
}

fn compute_text_same(text1: &str, text2: &str) -> f64 {
    strsim::normalized_levenshtein(text1, text2) * 100.0
}

fn compare_name(name1_opt: Option<&str>, name2_opt: Option<&str>) -> Option<NameMatchType> {
    let name1_raw = name1_opt?;
    let name2_raw = name2_opt?;

    let name1_sc_lower = convert(name1_raw, BuiltinConfig::T2s).to_lowercase();
    let name2_sc_lower = convert(name2_raw, BuiltinConfig::T2s).to_lowercase();

    if name1_sc_lower.trim() == name2_sc_lower.trim() {
        return Some(NameMatchType::Perfect);
    }

    let name1 = normalize_name_for_comparison(&name1_sc_lower);
    let name2 = normalize_name_for_comparison(&name2_sc_lower);

    let n1_alt = name1.replace(" - ", " (").trim().to_string() + ")";
    let n2_alt = name2.replace(" - ", " (").trim().to_string() + ")";
    if n1_alt.replace(' ', "") == n2_alt.replace(' ', "") {
        return Some(NameMatchType::VeryHigh);
    }

    let special_suffixes = [
        "deluxe",
        "explicit",
        "special edition",
        "bonus track",
        "feat",
        "with",
    ];
    for suffix in special_suffixes {
        let suffixed_form = format!("({}", suffix);
        if (name1.contains(&suffixed_form)
            && !name2.contains(&suffixed_form)
            && name2 == name1.split(&suffixed_form).next().unwrap_or("").trim())
            || (name2.contains(&suffixed_form)
                && !name1.contains(&suffixed_form)
                && name1 == name2.split(&suffixed_form).next().unwrap_or("").trim())
        {
            return Some(NameMatchType::VeryHigh);
        }
    }

    if let (Some(n1_base), Some(n2_base)) = (name1.split('(').next(), name2.split('(').next())
        && n1_base.trim() == n2_base.trim()
    {
        return Some(NameMatchType::High);
    }

    if (name1.contains('(')
        && !name2.contains('(')
        && name2 == name1.split('(').next().unwrap_or("").trim())
        || (name2.contains('(')
            && !name1.contains('(')
            && name1 == name2.split('(').next().unwrap_or("").trim())
    {
        return Some(NameMatchType::Medium);
    }

    if name1.chars().count() == name2.chars().count() {
        let count = name1
            .chars()
            .zip(name2.chars())
            .filter(|(c1, c2)| c1 == c2)
            .count();
        let len = name1.chars().count();
        let ratio = count as f64 / len as f64;
        if (ratio >= 0.8 && len >= 4) || (ratio >= 0.5 && (2..=3).contains(&len)) {
            return Some(NameMatchType::High);
        }
    }

    if compute_text_same(&name1, &name2) > 90.0 {
        return Some(NameMatchType::VeryHigh);
    }
    if compute_text_same(&name1, &name2) > 80.0 {
        return Some(NameMatchType::High);
    }
    if compute_text_same(&name1, &name2) > 68.0 {
        return Some(NameMatchType::Medium);
    }
    if compute_text_same(&name1, &name2) > 55.0 {
        return Some(NameMatchType::Low);
    }

    Some(NameMatchType::NoMatch)
}

fn compare_artists<S1, S2>(
    artists1: Option<&[S1]>,
    artists2: Option<&[S2]>,
) -> Option<ArtistMatchType>
where
    S1: AsRef<str>,
    S2: AsRef<str>,
{
    let list1_raw = artists1?;
    let list2_raw = artists2?;

    let list1: Vec<String> = list1_raw
        .iter()
        .map(|s| convert(s.as_ref(), BuiltinConfig::T2s).to_lowercase())
        .collect();
    let list2: Vec<String> = list2_raw
        .iter()
        .map(|s| convert(s.as_ref(), BuiltinConfig::T2s).to_lowercase())
        .collect();

    let set1: HashSet<&str> = list1.iter().map(|s| s.as_str()).collect();

    let count = list2.iter().filter(|s| set1.contains(s.as_str())).count();

    if count == list1.len() && list1.len() == list2.len() {
        return Some(ArtistMatchType::Perfect);
    }
    if (list1.len() >= 2 && count + 1 >= list1.len())
        || (list1.len() > 6 && (count as f64 / list1.len() as f64) > 0.8)
    {
        return Some(ArtistMatchType::VeryHigh);
    }
    if count == 1 && list1.len() == 1 && list2.len() == 2 {
        return Some(ArtistMatchType::High);
    }
    if list1.len() > 5
        && !list2.is_empty()
        && (list2[0].contains("various") || list2[0].contains("群星"))
    {
        return Some(ArtistMatchType::VeryHigh);
    }
    if list1.len() > 7 && list2.len() > 7 && (count as f64 / list1.len() as f64) > 0.66 {
        return Some(ArtistMatchType::High);
    }
    if list1.len() == 1 && list2.len() > 1 && !list2.is_empty() {
        if list1[0].starts_with(&list2[0]) {
            return Some(ArtistMatchType::High);
        }
        if list2[0].len() > 3 && list1[0].contains(&list2[0]) {
            return Some(ArtistMatchType::High);
        }
        if list2[0].len() > 1 && list1[0].contains(&list2[0]) {
            return Some(ArtistMatchType::Medium);
        }
    }
    if count == 1 && list1.len() == 1 && list2.len() >= 3 {
        return Some(ArtistMatchType::Medium);
    }
    if count >= 2 {
        return Some(ArtistMatchType::Low);
    }
    Some(ArtistMatchType::NoMatch)
}

fn compare_duration(duration1: Option<u64>, duration2: Option<u64>) -> Option<DurationMatchType> {
    let d1 = duration1.filter(|&d| d > 0)?;
    let d2 = duration2.filter(|&d| d > 0)?;
    let diff = (d1 as i64 - d2 as i64).unsigned_abs();
    Some(match diff {
        0 => DurationMatchType::Perfect,
        1..=299 => DurationMatchType::VeryHigh,
        300..=699 => DurationMatchType::High,
        700..=1499 => DurationMatchType::Medium,
        1500..=3499 => DurationMatchType::Low,
        _ => DurationMatchType::NoMatch,
    })
}
