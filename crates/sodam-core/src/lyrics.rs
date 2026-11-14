//! 歌词：优先读音频文件的内嵌歌词，其次读同名 `.lrc` 侧车文件。
//!
//! 支持 LRC 时间轴（`[mm:ss.xx]`、一行多个时间戳、翻译行）与纯文本歌词。
//! 解析只做文本处理，不依赖 UI；同步显示、高亮与点击跳转由应用层负责。

use std::path::Path;

use lofty::file::TaggedFileExt;
use lofty::tag::ItemKey;

/// 一行歌词。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricLine {
    /// 毫秒时间戳；纯文本歌词为 `None`。
    pub at_ms: Option<u64>,
    /// 原文。
    pub text: String,
    /// 翻译（同一时间戳的第二行）。
    pub translation: Option<String>,
}

/// 一份歌词。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lyrics {
    /// 按时间排序的行；纯文本时保持文件顺序。
    pub lines: Vec<LyricLine>,
    /// 是否有可用的时间轴。
    pub synced: bool,
}

impl Lyrics {
    /// 当前播放位置对应的歌词行下标。
    ///
    /// 二分找「最后一个时间戳 <= position」的行；早于第一行时返回 `None`。
    pub fn active_index(&self, position_ms: u64) -> Option<usize> {
        if !self.synced || self.lines.is_empty() {
            return None;
        }
        let mut low = 0usize;
        let mut high = self.lines.len();
        while low < high {
            let mid = (low + high) / 2;
            match self.lines[mid].at_ms {
                Some(at) if at <= position_ms => low = mid + 1,
                _ => high = mid,
            }
        }
        if low > 0 {
            Some(low - 1)
        } else {
            None
        }
    }
}

/// 读取歌词：内嵌优先，其次同名 `.lrc`。
pub fn load(path: &Path) -> Option<Lyrics> {
    if let Some(text) = embedded_lyrics(path) {
        if let Some(lyrics) = parse(&text) {
            return Some(lyrics);
        }
    }
    let sidecar = path.with_extension("lrc");
    let text = std::fs::read_to_string(sidecar).ok()?;
    parse(&text)
}

fn embedded_lyrics(path: &Path) -> Option<String> {
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    // ID3v2 的 USLT 在 lofty 里映射到 UnsyncLyrics；Vorbis/MP4 的 LYRICS 映射到 Lyrics。
    tag.get_string(ItemKey::Lyrics)
        .or_else(|| tag.get_string(ItemKey::UnsyncLyrics))
        .map(str::to_owned)
}

