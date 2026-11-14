//! 本地音乐库：**文件夹即歌单**，不需要登录、不联网。
//!
//! 扫描是同步的（core 不依赖 UI，也不该替调用方决定线程模型），调用方把它放到后台线程，
//! 扫完再把结果丢回界面。这里对每个文件、每个子目录的错误都是「跳过它自己」：
//! 用户随手选了一个包含 `System Volume Information` 或断链快捷方式的目录，整页数据
//! 不该因此变成空的。

use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;

use crate::log_outlet::{log_event, LogLevel, LogTopic};
use crate::models::TrackItem;

/// 本地曲目的 track id 前缀，后接绝对路径字符串。
///
/// 用前缀而不是另开一张 id 表：路径本身就是稳定主键，重扫之后同一首歌的 id 不变，
/// 队列里的曲目也不会因为重新扫描而对不上。
pub const LOCAL_ID_PREFIX: &str = "local:";

/// 参与扫描的扩展名（比较时统一小写，所以这里只写小写形态）。
///
/// 能不能真的解码由 rodio 的 feature 决定；这里只决定「这个文件算不算一首歌」。
/// 列进来的扩展名即使解不了码（Symphonia 还不支持 Opus），也要出现在列表里——
/// 用户得看到它，才知道为什么放不出来。
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "m4a", "mp4", "aac", "wav", "ogg", "opus"];

/// 歌单种类。显示名由 UI 按 kind 决定（`Folder` 用 `name`），core 不写界面文案。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalPlaylistKind {
    /// 「全部歌单」：根目录下的直接子文件 + 所有子文件夹的曲目。
    All,
    /// 根目录的一个直接子文件夹。
    Folder,
}

/// 本地曲目。
#[derive(Clone, Debug)]
pub struct LocalTrack {
    /// 音频文件在磁盘上的真实路径（绝对路径，扫描时由根目录拼出）。
    pub path: PathBuf,
    /// 曲名；标签缺失时用文件名（不含扩展名）。
    pub title: String,
    /// 歌手；标签缺失时用「未知歌手」。
    pub artist: String,
    /// 专辑；标签缺失时为空串。
    pub album: String,
    /// 标签时长；拿不到（空文件、坏文件）时为 0，界面显示 `--:--`。
    pub duration_seconds: u32,
}

impl LocalTrack {
    /// 转成播放器认识的 [`TrackItem`]（id = `"local:" + path`）。
    pub fn to_item(&self) -> TrackItem {
        TrackItem {
            id: format!("{LOCAL_ID_PREFIX}{}", self.path.to_string_lossy()),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            duration_seconds: i64::from(self.duration_seconds),
        }
    }
}

/// 一个歌单。`All` 的 `name` 是空串（界面按 kind 出文案）。
#[derive(Clone, Debug)]
pub struct LocalPlaylist {
    /// 歌单种类。
    pub kind: LocalPlaylistKind,
    /// `Folder` 用子文件夹名；`All` 给空串。
    pub name: String,
    /// 这个歌单对应的目录；`All` 是根目录。
    pub dir: PathBuf,
    /// 已按 [`LocalTrack::title`] 小写排序的曲目。
    pub tracks: Vec<LocalTrack>,
}

/// 一次扫描的结果。
#[derive(Clone, Debug, Default)]
pub struct LocalLibrary {
    /// 顺序：`All`、`Folder`（按名字排序）。
    pub playlists: Vec<LocalPlaylist>,
}

impl LocalLibrary {
    /// 「全部歌单」的下标，恒为 0。
    ///
    /// 写成方法而不是常量下标，是为了让调用方读起来是「取全部歌单」而不是「取第 0 个」，
    /// 以后顺序若变化只改这里。
    pub fn all_index(&self) -> usize {
        0
    }

    /// 整个曲库的曲目数。
    ///
    /// 「全部歌单」已经是根目录文件 + 所有子文件夹的并集，直接取它的长度；
    /// 再把各歌单相加会把子文件夹重复计算。
    pub fn track_count(&self) -> usize {
        self.playlists
            .get(self.all_index())
            .map_or(0, |playlist| playlist.tracks.len())
    }
}

