# 本地音乐库（文件夹即歌单，不联网）

本文件是本地音乐库的**接口契约**。三个执行者（core / shell / view）都按这里的名字写代码，
不要自行改名。任何名字冲突以本文件为准。

## 产品行为

1. 应用没有账号概念：启动直接进「本地音乐」页；侧边栏只有「本地音乐」一项。
2. 本地音乐页的「选择/更改文件夹」按钮用系统文件夹选择框（Windows 用 IFileOpenDialog）。
3. 被选目录下的**直接子文件**（音乐文件）归入「全部歌单」，不单独占据一个歌单。
4. 被选目录的**每个直接子文件夹** = 一个歌单，歌单名 = 子文件夹名（只扫该子文件夹的直接子文件，
   不再往更深层递归）。
5. 子文件夹里没有音乐 → 该歌单仍然出现，但是空的（显示空态，不隐藏）。
6. 「全部歌单」= 根目录直接子文件 + 所有子文件夹的歌，顺序：根目录文件在前，其后按子文件夹名排序。
7. 歌单展示顺序：「全部歌单」，然后按名字排序的子文件夹歌单。
8. 选中的文件夹路径持久化到 `config.json`；下次启动自动重新扫描。
9. 点歌单 → 下面列出该歌单曲目；单击曲目播放；「播放全部」按歌单顺序入队。
10. 本地播放**完全不联网**：不联网、没有签名服务、没有缓存下载；文件就是文件。
11. 播放失败（文件被移走、解码失败）给一句可行动的原因，不自动跳歌。

## 支持的音乐扩展名

`mp3`、`flac`、`m4a`、`mp4`、`aac`、`wav`、`ogg`、`opus`（大小写不敏感）。
解码能力由 rodio 的 feature 决定；opus 目前没有解码器（Symphonia 还不支持），
文件会出现在列表里，播放时给「文件读不出来」的人话文案。

## 契约 A：`crates/sodam-core/src/local.rs`（core）

```rust
/// 本地曲目的 track id 前缀，后接绝对路径字符串。
pub const LOCAL_ID_PREFIX: &str = "local:";
/// 参与扫描的扩展名（小写形态）。
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "m4a", "mp4", "aac", "wav", "ogg", "opus"];

pub enum LocalPlaylistKind { All, Folder }

pub struct LocalTrack { path, title, artist, album, duration_seconds }
pub struct LocalPlaylist { kind, name, dir, tracks }
pub struct LocalLibrary { playlists }   // { all_index() == 0, track_count() }

/// 同步扫描；调用者放后台线程。
pub fn scan(root: &Path) -> LocalLibrary;
```

扫描规则：

* 根目录不存在 / 没权限 → `playlists` 为空（不 panic、不 Err）。
* 每个文件、每个子目录的错误都「跳过它自己」，不影响其余扫描。
* 以 `.` 开头的隐藏目录/文件不扫。
* 标签缺失时：曲名 = 文件名（去扩展名）、歌手 = 「未知歌手」、时长 = 0（界面显示 `--:--`）。

## 契约 B：`crates/sodam/src/app.rs`（shell）

`Root` 持有扫描状态：

```rust
pub(crate) local_library: Option<Arc<sodam_core::local::LocalLibrary>>,
pub(crate) local_loading: bool,
pub(crate) local_error: Option<String>,
pub(crate) local_selected: usize,           // 当前选中的歌单下标，默认 0（全部歌单）
pub(crate) local_scroll: gpui::UniformListScrollHandle,
```

启动时若 `settings.local_music_folder` 非空就调 `scan_local_library(cx)`。

## 契约 C：`crates/sodam/src/app_local.rs`（动作）

```rust
impl Root {
    /// 系统文件夹选择框 → 写 settings.local_music_folder → save → 重新扫描。
    pub(crate) fn pick_local_folder(&mut self, cx: &mut Context<Self>);
    /// 用 settings.local_music_folder 后台扫描，回 UI 写 local_library / local_loading / local_error。
    pub(crate) fn scan_local_library(&mut self, cx: &mut Context<Self>);
    /// 清空文件夹（只清设置，不删文件）。
    pub(crate) fn clear_local_folder(&mut self, cx: &mut Context<Self>);
    /// 选中歌单。
    pub(crate) fn select_local_playlist(&mut self, index: usize, cx: &mut Context<Self>);
    /// 播放某歌单全部 / 某一首（index 为该歌单内的曲目下标）。
    pub(crate) fn play_local_playlist(&mut self, playlist: usize, cx: &mut Context<Self>);
    pub(crate) fn play_local_track(&mut self, playlist: usize, track: usize, cx: &mut Context<Self>);
    /// 歌单显示名（UI 文案集中在这里，core 不认识中文）。
    pub(crate) fn local_playlist_name(&self, playlist: &sodam_core::local::LocalPlaylist) -> String;
}
```

约定：

* `pick_folder_blocking` 只在 Windows 可用（`rfd`/IFileOpenDialog）；其余平台返回 `None`，
  UI 需要给出清楚的提示（见 docs/ROADMAP.md 的 Linux 通路待办）。
* 扫描结果在后台线程完成后整体替换 `local_library`；扫描期间换过文件夹的话结果作废。

## 契约 D：`crates/sodam/src/views/local.rs`（view）

只负责画。入场：标题 + 动作行 + 文件夹路径；选定歌单后画卡片行（选中态 accent 描边）、
标题 + 「播放全部」、曲目列表（`uniform_list` + `track_scroll` + 滚轮平滑滚动）。
空态不空页：未选文件夹画引导；有 `local_error` 画失败原因；空歌单画空一个曲目列表。