/// 解析 LRC 或纯文本歌词。
///
/// 返回 `None` 表示没有任何可用内容（空文件、只有元信息）。
pub fn parse(text: &str) -> Option<Lyrics> {
    let text = text.trim_start_matches('\u{feff}');
    let mut offset_ms: i64 = 0;
    let mut timed: Vec<LyricLine> = Vec::new();
    let mut plain: Vec<String> = Vec::new();

    for raw in text.lines() {
        let raw = raw.trim_end_matches('\r').trim();
        if raw.is_empty() {
            // 纯文本歌词保留空行作为段落间隔；LRC 的空行没有意义。
            continue;
        }
        let (timestamps, body, meta) = split_timestamps(raw);
        if let Some(offset) = meta_offset(raw) {
            offset_ms = offset;
        }
        if timestamps.is_empty() {
            if meta || is_credit_line(body) {
                continue;
            }
            // 没有任何时间戳的行先当作纯文本候选；如果整份文件有 LRC 行，它们会被忽略。
            plain.push(body.to_string());
            continue;
        }
        let body = body.trim();
        if body.is_empty() || is_credit_line(body) {
            continue;
        }
        // 显式的「原文 // 译文」优先；没有就整行当原文，稍后再按时间戳/行内规则拆分。
        let (text, translation) = match split_explicit_translation(body) {
            Some((text, translation)) => (text, Some(translation)),
            None => (body.to_string(), None),
        };
        for timestamp in timestamps {
            timed.push(LyricLine {
                at_ms: Some(timestamp),
                text: text.clone(),
                translation: translation.clone(),
            });
        }
    }

    if timed.is_empty() {
        let lines: Vec<LyricLine> = plain
            .into_iter()
            .filter(|line| !line.is_empty())
            .map(|text| {
                let mut line = LyricLine {
                    at_ms: None,
                    text,
                    translation: None,
                };
                split_inline_translation(&mut line);
                line
            })
            .collect();
        if lines.is_empty() {
            return None;
        }
        return Some(Lyrics {
            lines,
            synced: false,
        });
    }

    if offset_ms != 0 {
        for line in &mut timed {
            if let Some(at) = line.at_ms {
                line.at_ms = Some(if offset_ms >= 0 {
                    at.saturating_add(offset_ms as u64)
                } else {
                    at.saturating_sub(offset_ms.unsigned_abs())
                });
            }
        }
    }

    timed.sort_by_key(|line| line.at_ms);

    // 同一时间戳的第二行视为翻译。有的 LRC 一行原文、一行翻译，时间戳完全相同；
    // 也有的把翻译放在原文后面、用 `//` 分隔（前面已处理）。
    let mut merged: Vec<LyricLine> = Vec::new();
    for line in timed {
        let same_time = merged
            .last()
            .is_some_and(|last| last.at_ms == line.at_ms && last.at_ms.is_some());
        if same_time && merged.last().is_some_and(|last| last.text != line.text) {
            let last = merged.last_mut().expect("刚刚判过非空");
            // 两边都已经各自带译文时不再合并，避免把第三种文本塞进同一行。
            if last.translation.is_none() && line.translation.is_none() {
                last.translation = Some(line.text);
                continue;
            }
        }
        merged.push(line);
    }

    // 第三层：时间戳非常接近、但语言不同的相邻两行，视为原文 + 译文。
    // 只靠时间差会把快速连唱的两句英文并成一句，所以这里必须语言类型也相反。
    let mut merged = merge_near_translations(merged);

    // 最后才走行内拆分：很多双语 LRC 把「英文原文 中文翻译」写在同一行，用空格分隔。
    // 这里按第一个 CJK 字符拆成两行：原文留在正文，翻译进入 translation。
    for line in &mut merged {
        split_inline_translation(line);
    }

    Some(Lyrics {
        lines: merged,
        synced: true,
    })
}

/// 显式的「原文 // 译文」写法。
fn split_explicit_translation(text: &str) -> Option<(String, String)> {
    let (main, translation) = text.split_once("//")?;
    let main = main.trim();
    let translation = translation.trim();
    if main.is_empty() || translation.is_empty() {
        return None;
    }
    if latin_count(main) < 3 || cjk_count(translation) < 2 {
        return None;
    }
    Some((main.to_string(), translation.to_string()))
}

/// 时间戳相差不超过这个值时，才考虑把相邻两行当成一对原文/译文。
const NEAR_TRANSLATION_MS: u64 = 300;

/// 把「英文一行、中文紧接着一行」的双语 LRC 合并成原文 + 译文。
///
/// 只处理语言明显相反的一对；同一语言的近时间行（快速连唱）保持独立。
fn merge_near_translations(lines: Vec<LyricLine>) -> Vec<LyricLine> {
    let mut out = Vec::with_capacity(lines.len());
    let mut iter = lines.into_iter().peekable();
    while let Some(mut line) = iter.next() {
        let pairs = line.translation.is_none()
            && iter.peek().is_some_and(|next| {
                next.translation.is_none()
                    && match (line.at_ms, next.at_ms) {
                        (Some(left), Some(right)) => {
                            right >= left
                                && right - left <= NEAR_TRANSLATION_MS
                                && languages_differ(&line.text, &next.text)
                        }
                        _ => false,
                    }
            });
        if pairs {
            line.translation = iter.next().map(|next| next.text);
        }
        out.push(line);
    }
    out
}