/// 同步扫描 `root`，调用者自己放后台线程。
///
/// 根目录不存在 / 不是目录 / 没有权限时返回 `playlists` 为空的库（不 panic、不 Err）：
/// 「选错了文件夹」是正常操作，不是程序错误。
pub fn scan(root: &Path) -> LocalLibrary {
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    let mut library = LocalLibrary {
        playlists: Vec::new(),
    };
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(err) => {
            // 选错文件夹 / 盘没挂上 / 没权限：界面只说「失败」时，这条日志是唯一的区别。
            let path_text = root.display().to_string();
            let detail = err.to_string();
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "scan_root_failed",
                &[("path", path_text.as_str()), ("detail", detail.as_str())],
            );
            return library;
        }
    };

    let mut root_tracks = Vec::new();
    // (文件夹名, 目录, 曲目)；先收集再排序，避免依赖 `read_dir` 的平台相关顺序。
    let mut folders: Vec<(String, PathBuf, Vec<LocalTrack>)> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let Some((is_dir, is_file)) = entry_kind(&entry, &path) else {
            // 断链的快捷方式、正在被独占的名字：跳过这一项，不影响其余扫描。
            continue;
        };
        if is_dir {
            // 以 `.` 开头的目录是隐藏目录（macOS 的 `.Trashes`、同步盘的临时目录），
            // 它们几乎不是用户的歌单；扫进来只会让侧栏多出一堆噪声。
            if name.starts_with('.') {
                continue;
            }
            // 读不了就整条跳过（`System Volume Information` 这类系统目录）：
            // 它不是一个「空歌单」，而是一个打不开的目录，列出来只会让用户以为文件丢了。
            if let Some(tracks) = scan_direct_tracks(&path) {
                folders.push((name, path, tracks));
            }
        } else if is_file && !name.starts_with('.') && is_audio_path(&path) {
            // 隐藏文件名同样跳过：macOS 的 `._foo.mp3` 是 AppleDouble 资源叉，不是音频。
            root_tracks.push(read_track(&path));
        }
    }

    sort_tracks(&mut root_tracks);
    folders.sort_by_cached_key(|folder| (folder.0.to_lowercase(), folder.0.clone()));

    // 「全部歌单」= 根目录直接文件在前，其后各子文件夹按名字顺序，是拼接而不是全局重排：
    // 用户找歌时记住的是「它在哪个文件夹里」，全局按曲名重排会把这份记忆打散。
    let total_folder_tracks: usize = folders.iter().map(|(_, _, tracks)| tracks.len()).sum();
    let mut all_tracks = root_tracks;
    all_tracks.reserve(total_folder_tracks);
    for (_, _, tracks) in &folders {
        all_tracks.extend(tracks.iter().cloned());
    }

    library.playlists.push(LocalPlaylist {
        kind: LocalPlaylistKind::All,
        name: String::new(),
        dir: root,
        tracks: all_tracks,
    });
    for (name, dir, tracks) in folders {
        library.playlists.push(LocalPlaylist {
            kind: LocalPlaylistKind::Folder,
            name,
            dir,
            tracks,
        });
    }

    library
}

/// 扫一个目录的**直接**子文件；不递归——歌单只到这一层。
///
/// 优先使用目录项自带的 `file_type()` 避免额外的 `stat` 系统调用；
/// 仅当条目是符号链接时才调用 `metadata()` 跟随目标类型。
fn entry_kind(entry: &std::fs::DirEntry, path: &Path) -> Option<(bool, bool)> {
    let ft = entry.file_type().ok()?;
    if ft.is_symlink() {
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.is_dir(), meta.is_file()))
    } else {
        Some((ft.is_dir(), ft.is_file()))
    }
}

/// 返回 `None` 表示这个目录根本读不了（权限、系统目录），调用方据此跳过整个歌单；
/// 返回 `Some(vec![])` 才是「目录读到了，但里面没有音乐」——空歌单要留下。
fn scan_direct_tracks(dir: &Path) -> Option<Vec<LocalTrack>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            let path_text = dir.display().to_string();
            let detail = err.to_string();
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "scan_folder_failed",
                &[("path", path_text.as_str()), ("detail", detail.as_str())],
            );
            return None;
        }
    };
    let mut tracks = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Some((_, is_file)) = entry_kind(&entry, &path) else {
            continue;
        };
        if is_file && is_audio_path(&path) {
            tracks.push(read_track(&path));
        }
    }
    sort_tracks(&mut tracks);
    Some(tracks)
}

/// 扩展名是否在白名单里（大小写不敏感；Windows 上 `MP3` 很常见）。
fn is_audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            AUDIO_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// 按曲名小写排序；撞名时按路径兜底，保证同一目录每次扫出来的顺序完全相同。
fn sort_tracks(tracks: &mut [LocalTrack]) {
    tracks.sort_by_cached_key(|track| (track.title.to_lowercase(), track.path.clone()));
}