/// 两边是否明显是不同语言（一侧以拉丁字母为主，另一侧以 CJK 为主）。
fn languages_differ(left: &str, right: &str) -> bool {
    let (left_latin, left_cjk) = (latin_count(left), cjk_count(left));
    let (right_latin, right_cjk) = (latin_count(right), cjk_count(right));
    let is_latin = |latin: usize, cjk: usize| latin >= 3 && latin > cjk * 2;
    let is_cjk = |latin: usize, cjk: usize| cjk >= 2 && cjk > latin * 2;
    (is_latin(left_latin, left_cjk) && is_cjk(right_latin, right_cjk))
        || (is_cjk(left_latin, left_cjk) && is_latin(right_latin, right_cjk))
}

fn latin_count(text: &str) -> usize {
    text.chars().filter(char::is_ascii_alphabetic).count()
}

fn cjk_count(text: &str) -> usize {
    text.chars().filter(|ch| is_cjk(*ch)).count()
}

/// 作词/作曲/制作这类元信息：不是歌词，不参与高亮、翻译和行内拆分。
fn is_credit_line(text: &str) -> bool {
    const LABELS: &[&str] = &[
        "作词",
        "作曲",
        "编曲",
        "制作",
        "监制",
        "混音",
        "母带",
        "录音",
        "和声",
        "出品",
        "发行",
        "版权",
        "著作权",
        "lyricist",
        "composer",
        "producer",
        "arranger",
        "mixing",
        "mastering",
        "copyright",
    ];
    if let Some((label, _)) = text.split_once([':', '：']) {
        let label = label.trim().to_lowercase();
        let compact: String = label.chars().filter(|ch| !ch.is_whitespace()).collect();
        // OP/SP 是唱片公司的版权行，不是歌词；两种冒号、空格写法都认。
        if matches!(compact.as_str(), "op" | "sp" | "op/sp" | "op&sp") {
            return true;
        }
        if LABELS.iter().any(|key| label.contains(key)) {
            return true;
        }
    }
    let lower = text.trim().to_lowercase();
    if lower.starts_with("copyright")
        || lower.starts_with("all rights reserved")
        || lower.starts_with('©')
        || lower.starts_with('℗')
    {
        return true;
    }
    // 常见的「未经授权不得翻唱/使用」版权声明，通常整行在括号里。
    let restricted = lower.contains("未经授权")
        || lower.contains("未经许可")
        || lower.contains("未经著作")
        || lower.contains("版权所有");
    let usage = lower.contains("翻唱")
        || lower.contains("翻录")
        || lower.contains("使用")
        || lower.contains("转载")
        || lower.contains("复制");
    if restricted && usage {
        return true;
    }
    lower.starts_with("written by ")
        || lower.starts_with("lyrics by ")
        || lower.starts_with("music by ")
}

/// 把「English lyric 中文翻译」这种同一行的双语歌词拆开。
///
/// 只在英文部分足够长（至少 3 个字母）且中文部分至少 2 个汉字时拆分，
/// 避免把 `DJ制作：DJ东东`、`词 Lyricist : ...` 这类制作信息拆坏。
fn split_inline_translation(line: &mut LyricLine) {
    if line.translation.is_some() {
        return;
    }
    let mut latin = 0usize;
    let mut split_at = None;
    for (index, ch) in line.text.char_indices() {
        if is_cjk(ch) {
            split_at = Some(index);
            break;
        }
        if ch.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    let Some(index) = split_at else {
        return;
    };
    if latin < 3 {
        return;
    }
    let (main, translation) = line.text.split_at(index);
    let main = main
        .trim_end_matches(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    ',' | '.'
                        | '-'
                        | '–'
                        | '/'
                        | '|'
                        | '·'
                        | ':'
                        | '：'
                        | '('
                        | '（'
                        | '['
                        | '【'
                        | '《'
                )
        })
        .trim()
        .to_string();
    // 英文原文后常见「(中文翻译)」写法：括号不属于译文内容。
    let translation = translation
        .trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, ')' | '）' | ']' | '】' | '》'))
        .to_string();
    if main.is_empty() || translation.chars().filter(|ch| is_cjk(*ch)).count() < 2 {
        return;
    }
    line.text = main;
    line.translation = Some(translation);
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3040..=0x30FF | 0x3400..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF
    )
}

/// 拆出前缀时间戳与正文。`[ti:xxx]` 这类元信息单独标记。
fn split_timestamps(line: &str) -> (Vec<u64>, &str, bool) {
    let mut timestamps = Vec::new();
    let mut rest = line;
    let mut meta = false;
    while let Some(stripped) = rest.strip_prefix('[') {
        let Some(end) = stripped.find(']') else {
            break;
        };
        let tag = &stripped[..end];
        if let Some(ms) = parse_timestamp(tag) {
            timestamps.push(ms);
        } else {
            // 不是时间戳的 `[...]` 是元信息（ti/ar/al/by/offset），整行跳过。
            meta = true;
            return (timestamps, rest, meta);
        }
        rest = &stripped[end + 1..];
    }
    (timestamps, rest, meta)
}

fn parse_timestamp(tag: &str) -> Option<u64> {
    let tag = tag.trim();
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;
    // 秒可以带小数（两位或三位），也允许 `12` 这种整秒。
    let seconds: f64 = seconds.trim().parse().ok()?;
    if !seconds.is_finite() || !(0.0..60.0).contains(&seconds) {
        return None;
    }
    Some(minutes.saturating_mul(60_000) + (seconds * 1000.0).round() as u64)
}

fn meta_offset(line: &str) -> Option<i64> {
    let inner = line.strip_prefix("[offset:")?;
    let value = inner.strip_suffix(']')?;
    value.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_timestamps_and_translations() {
        let lyrics = parse(
            "[ti:测试]\n\
             [offset:0]\n\
             [00:01.00]第一句\n\
             [00:01.00]First line\n\
             [00:03.50]第二句\n\
             [00:05.25][00:07.25]重复句\n",
        )
        .expect("歌词");
        assert!(lyrics.synced);
        assert_eq!(lyrics.lines.len(), 4);
        assert_eq!(lyrics.lines[0].at_ms, Some(1000));
        assert_eq!(lyrics.lines[0].text, "第一句");
        assert_eq!(lyrics.lines[0].translation.as_deref(), Some("First line"));
        assert_eq!(lyrics.lines[1].at_ms, Some(3500));
        assert_eq!(lyrics.lines[2].at_ms, Some(5250));
        assert_eq!(lyrics.lines[3].at_ms, Some(7250));
        assert_eq!(lyrics.lines[2].text, "重复句");
        assert_eq!(lyrics.active_index(0), None);
        assert_eq!(lyrics.active_index(999), None);
        assert_eq!(lyrics.active_index(1000), Some(0));
        assert_eq!(lyrics.active_index(3000), Some(0));
        assert_eq!(lyrics.active_index(3500), Some(1));
        assert_eq!(lyrics.active_index(99_999), Some(3));
    }

    #[test]
    fn splits_inline_english_and_chinese_translation() {
        let lyrics = parse(
            "[00:03.00]What's the trick? I wish I knew 爱的诀窍是什么 真希望我能懂\n\
             [00:04.00]You crossed the line and it's time to say  F U 你越过了界 所以是时候对你说不\n",
        )
        .expect("歌词");
        assert_eq!(lyrics.lines[0].text, "What's the trick? I wish I knew");
        assert_eq!(
            lyrics.lines[0].translation.as_deref(),
            Some("爱的诀窍是什么 真希望我能懂")
        );
        // 英文末尾的感叹词保留在正文，翻译从第一个汉字开始。
        assert_eq!(
            lyrics.lines[1].text,
            "You crossed the line and it's time to say  F U"
        );
        assert_eq!(
            lyrics.lines[1].translation.as_deref(),
            Some("你越过了界 所以是时候对你说不")
        );
    }

    #[test]
    fn filters_credits_and_keeps_chinese_first_lines() {
        let lyrics = parse(
            "[00:00.00]作词 : Pablo Bowman/Daniel Boyle\n\
             [00:00.10]DJ制作：DJ东东\n\
             [00:00.20]你好 hello\n",
        )
        .expect("歌词");
        assert_eq!(lyrics.lines.len(), 1, "作词/制作信息不是歌词");
        assert_eq!(lyrics.lines[0].text, "你好 hello");
        assert_eq!(lyrics.lines[0].translation, None);
    }

    #[test]
    fn filters_op_sp_and_copyright_notice() {
        let lyrics = parse(
            "[00:00.00]OP/SP：青风音乐 Cheerful Music\n\
             [00:00.00]（未经授权不得翻唱或使用）\n\
             [00:01.00]第一句\n",
        )
        .expect("歌词");
        assert_eq!(lyrics.lines.len(), 1, "OP/SP 与版权声明不是歌词");
        assert_eq!(lyrics.lines[0].text, "第一句");
        assert_eq!(lyrics.lines[0].translation, None);
    }

    #[test]
    fn parenthesized_translation_loses_its_brackets() {
        let lyrics = parse("[00:01.00]I love you（我爱你）\n").expect("歌词");
        assert_eq!(lyrics.lines[0].text, "I love you");
        assert_eq!(lyrics.lines[0].translation.as_deref(), Some("我爱你"));
    }

    #[test]
    fn merges_near_timestamp_translation_only_across_languages() {
        let lyrics = parse(
            "[00:01.00]Walking beneath the stars\n\
             [00:01.18]漫步在星空之下\n\
             [00:02.00]The night feels so alive\n\
             [00:02.10]And dreams are all around\n",
        )
        .expect("歌词");
        assert_eq!(lyrics.lines.len(), 3);
        assert_eq!(lyrics.lines[0].text, "Walking beneath the stars");
        assert_eq!(
            lyrics.lines[0].translation.as_deref(),
            Some("漫步在星空之下")
        );
        // 同语言的近时间行是快速连唱，不能当翻译吞掉。
        assert_eq!(lyrics.lines[1].text, "The night feels so alive");
        assert_eq!(lyrics.lines[1].translation, None);
        assert_eq!(lyrics.lines[2].text, "And dreams are all around");
        assert_eq!(lyrics.lines[2].translation, None);
    }

    #[test]
    fn explicit_slash_translation_takes_priority() {
        let lyrics = parse("[00:01.00]Hold on // 坚持下去\n").expect("歌词");
        assert_eq!(lyrics.lines[0].text, "Hold on");
        assert_eq!(lyrics.lines[0].translation.as_deref(), Some("坚持下去"));
    }

    #[test]
    fn splits_inline_translation_in_plain_lyrics_too() {
        let lyrics = parse("I'll be fine 我也会安然无恙\n").expect("歌词");
        assert_eq!(lyrics.lines[0].text, "I'll be fine");
        assert_eq!(
            lyrics.lines[0].translation.as_deref(),
            Some("我也会安然无恙")
        );
    }

    #[test]
    fn parses_plain_lyrics_without_timestamps() {
        let lyrics = parse("第一行\n\n第二行\n").expect("歌词");
        assert!(!lyrics.synced);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "第一行");
        assert_eq!(lyrics.lines[1].text, "第二行");
        assert_eq!(lyrics.active_index(10_000), None);
    }

    #[test]
    fn offset_shifts_timestamps() {
        let lyrics = parse("[offset:500]\n[00:01.00]延迟\n").expect("歌词");
        assert_eq!(lyrics.lines[0].at_ms, Some(1500));
    }

    #[test]
    fn negative_offset_subtracts_safely() {
        let lyrics = parse("[offset:-200]\n[00:00.10]提前\n").expect("歌词");
        assert_eq!(lyrics.lines[0].at_ms, Some(0));
    }

    #[test]
    fn empty_or_metadata_only_returns_none() {
        assert!(parse("").is_none());
        assert!(parse("[ti:标题]\n[ar:歌手]\n").is_none());
    }

    #[test]
    fn sidecar_lrc_is_used_when_no_embedded_lyrics() {
        let dir = std::env::temp_dir().join(format!("sodam-lyrics-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录");
        let lrc = dir.join("song.lrc");
        let mut file = std::fs::File::create(&lrc).expect("写 lrc");
        writeln!(file, "[00:02.00]侧车歌词").expect("写内容");
        drop(file);
        let parsed = load(&dir.join("song.flac")).expect("应回退到侧车");
        assert_eq!(parsed.lines[0].text, "侧车歌词");
        assert_eq!(parsed.lines[0].at_ms, Some(2000));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