/// 读一首歌的标签与时长；任何一步失败都退化成「文件名 + 未知歌手 + 时长 0」。
///
/// 这里刻意**不**把读失败的文件丢掉：空文件、半截下载、解不了的编码都还是用户目录里的
/// 一首歌，列表里看得见才解释得清为什么放不出来。解码失败由播放引擎报错。
fn read_track(path: &Path) -> LocalTrack {
    let fallback_title = || {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned())
    };
    let mut track = LocalTrack {
        path: path.to_owned(),
        title: String::new(),
        artist: "未知歌手".to_string(),
        album: String::new(),
        duration_seconds: 0,
    };

    // 扫描曲库时关闭封面解析，避免把每首歌数 MB 的内嵌图片读入堆内存后再丢弃。
    let tagged = lofty::probe::Probe::open(path).ok().and_then(|probe| {
        let options = lofty::config::ParseOptions::new().read_cover_art(false);
        probe.options(options).read().ok()
    });
    let Some(tagged) = tagged else {
        track.title = fallback_title();
        return track;
    };
    // 时长优先信容器头：mp3 有 Xing 头、flac 有 STREAMINFO，都能直接算出来，不用先解码。
    track.duration_seconds = tagged
        .properties()
        .duration()
        .as_secs()
        .min(u64::from(u32::MAX)) as u32;

    // `primary_tag` 是格式认为「应该看它」的那个标签（mp3 的 ID3v2、flac 的 Vorbis Comment），
    // 没有时退回第一个，避免只认一种标签的旧文件读不出标题。
    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        // 标签里的空串（很多抓轨工具会写空标题）比没有标签更糟：当作缺失处理。
        if let Some(title) = tag
            .title()
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            track.title = title.to_string();
        }
        if let Some(artist) = tag
            .artist()
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            track.artist = artist.to_string();
        }
        if let Some(album) = tag
            .album()
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            track.album = album.to_string();
        }
    }
    if track.title.is_empty() {
        track.title = fallback_title();
    }

    track
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_DIR: AtomicU32 = AtomicU32::new(0);

    /// 每个用例独占一个临时目录：进程 id 避开并行跑的不同测试进程，自增序号避开同进程内的用例。
    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sodam-local-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        // 上一次崩溃可能留下同名目录；清掉再看断言，免得读到旧文件。
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 造一个空文件——`lofty` 读不出标签，正好覆盖「标签缺失」这条回退路径。
    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("建父目录");
        }
        std::fs::write(path, b"").expect("写空文件");
    }

    #[test]
    fn scan_orders_playlists_and_keeps_empty_folders() {
        let root = temp_root();
        touch(&root.join("a.mp3"));
        touch(&root.join("sub1").join("b.mp3"));
        std::fs::create_dir_all(root.join("sub2")).expect("空歌单目录");
        touch(&root.join("sub3").join("c.flac"));
        // 隐藏目录不参与扫描，也不该占据一个歌单位。
        touch(&root.join(".hidden").join("d.mp3"));

        let library = scan(&root);
        let shape: Vec<(LocalPlaylistKind, &str)> = library
            .playlists
            .iter()
            .map(|playlist| (playlist.kind, playlist.name.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (LocalPlaylistKind::All, ""),
                (LocalPlaylistKind::Folder, "sub1"),
                (LocalPlaylistKind::Folder, "sub2"),
                (LocalPlaylistKind::Folder, "sub3"),
            ],
            "顺序必须是 All、按名字排序的子文件夹"
        );
        assert_eq!(library.all_index(), 0);
        assert_eq!(library.playlists[0].tracks.len(), 3, "全部歌单 = 1 + 1 + 1");
        assert_eq!(library.track_count(), 3);
        // sub2 是空歌单：仍然存在，只是没有曲目。
        assert_eq!(library.playlists[2].kind, LocalPlaylistKind::Folder);
        assert_eq!(library.playlists[2].name, "sub2");
        assert!(library.playlists[2].tracks.is_empty());
        // 每首都在它自己那一档里，All 里的顺序 = 根目录文件 + 各子文件夹。
        assert_eq!(library.playlists[1].tracks[0].title, "b");
        assert_eq!(library.playlists[3].tracks[0].title, "c");
        let all_titles: Vec<&str> = library.playlists[0]
            .tracks
            .iter()
            .map(|track| track.title.as_str())
            .collect();
        assert_eq!(all_titles, vec!["a", "b", "c"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_files_still_scan_with_filename_fallback() {
        let root = temp_root();
        touch(&root.join("no tags.mp3"));
        touch(&root.join("sub").join("song.flac"));

        let library = scan(&root);
        let track = library.playlists[0]
            .tracks
            .iter()
            .find(|track| track.title == "no tags")
            .expect("根目录文件应出现在全部歌单里");
        assert_eq!(track.title, "no tags", "标签缺失时用文件名（去扩展名）");
        assert_eq!(track.artist, "未知歌手");
        assert!(track.album.is_empty());
        assert_eq!(track.duration_seconds, 0, "读不到时长就是 0");

        // 播放器认识的转换路径：id 带前缀，指向真实文件。
        let item = track.to_item();
        assert!(item.id.starts_with(LOCAL_ID_PREFIX));
        assert!(item.id.ends_with("no tags.mp3"));
        assert_eq!(item.duration_seconds, 0);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_or_unreadable_root_yields_empty_library() {
        let root = temp_root().join("does-not-exist");
        let library = scan(&root);
        assert!(library.playlists.is_empty(), "读不到根目录时没有歌单");
        assert_eq!(library.track_count(), 0);
    }

    #[test]
    fn tracks_sort_by_lowercased_title() {
        let root = temp_root();
        touch(&root.join("Bravo.mp3"));
        touch(&root.join("alpha.mp3"));
        touch(&root.join("Charlie.mp3"));

        let library = scan(&root);
        let titles: Vec<&str> = library.playlists[0]
            .tracks
            .iter()
            .map(|track| track.title.as_str())
            .collect();
        assert_eq!(titles, vec!["alpha", "Bravo", "Charlie"], "按小写曲名排序");
        assert_eq!(library.playlists[0].tracks.len(), 3);

        let _ = std::fs::remove_dir_all(&root);
    }
}
