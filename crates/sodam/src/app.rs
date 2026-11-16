//! 应用根视图：状态定义、初始化、顶层渲染。纯本地播放器：不联网、不登录。

#[path = "app_actions.rs"]
mod actions;
#[path = "app_artwork.rs"]
mod artwork_actions;
#[path = "app_lyrics.rs"]
mod lyrics_actions;
#[path = "app_theme.rs"]
mod theme_actions;

pub(crate) use lyrics_actions::{line_progress, lyric_tokens, LyricToken};

use gpui::prelude::*;
use gpui::{div, px, AnimationExt, Context, Decorations, FocusHandle, Render, Window};

use crate::artwork::ArtworkView;
use sodam_core::{
    log_outlet::{self, log_event, LogLevel, LogTopic},
    lyrics::Lyrics,
    models::TrackItem,
    queue::Queue,
    PlaybackEngine, Settings,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 本地保存/写入类失败的统一说法：这类失败和磁盘有关。
pub(crate) const SAVE_TEXT: &str = "没能保存，检查磁盘空间后重试。";

/// 队列右键菜单状态：被点中的行下标与鼠标位置。
pub struct QueueMenu {
    pub index: usize,
    pub position: gpui::Point<gpui::Pixels>,
}

/// 封面主题色的过渡状态：切歌/切歌单时 9 阶色板连续变化。
pub(crate) struct PaletteAnim {
    pub from: sodam_core::artwork::Palette,
    pub to: sodam_core::artwork::Palette,
    /// 最终目标；`None` 表示没有封面，过渡结束后回到中性主题。
    pub target: Option<sodam_core::artwork::Palette>,
    pub started: std::time::Instant,
}

/// 曲目文字的交叉淡化：标题/歌手/专辑一起换，用户看到的是一次完整的内容更替。
pub(crate) struct TrackTextSwap {
    pub from_title: String,
    pub from_artist: String,
    pub from_album: String,
    pub seq: u64,
    /// 新内容进入方向：+1 从下（下一首），-1 从上（上一首）。
    pub direction: f32,
    pub changed_at: std::time::Instant,
}

impl TrackTextSwap {
    /// 只在动画窗口内渲染旧文案；窗口外直接显示新内容，切页回来不会重播。
    pub(crate) fn active(&self) -> bool {
        self.changed_at.elapsed() < ui::motion::duration::CONTENT * 2
    }
}

/// 一次歌单切换的统一事务：所有元素共用同一个开始时刻和切换序号。
///
/// 旧实现让标题、头图、列表、色板各自记录时间，快速 A → B → C 时容易出现
/// 一部分重播、一部分继续旧动画。现在只在这里记一份时间基准。
pub(crate) struct PlaylistTransition {
    pub(crate) from_index: usize,
    pub(crate) to_index: usize,
    pub(crate) started_at: std::time::Instant,
    pub(crate) sequence: u64,
}

/// 一个独立插值的 0..1 值；重新设目标时从当前值出发。
#[derive(Clone, Copy)]
pub(crate) struct RowAnim {
    pub from: f32,
    pub to: f32,
    pub started: std::time::Instant,
    pub duration: std::time::Duration,
    pub seq: u64,
}

impl RowAnim {
    fn new(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            started: std::time::Instant::now(),
            duration: std::time::Duration::ZERO,
            seq: 0,
        }
    }

    /// 当前值（按动画实际使用的 ease-out-quint 估算）。
    pub(crate) fn current(&self, now: std::time::Instant) -> f32 {
        if self.duration.is_zero() {
            return self.to;
        }
        let t = (now.saturating_duration_since(self.started).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(5);
        self.from + (self.to - self.from) * eased
    }

    /// 换目标：从当前值续接，不跳回 0/1。
    pub(crate) fn retarget(
        &mut self,
        to: f32,
        duration: std::time::Duration,
        now: std::time::Instant,
    ) {
        if (self.to - to).abs() < f32::EPSILON {
            return;
        }
        self.from = self.current(now);
        self.to = to;
        self.started = now;
        self.duration = duration;
        self.seq = self.seq.wrapping_add(1);
    }

    /// 动画结束后把起点收敛到终点。
    ///
    /// 不做这一步的话，行会一直走 `with_animation` 分支，动画元素和它的
    /// element state 会一直挂着；行离开/重新挂载时还可能从旧的 `from` 重播。
    pub(crate) fn settle(&mut self, now: std::time::Instant) -> bool {
        if self.duration.is_zero() {
            return false;
        }
        if now.saturating_duration_since(self.started) < self.duration {
            return false;
        }
        self.from = self.to;
        self.duration = std::time::Duration::ZERO;
        true
    }
}

/// 侧栏一行自己的两个动画：背景透明度与文字亮度。
#[derive(Clone, Copy)]
pub(crate) struct SidebarRowAnim {
    pub bg: RowAnim,
    pub text: RowAnim,
    /// 这一行当前是否在指针下；每行各记一份，不依赖悬停事件的先后顺序。
    pub hovered: bool,
}

impl SidebarRowAnim {
    pub(crate) fn new(selected: bool, hovered: bool) -> Self {
        let bg = if selected {
            0.13
        } else if hovered {
            0.07
        } else {
            0.0
        };
        Self {
            bg: RowAnim::new(bg),
            text: RowAnim::new(if selected { 1.0 } else { 0.0 }),
            hovered,
        }
    }

    /// 悬停进出：只重设背景目标，选中行始终 13% 白。
    pub(crate) fn set_hovered(
        &mut self,
        hovered: bool,
        selected: bool,
        now: std::time::Instant,
    ) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        if selected {
            return true;
        }
        let target = if hovered { 0.07 } else { 0.0 };
        let duration = if hovered {
            ui::motion::duration::HOVER
        } else {
            ui::motion::duration::SIDEBAR_HOVER_OUT
        };
        self.bg.retarget(target, duration, now);
        true
    }
}

/// 「正在播放」行的高亮过渡：旧行淡出、新行淡入，序号和主题色一起变化。
#[derive(Default)]
pub(crate) struct NowPlayingFade {
    pub current: Option<String>,
    pub previous: Option<String>,
    pub changed_at: Option<std::time::Instant>,
}

impl NowPlayingFade {
    /// 记录当前曲目变化。返回是否发生了变化。
    pub(crate) fn note(&mut self, next: Option<String>) -> bool {
        if self.current == next {
            return false;
        }
        self.previous = self.current.take();
        self.current = next;
        self.changed_at = Some(std::time::Instant::now());
        true
    }

    /// 本次高亮过渡的进度（0..1）。渲染时取一次，供列表行闭包使用。
    pub(crate) fn progress(&self) -> f32 {
        match self.changed_at {
            Some(at) => ui::motion::progress(at, ui::motion::duration::PLAYING),
            None => 1.0,
        }
    }

    /// 过渡是否还在进行（渲染时据此请求下一帧）。
    pub(crate) fn animating(&self) -> bool {
        self.changed_at
            .is_some_and(|at| at.elapsed() < ui::motion::duration::PLAYING)
    }
}

/// 「正在播放」行的高亮强度：旧行从 1 连续降到 0，新行从 0 升到 1。
///
/// `id` 可以是完整 track id，也可以只是本地路径：队列行传完整 id，
/// 本地列表行传路径，两种写法都在这里归一。
pub(crate) fn playing_strength(
    current: Option<&str>,
    previous: Option<&str>,
    t: f32,
    id: &str,
) -> f32 {
    let matches = |candidate: &str| {
        candidate == id
            || candidate
                .strip_prefix(sodam_core::local::LOCAL_ID_PREFIX)
                .is_some_and(|path| path == id)
    };
    if current.is_some_and(matches) {
        t
    } else if previous.is_some_and(matches) {
        1.0 - t
    } else {
        0.0
    }
}

/// 队列删除后的一小段时间：被删的行先用一层「幽灵」淡出，
/// 下面的行同时上移归位。
pub(crate) struct QueueRemoval {
    pub epoch: u64,
    /// 被删行在虚拟列表里的槽位。
    pub slot: usize,
    pub track: TrackItem,
    /// 删除时的滚动偏移，决定幽灵行的屏幕位置。
    pub scroll_top: f32,
    pub at: std::time::Instant,
}

/// 控件悬停监听器的具体类型（避免在 `impl Trait` 里泄露 `Context` 的生命周期）。
pub(crate) type HoverListener = Box<dyn Fn(&bool, &mut gpui::Window, &mut gpui::App) + 'static>;

/// 根视图状态。
pub struct Root {
    /// 自绘标题栏拖拽区的按下锚点：按下只记录，移动超过阈值才交给合成器，
    /// 否则合成器会抢走指针，双击最大化收不到第二次按下（见 ui::titlebar）。
    pub(crate) titlebar_anchor: ui::titlebar::DragAnchor,
    pub(crate) sidebar_logo: Option<(i32, Arc<gpui::RenderImage>)>,
    pub language: Language,
    pub settings: Settings,
    pub queue: Queue,
    /// 底部状态行：普通操作反馈的文案与生命周期（渲染见 `ui::status_line`）。
    pub(crate) status_line: ui::status_line::StatusLine,
    /// 全局 message 队列与计时去重集合。
    pub(crate) messages: Vec<ui::message::Message>,
    pub(crate) next_message_id: u64,
    pub(crate) message_timers: HashSet<u64>,
    /// 窗口根元素的焦点。
    ///
    /// 必须有：gpui 的上下文栈取自「聚焦节点到根的路径」，没有聚焦元素时栈是空的，
    /// 带上下文谓词的快捷键绑定（播放三键绑在 `workspace` 上）永远不会匹配。
    /// 所以根元素自己持焦点，点任意可交互元素时焦点自然让出去。
    pub workspace_focus: FocusHandle,
    /// 顶栏搜索框的编辑状态（本地曲库过滤）。
    pub(crate) search_field: crate::ui::text_field::TextField,
    pub(crate) search_focus: FocusHandle,
    /// 搜索框在窗口里的位置：输入法候选窗要贴在这里。
    pub(crate) search_bounds: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// 本地音乐库的扫描结果（`None` = 还没扫或没选文件夹）。
    ///
    /// 用 `Arc` 缓存：整库可能上万首，渲染时每个列表行都要读，clone 一次句柄比
    /// 每次重算便宜得多；扫描在线程里跑完整体替换，不原地改。
    pub(crate) local_library: Option<Arc<sodam_core::local::LocalLibrary>>,
    /// 正在扫描（决定页面显示「正在扫描」而不是空态）。
    pub(crate) local_loading: bool,
    pub(crate) local_scan_generation: u64,
    /// 扫描失败的可见原因（读不到目录等）；`None` = 没有失败。
    pub(crate) local_error: Option<String>,
    /// 搜索结果的缓存（查询词、曲库指针、结果）：播放中每帧重绘时不再全库扫描。
    pub(crate) search_cache: Option<(String, usize, Arc<Vec<sodam_core::local::LocalTrack>>)>,
    /// 当前选中的歌单下标（`0` 恒为「全部歌单」）。
    pub(crate) local_selected: usize,
    /// 侧栏上一次渲染时选中的歌单下标（只用于检测变化）。
    pub(crate) sidebar_selection_shown: usize,
    /// 播放页打开时隐藏侧栏选中；这里记录可见状态，隐藏/恢复都用同一套淡出。
    pub(crate) sidebar_selection_visible: bool,
    /// 每个歌单行独立的背景/文字动画状态。
    pub(crate) sidebar_rows: HashMap<usize, SidebarRowAnim>,
    /// 细条标记的起点与代次；相邻行滑动，远距离交叉淡化。
    pub(crate) sidebar_marker_from: usize,
    pub(crate) sidebar_marker_seq: u64,
    /// 全窗控件的悬停淡入淡出状态（按钮等非列表控件）。
    pub(crate) hovers: ui::motion::HoverStates,
    pub(crate) local_hover: ui::motion::RowHover,
    pub(crate) search_hover: ui::motion::RowHover,
    pub(crate) queue_hover: ui::motion::RowHover,
    pub(crate) lyrics_hover: ui::motion::RowHover,
    /// 搜索框上一次渲染时的查询词：结果集变化后重置悬停行，避免高亮错位。
    pub(crate) search_hover_query: Option<String>,
    /// 曲目文字（标题/歌手/专辑）的交叉淡化状态。
    pub(crate) track_text_shown: Option<(String, String, String)>,
    pub(crate) track_text_swap: Option<TrackTextSwap>,
    pub(crate) track_text_seq: u64,
    /// 下一次文字交换的进入方向：+1 下一首 / -1 上一首 / 0 默认。
    pub(crate) track_swap_dir: f32,
    /// 当前歌单切换事务（标题/头图/列表共用同一时间基准）。
    pub(crate) playlist_transition: Option<PlaylistTransition>,
    pub(crate) playlist_transition_seq: u64,
    /// 头图当前显示的是哪个歌单、哪张源封面；用于区分「换歌单」和「同图升级」。
    pub(crate) hero_shown: Option<(usize, Option<String>)>,
    /// 封面双层交叉淡化：歌单头图 / 播放栏小封面 / 播放页大封面。
    pub(crate) hero_cover_fade: ui::motion::CoverFade,
    pub(crate) bar_cover_fade: ui::motion::CoverFade,
    pub(crate) page_cover_fade: ui::motion::CoverFade,
    /// 播放/暂停、播放模式、静音图标的交叉淡化。
    pub(crate) play_icon_shown: bool,
    pub(crate) play_icon_seq: u64,
    pub(crate) mode_icon_shown: &'static str,
    pub(crate) mode_icon_seq: u64,
    pub(crate) mute_icon_shown: bool,
    pub(crate) mute_icon_seq: u64,
    /// 搜索框焦点描边的过渡（只在状态真正变化时换代次，首帧不闪）。
    pub(crate) search_focus_shown: bool,
    pub(crate) search_focus_seq: u64,
    /// 正在播放行的高亮过渡。
    pub(crate) now_playing_fade: NowPlayingFade,
    /// 队列删除后的幽灵行与重排动画。
    pub(crate) queue_removal: Option<QueueRemoval>,
    /// 队列删除重排的代次：每次删除换一个动画 id，行才会重新播归位动画。
    pub(crate) queue_reflow_seq: u64,
    /// 队列右键菜单的弹出/收起动画。
    pub(crate) queue_menu_seq: u64,
    pub(crate) queue_menu_closing: bool,
    /// 本地扫描的加载指示延后出现（任务很快完成就不闪一下 spinner）。
    pub(crate) local_loading_visible: bool,
    /// 本地曲目列表的滚动句柄。
    pub(crate) local_scroll: gpui::UniformListScrollHandle,
    /// 滚轮平滑滚动状态：每个列表一个槽位，逐帧缓动推进。
    pub(crate) scroll_anims: ui::smooth_scroll::ScrollAnims,
    /// 侧栏歌单列表的滚动句柄（滚轮缓动 + 与主列表隔离）。
    pub(crate) sidebar_scroll: gpui::ScrollHandle,
    /// 队列抽屉的滚动句柄（打开时锚到「正在播放」）。
    pub queue_scroll: gpui::UniformListScrollHandle,
    /// 队列抽屉是否展开。
    pub queue_open: bool,
    /// 队列正在播放关闭动画：面板还没卸载，等滑出结束再移除。
    pub(crate) queue_closing: bool,
    /// 队列开合动画代次：每次开/关换一个元素 id，动画才会重播。
    pub(crate) queue_anim_seq: u64,
    /// 队列开合动画的起始时刻。只在动画窗口内挂 `with_animation`；
    /// 窗口外直接落最终位置，避免页面后续重绘时动画状态丢失、面板回到屏外。
    pub(crate) queue_anim_at: Option<std::time::Instant>,
    pub(crate) preferences_writer: sodam_core::preferences::PreferencesWriter,
    /// 进度条轨道在窗口里的位置（画布测量，鼠标事件据此换算 seek 位置）。
    pub progress_track_bounds: Arc<Mutex<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// 音量轨道在窗口里的位置（画布测量，鼠标事件据此换算音量）。
    pub volume_track_bounds: Arc<Mutex<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// 队列右键菜单。
    pub queue_menu: Option<QueueMenu>,
    /// 播放请求序号：每次 start_track +1。
    ///
    /// 作用：装载是异步的，如果用户在 A 装载途中换了 B，
    /// A 完成时必须**作废**，否则会把正在播的 B 顶掉（实测踩过）。
    pub(crate) play_seq: u64,
    /// 播放错误（显示在播放栏：错误必须有地方可见）。
    pub playback_error: Option<String>,
    /// 交给音频线程的装载序号；避免同一曲目重载/失败时永久 loading。
    pub(crate) pending_engine_load: Option<u64>,
    /// 正在加载的曲目：非空时播放栏显示 loading，且进度条不可拖。
    pub pending_track: Option<TrackItem>,
    /// 已处理过的完成事件（曲目结束序号 + 跳转序号）。
    pub(crate) last_finished_seq: u64,
    pub(crate) last_seek_seq: u64,
    /// 拖动进度条时的预览位置（0.0~1.0）：拖动中只改它，松手才真跳。
    pub progress_preview: Option<f32>,
    /// 队列快照（含版本号）：避免抽屉每帧深拷贝整条队列。
    pub queue_cache: (u64, Arc<Vec<TrackItem>>),
    /// 真实播放过的曲目 id（最近在后）。用于队列抽屉的「已播放」分区；
    /// 不能用「当前下标之前」来推 —— 那会把跳过的歌也算进去（实测踩过）。
    pub played_history: Vec<String>,
    /// 启动时读到的上次会话与进度：曲库扫完后按它们恢复（只尝试一次）。
    pub(crate) saved_session: Option<sodam_core::session::PlaybackSession>,
    pub(crate) saved_checkpoint: Option<sodam_core::session::PlaybackCheckpoint>,
    /// 本次启动是否还要恢复上次会话（扫描完成后清掉）。
    pub(crate) session_pending: bool,
    pub(crate) playback_generation: u64,
    /// 只有这次装载的快照可以保存进度。
    pub(crate) checkpoint_load_seq: Option<u64>,
    /// 下一次 `start_track` 要装载到的秒数并保持暂停；`None` = 正常从头播放。
    pub(crate) resume_at: Option<f64>,
    /// 会话与播放进度的后台单写者。
    pub(crate) session_writer: sodam_core::session::SessionWriter,
    /// 上次写进进度文件的曲目与播放状态：切歌/暂停立刻落盘，播放中每 5 秒一次。
    pub(crate) checkpoint_track: String,
    pub(crate) checkpoint_playing: bool,
    pub(crate) checkpoint_at: std::time::Instant,
    /// 当前曲目的封面（有封面才有值；异步准备，晚到不覆盖新曲目）。
    pub(crate) artwork: Option<ArtworkView>,
    /// 当前选中的歌单封面：没在播放时用它给整窗背景兜底。
    pub(crate) playlist_art: Option<ArtworkView>,
    /// 当前生效的封面色板（渐变的起点/中途值）。
    pub(crate) palette_current: sodam_core::artwork::Palette,
    /// 正在进行的色板渐变；由渲染帧推进（不再用 16ms 定时器）。
    pub(crate) palette_anim: Option<PaletteAnim>,
    /// 封面异步请求序号：旧请求回来时不能覆盖新曲目。
    pub(crate) artwork_seq: u64,
    pub(crate) artwork_pending: bool,
    flow_clock: ui::backdrop::FlowClock,
    pub(crate) light_mode_anim: Option<(f32, f32, std::time::Instant)>,
    pub(crate) flow_mix_anim: Option<(f32, f32, std::time::Instant)>,
    pub(crate) flow_target: f32,
    /// 歌单封面异步请求序号（选中的歌单可能很快又被换掉）。
    pub(crate) playlist_art_seq: u64,
    /// 曲目缩略图缓存：绝对路径 → 96px PNG（`None` = 这首歌没有内嵌封面）。
    ///
    /// 用 `Arc` 包一层：列表渲染每帧都要把快照交给 `uniform_list` 的闭包，
    /// 深拷贝几百个路径会把滚动拖卡。
    pub(crate) cover_thumbs: std::sync::Arc<HashMap<String, Option<PathBuf>>>,
    /// 缩略图对应的色板：切歌时不等全尺寸封面准备完，先用它开始颜色过渡。
    pub(crate) cover_palettes: HashMap<String, Option<sodam_core::artwork::Palette>>,
    /// 预热时顺手落盘的全尺寸封面：切歌时直接读这个文件，
    /// 不再为了一首歌重新走一遍 lofty 标签解析。
    pub(crate) cover_files: HashMap<String, PathBuf>,
    /// 歌单头图缓存：歌单下标 → 320px PNG（每张歌单只取第一首有封面的歌）。
    pub(crate) hero_thumbs: HashMap<usize, PathBuf>,
    /// 320px 头图对应的源文件；用来判断 96→320 是否只是同一张图升级。
    pub(crate) hero_sources: HashMap<usize, String>,
    /// 已在请求中的缩略图，避免同一行每帧重复排队。
    pub(crate) cover_requests: HashSet<String>,
    /// 缩略图预热代数：重新扫描后旧任务的结果直接丢弃。
    pub(crate) cover_prewarm_seq: u64,
    /// 是否展开「正在播放」页（大封面 + 歌词）。
    pub(crate) player_view_open: bool,
    /// 是否打开设置页。
    pub(crate) settings_open: bool,
    /// 设置页滚动句柄。
    pub(crate) settings_scroll: gpui::ScrollHandle,
    pub(crate) settings_active_section: usize,
    pub(crate) settings_scroll_lock_until: Option<std::time::Instant>,
    pub(crate) settings_slider_drag: Option<&'static str>,
    /// 当前曲目的歌词（内嵌歌词 / 侧车 .lrc）。
    pub(crate) lyrics: Option<Arc<Lyrics>>,
    /// 歌词排版缓存：断句、动态行高与累计偏移；歌词或面板宽度变化时重建。
    pub(crate) lyrics_layout: Option<views::player::LyricsLayout>,
    /// 译文开关动画：代次、起始透明度、起始时刻。
    pub(crate) lyrics_translation_seq: u64,
    pub(crate) lyrics_translation_from: f32,
    pub(crate) lyrics_translation_at: Option<std::time::Instant>,
    /// 歌词对应的曲目 id，避免晚到的结果贴到新曲目上。
    pub(crate) lyrics_for: Option<String>,
    /// 歌词异步请求序号。
    pub(crate) lyrics_seq: u64,
    /// 当前高亮的歌词行。
    pub(crate) lyrics_active: Option<usize>,
    /// 上一帧高亮的行：用于让「刚离开当前行」平滑缩小。
    pub(crate) lyrics_prev_active: Option<usize>,
    pub(crate) lyrics_prev_growth: f32,
    /// 歌词缩放动画代次：每次换行换一个元素 id，动画才会重播。
    pub(crate) lyrics_anim_seq: u64,
    /// 最近一次换行的时刻：旧行保留「已唱完」样式做退场动画，超时后切回普通样式。
    pub(crate) lyrics_switch_at: Option<std::time::Instant>,
    /// 歌词列表的滚动句柄（跟随当前行居中）。
    pub(crate) lyrics_scroll: gpui::ScrollHandle,
    /// 上一次居中的行号：行号变化或手动滚开后重新锚定。
    pub(crate) lyrics_scroll_line: Option<usize>,
    pub(crate) lyrics_scroll_stagger: Option<(std::time::Instant, f32)>,
    pub(crate) lyrics_last_seek: Option<std::time::Instant>,
    /// 用户手动滚歌词后的暂停跟随时间。
    pub(crate) lyrics_hold_until: Option<std::time::Instant>,
    /// 进度插值锚点：心跳拿到新的 position 时重置，播放中每帧按经过时间外推。
    pub(crate) progress_anchor_at: std::time::Instant,
    pub(crate) progress_anchor_seconds: f64,
    pub(crate) progress_anchor_track: String,
    /// 音频播放引擎（rodio，跑在专属线程上）
    pub engine: PlaybackEngine,
}

use crate::ui;
use crate::ui::i18n::Language;
use crate::ui::theme;
use crate::views;

impl Root {
    /// 构造根视图：加载配置、起本地扫描与播放心跳。
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        // 配置里的「详细日志」要覆盖启动就生效：日志出口在 `main` 里已建好，
        // 这里补上开关状态（环境变量那一路已在出口初始化时读过，两者取并集）。
        log_outlet::set_global_detailed(settings.detailed_logs);
        theme::set_light_mix(if settings.light_mode { 1.0 } else { 0.0 });
        let initial_flow = if settings.flowing_background && settings.flowing_background_global {
            1.0
        } else {
            0.0
        };
        theme::set_flow_mix(initial_flow);
        let engine = PlaybackEngine::with_volume(settings.volume);
        engine.set_loudness_equalization(settings.loudness_equalization);
        let mut settings = settings;
        settings.volume = engine.snapshot().volume;
        // 有上次会话（或旧版本只记了单曲与进度）就等曲库扫完再恢复；此时还不知道文件在不在。
        let session_dir = sodam_core::session::session_dir();
        let saved_session = sodam_core::session::load_session(&session_dir);
        let saved_checkpoint = sodam_core::session::load_checkpoint(&session_dir);
        let playback_generation = saved_session
            .as_ref()
            .map_or(0, |session| session.playback_generation)
            .max(
                saved_checkpoint
                    .as_ref()
                    .map_or(0, |saved| saved.playback_generation),
            );
        let session_pending = saved_session.is_some() || !settings.last_track_id.is_empty();
        // 首次运行就把配置落盘，方便用户直接编辑语言和快捷键。
        if !Settings::config_path().exists() {
            if let Err(error) = settings.save() {
                let detail = error.to_string();
                log_event(
                    LogLevel::Warn,
                    LogTopic::App,
                    "config_save_failed",
                    &[("detail", detail.as_str())],
                );
            }
        }
        // 先写入深色中性 base 色板：首帧还没有封面，页面/面板要靠它取深色底。
        theme::set_ambient_palette(None);
        let language = match Language::from_key(&settings.language) {
            Some(Language::System) | None => Language::system_locale(),
            Some(language) => language,
        };
        // 启动文案是写死的常量，不含任何接口地址；构造期也还没有 `Root`，
        // 直接放进状态行即可，不必绕 `remember_status`。
        let mut status_line = ui::status_line::StatusLine::default();
        status_line.set(
            language
                .text("本地音乐就绪：点击顶栏的文件夹图标选择音乐文件夹")
                .to_string(),
        );
        let mut queue = Queue::new(Vec::<TrackItem>::new());
        // 播放模式跨启动保留（切换按钮在播放栏）。
        queue.mode = settings.play_mode;
        let initial_mode_icon = queue.mode.icon();
        let engine_progress = engine.snapshot();
        // 首帧不播译文显隐动画：从配置的当前状态直接开始。
        let lyrics_translation_from = if settings.lyrics_translation {
            1.0
        } else {
            0.0
        };

        let mut root = Self {
            titlebar_anchor: std::rc::Rc::new(std::cell::Cell::new(None)),
            sidebar_logo: None,
            language,
            settings,
            queue,
            played_history: Vec::new(),
            saved_session,
            saved_checkpoint,
            session_pending,
            playback_generation,
            checkpoint_load_seq: None,
            resume_at: None,
            session_writer: sodam_core::session::SessionWriter::new(session_dir),
            checkpoint_track: String::new(),
            checkpoint_playing: false,
            checkpoint_at: std::time::Instant::now(),
            queue_menu: None,
            play_seq: 0,
            playback_error: None,
            status_line,
            messages: Vec::new(),
            next_message_id: 0,
            message_timers: HashSet::new(),
            workspace_focus: cx.focus_handle(),
            search_field: crate::ui::text_field::TextField::default(),
            search_focus: cx.focus_handle(),
            search_bounds: std::rc::Rc::new(std::cell::Cell::new(None)),
            local_library: None,
            local_loading: false,
            local_scan_generation: 0,
            local_error: None,
            search_cache: None,
            local_selected: 0,
            sidebar_selection_shown: 0,
            sidebar_selection_visible: true,
            sidebar_rows: HashMap::new(),
            sidebar_marker_from: 0,
            sidebar_marker_seq: 0,
            hovers: ui::motion::HoverStates::default(),
            local_hover: ui::motion::RowHover::default(),
            search_hover: ui::motion::RowHover::default(),
            queue_hover: ui::motion::RowHover::default(),
            lyrics_hover: ui::motion::RowHover::default(),
            search_hover_query: None,
            track_text_shown: None,
            track_text_swap: None,
            track_text_seq: 0,
            track_swap_dir: 0.0,
            playlist_transition: None,
            playlist_transition_seq: 0,
            hero_shown: None,
            hero_cover_fade: Default::default(),
            bar_cover_fade: Default::default(),
            page_cover_fade: Default::default(),
            play_icon_shown: false,
            play_icon_seq: 0,
            mode_icon_shown: initial_mode_icon,
            mode_icon_seq: 0,
            mute_icon_shown: false,
            mute_icon_seq: 0,
            search_focus_shown: false,
            search_focus_seq: 0,
            now_playing_fade: NowPlayingFade::default(),
            queue_removal: None,
            queue_reflow_seq: 0,
            queue_menu_seq: 0,
            queue_menu_closing: false,
            local_loading_visible: false,
            local_scroll: gpui::UniformListScrollHandle::new(),
            scroll_anims: ui::smooth_scroll::ScrollAnims::default(),
            sidebar_scroll: gpui::ScrollHandle::new(),
            queue_scroll: gpui::UniformListScrollHandle::new(),
            queue_open: false,
            queue_closing: false,
            queue_anim_seq: 0,
            queue_anim_at: None,
            preferences_writer: sodam_core::preferences::PreferencesWriter::new(
                Settings::config_path(),
            ),
            progress_track_bounds: Arc::new(Mutex::new(None)),
            volume_track_bounds: Arc::new(Mutex::new(None)),
            pending_engine_load: None,
            pending_track: None,
            last_finished_seq: engine_progress.finished_seq,
            last_seek_seq: engine_progress.seek_seq,
            progress_preview: None,
            queue_cache: (0, Arc::new(Vec::new())),
            artwork: None,
            playlist_art: None,
            palette_current: sodam_core::artwork::Palette::NEUTRAL_DARK,
            palette_anim: None,
            artwork_seq: 0,
            artwork_pending: false,
            flow_clock: ui::backdrop::FlowClock::default(),
            light_mode_anim: None,
            flow_mix_anim: None,
            flow_target: initial_flow,
            playlist_art_seq: 0,
            cover_thumbs: std::sync::Arc::new(HashMap::new()),
            cover_palettes: HashMap::new(),
            cover_files: HashMap::new(),
            hero_thumbs: HashMap::new(),
            hero_sources: HashMap::new(),
            cover_requests: HashSet::new(),
            cover_prewarm_seq: 0,
            player_view_open: false,
            settings_open: false,
            settings_scroll: gpui::ScrollHandle::new(),
            settings_active_section: 0,
            settings_scroll_lock_until: None,
            settings_slider_drag: None,
            lyrics: None,
            lyrics_layout: None,
            lyrics_translation_seq: 0,
            lyrics_translation_from,
            lyrics_translation_at: None,
            lyrics_for: None,
            lyrics_seq: 0,
            lyrics_active: None,
            lyrics_prev_active: None,
            lyrics_prev_growth: 0.0,
            lyrics_anim_seq: 0,
            lyrics_switch_at: None,
            lyrics_scroll: gpui::ScrollHandle::new(),
            lyrics_scroll_line: None,
            lyrics_scroll_stagger: None,
            lyrics_last_seek: None,
            lyrics_hold_until: None,
            progress_anchor_at: std::time::Instant::now(),
            progress_anchor_seconds: 0.0,
            progress_anchor_track: String::new(),
            engine,
        };
        // 退出时把摘要日志与未落盘的音量刷进配置（同步等待，最多 2 秒）。
        // gpui 的 on_app_quit 要求闭包返回 'static Future：这里同步做完再回一个空 future，
        // 不借用任何 root 状态。
        cx.on_app_quit(|root, _cx| {
            let snapshot = root.engine.snapshot();
            root.persist_checkpoint(&snapshot);
            root.persist_session();
            root.session_writer.flush();
            sodam_core::flush_global();
            root.preferences_writer.flush();
            std::future::ready(())
        })
        .detach();

        // 开发期滚动压测：`SODAM_SCROLL_TEST=1` 时按行推进本地列表滚动，
        // 配合 `SODAM_FRAME_LOG=1`（把 app 话题提到 DEBUG）就能量出「滑动是否掉帧」。
        if std::env::var("SODAM_SCROLL_TEST").is_ok() {
            let handle = root.local_scroll.clone();
            cx.spawn(async move |_this, cx| {
                let mut item = 0usize;
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(32))
                        .await;
                    item = if item > 200 { 0 } else { item + 2 };
                    handle.scroll_to_item(item, gpui::ScrollStrategy::Center);
                    cx.refresh();
                }
            })
            .detach();
        }
        Self::start_heartbeat(cx);

        // 开发验证用：`SODAM_AUTOPLAY=1` 启动即自动播放「全部歌单」第一首；
        // `SODAM_POPUP=queue` 启动即打开队列抽屉（便于截图核对）。
        if std::env::var("SODAM_AUTOPLAY").is_ok() {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(3500))
                    .await;
                let _ = this.update(cx, |root, cx| {
                    if let Some(library) = root.local_library.as_ref() {
                        if let Some(playlist) = library.playlists.first() {
                            if !playlist.tracks.is_empty() {
                                let tracks: Vec<TrackItem> =
                                    playlist.tracks.iter().map(|t| t.to_item()).collect();
                                root.play_from(tracks, 0, cx);
                            }
                        }
                    }
                });
            })
            .detach();
        }
        // 竞态复现：先播 A，300ms 后立刻换 B（验证 A 的装载结果会被丢弃）
        if std::env::var("SODAM_RACE_TEST").is_ok() {
            cx.spawn(async move |this, cx| {
                for _ in 0..60 {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(500))
                        .await;
                    let ready = this.update(cx, |root, _cx| {
                        root.local_library
                            .as_ref()
                            .is_some_and(|library| library.track_count() >= 2)
                    });
                    if ready.unwrap_or(false) {
                        break;
                    }
                }
                let _ = this.update(cx, |root, cx| {
                    let Some(library) = root.local_library.clone() else {
                        return;
                    };
                    let Some(playlist) = library.playlists.first() else {
                        return;
                    };
                    if playlist.tracks.len() < 2 {
                        return;
                    }
                    let picks = [0usize, 1];
                    let first: Vec<TrackItem> =
                        playlist.tracks.iter().map(|t| t.to_item()).collect();
                    root.play_from(first, picks[0], cx);
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(200))
                            .await;
                        let _ = this.update(cx, |root, cx| {
                            if let Some(library) = root.local_library.as_ref() {
                                let playlist = &library.playlists[0];
                                let second: Vec<TrackItem> =
                                    playlist.tracks.iter().map(|t| t.to_item()).collect();
                                root.play_from(second, picks[1], cx);
                            }
                        });
                    })
                    .detach();
                });
            })
            .detach();
        }
        // 开发验证用：按标题关键字自动播放某首歌（用于核对歌词/翻译等单曲界面）。
        if let Ok(needle) = std::env::var("SODAM_PLAY_NAME") {
            let needle = needle.trim().to_lowercase();
            if !needle.is_empty() {
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(200))
                            .await;
                        let ready = this.update(cx, |root, _cx| {
                            root.local_library.as_ref().is_some_and(|library| {
                                library.playlists.iter().any(|playlist| {
                                    playlist
                                        .tracks
                                        .iter()
                                        .any(|track| track.title.to_lowercase().contains(&needle))
                                })
                            })
                        });
                        if ready.unwrap_or(false) {
                            break;
                        }
                    }
                    let _ = this.update(cx, |root, cx| {
                        let Some(library) = root.local_library.clone() else {
                            return;
                        };
                        for (playlist_index, playlist) in library.playlists.iter().enumerate() {
                            let Some(track_index) = playlist
                                .tracks
                                .iter()
                                .position(|track| track.title.to_lowercase().contains(&needle))
                            else {
                                continue;
                            };
                            let tracks: Vec<TrackItem> = playlist
                                .tracks
                                .iter()
                                .map(|track| track.to_item())
                                .collect();
                            root.local_selected = playlist_index;
                            root.play_from(tracks, track_index, cx);
                            return;
                        }
                    });
                })
                .detach();
            }
        }
        if let Ok(index) = std::env::var("SODAM_JUMP_TEST") {
            let index: usize = index.trim().parse().unwrap_or(0);
            cx.spawn(async move |this, cx| {
                // 等队列非空再模拟点击
                for _ in 0..60 {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(500))
                        .await;
                    let ready = this.update(cx, |root, _cx| root.queue.len() > index);
                    if ready.unwrap_or(false) {
                        break;
                    }
                }
                let _ = this.update(cx, |root, cx| {
                    root.jump_in_queue(index, cx);
                    cx.notify();
                });
            })
            .detach();
        }
        if let Ok(value) = std::env::var("SODAM_POPUP") {
            if value.trim() == "queue" {
                root.queue_open = true;
            }
        }
        // 开发验证用：启动即展开「正在播放」页，便于截图核对歌词/大封面过渡。
        if std::env::var("SODAM_PLAYER").is_ok() {
            root.player_view_open = true;
        }
        // 开发验证用：曲库就绪后自动选中第 N 个歌单，便于截图核对切歌单过渡。
        if let Ok(value) = std::env::var("SODAM_SELECT") {
            if let Ok(index) = value.trim().parse::<usize>() {
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(200))
                            .await;
                        let ready = this.update(cx, |root, _cx| {
                            root.local_library
                                .as_ref()
                                .is_some_and(|library| library.playlists.len() > index)
                        });
                        if ready.unwrap_or(false) {
                            break;
                        }
                    }
                    // 留出截图启动时间，真机验收时能拍到过渡中间帧。
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1200))
                        .await;
                    let _ = this.update(cx, |root, cx| {
                        root.select_local_playlist(index, cx);
                    });
                })
                .detach();
            }
        }
        // 开发/截图便利：临时覆盖音乐文件夹，不写回配置。
        if let Ok(folder) = std::env::var("SODAM_LOCAL_FOLDER") {
            if !folder.trim().is_empty() {
                root.settings.local_music_folder = folder;
            }
        }
        // 选过文件夹就自动重扫：路径存在 config.json 里，重启即恢复歌单。
        if !root.settings.local_music_folder.trim().is_empty() {
            root.scan_local_library(cx);
        }
        root
    }
}

impl Root {
    pub fn tr(&self, text: &'static str) -> &'static str {
        self.language.text(text)
    }

    pub fn localized(&self, key: &'static str, args: &[String]) -> String {
        self.language.textf(key, args)
    }

    /// 控件悬停监听器：把进入/离开记进 `hovers`，只在状态真正变化时重绘。
    pub(crate) fn hover_listener(
        cx: &gpui::Context<Root>,
        key: impl std::hash::Hash + Clone + 'static,
    ) -> HoverListener {
        let entity = cx.entity();
        Box::new(move |hovered: &bool, _window, cx| {
            entity.update(cx, |root, cx| {
                if root.hovers.set(key.clone(), *hovered) {
                    cx.notify();
                }
            });
        })
    }

    /// 记录曲目文字变化：旧标题/歌手/专辑留一份，渲染时做交叉淡化。
    ///
    /// 只在显示内容真的变化时记录；同一首歌重复装载（单曲循环）不会重播动画。
    pub(crate) fn note_track_text(&mut self, title: &str, artist: &str, album: &str) {
        let next = (title.to_string(), artist.to_string(), album.to_string());
        if self.track_text_shown.as_ref() == Some(&next) {
            return;
        }
        let from = self.track_text_shown.replace(next);
        let Some((from_title, from_artist, from_album)) = from else {
            return;
        };
        self.track_text_seq = self.track_text_seq.wrapping_add(1);
        // 默认从下方进入；上一首时 next/prev 会写入 -1。
        let direction = if self.track_swap_dir.abs() > f32::EPSILON {
            self.track_swap_dir
        } else {
            1.0
        };
        self.track_swap_dir = 0.0;
        self.track_text_swap = Some(TrackTextSwap {
            from_title,
            from_artist,
            from_album,
            seq: self.track_text_seq,
            direction,
            changed_at: std::time::Instant::now(),
        });
    }

    /// 每帧同步「正在播放」曲目：高亮从旧行连续过渡到新行。
    fn sync_now_playing(&mut self, window: &mut Window) {
        let next = self
            .pending_track
            .as_ref()
            .or_else(|| self.queue.current())
            .map(|track| track.id.clone());
        self.now_playing_fade.note(next);
        if self.now_playing_fade.animating() {
            // 主题色/歌词另有动画；这里需要额外帧来推进行高亮的颜色插值。
            window.request_animation_frame();
        }
    }

    /// 播放位置的显示值：以最近一次快照为锚点，按经过时间外推到当前帧。
    ///
    /// 音频线程的位置快照约 100ms 更新一次，直接拿它画进度条会「一秒挪一下」；
    /// 这里在两次快照之间做线性外推，进度线与歌词高亮才是连续的。
    pub fn display_position_seconds(&self) -> f64 {
        let snapshot = self.engine.snapshot();
        self.position_for_snapshot(&snapshot)
    }

    pub(crate) fn position_for_snapshot(&self, snapshot: &sodam_core::PlaybackSnapshot) -> f64 {
        if snapshot.playing
            && self.pending_track.is_none()
            && self.progress_anchor_track == snapshot.track_id
        {
            let elapsed = self.progress_anchor_at.elapsed().as_secs_f64();
            let position = self.progress_anchor_seconds + elapsed;
            // 时长未知（标签没写时长）时不能把它钳成 0，否则进度和歌词永远不动。
            if snapshot.duration_seconds > 0.0 {
                position.min(snapshot.duration_seconds)
            } else {
                position
            }
        } else {
            snapshot.position_seconds
        }
    }

    /// 心跳拿到新快照时校准插值锚点；保证显示位置单调、不因快照滞后往回跳。
    pub(crate) fn observe_progress(&mut self, snapshot: &sodam_core::PlaybackSnapshot) {
        let now = std::time::Instant::now();
        if !snapshot.playing || snapshot.track_id.is_empty() {
            self.progress_anchor_at = now;
            self.progress_anchor_seconds = snapshot.position_seconds;
            self.progress_anchor_track = snapshot.track_id.clone();
            return;
        }
        if self.progress_anchor_track != snapshot.track_id {
            self.progress_anchor_track = snapshot.track_id.clone();
            self.progress_anchor_seconds = snapshot.position_seconds;
            self.progress_anchor_at = now;
            return;
        }
        let displayed =
            self.progress_anchor_seconds + self.progress_anchor_at.elapsed().as_secs_f64();
        if (snapshot.position_seconds - displayed).abs() > 0.35 {
            // seek 或快照跳变：直接对齐。
            self.progress_anchor_seconds = snapshot.position_seconds;
            self.progress_anchor_at = now;
        } else if snapshot.position_seconds > self.progress_anchor_seconds {
            // 快照追上来了：以「已经显示到的位置」和快照的较大值为新锚点，
            // 避免进度条在每次心跳时回退一小格。
            self.progress_anchor_seconds = displayed.max(snapshot.position_seconds);
            self.progress_anchor_at = now;
        }
    }

    /// 切歌/seek 后重置插值锚点。
    pub(crate) fn reset_progress_anchor(&mut self) {
        let snapshot = self.engine.snapshot();
        self.progress_anchor_at = std::time::Instant::now();
        self.progress_anchor_seconds = snapshot.position_seconds;
        self.progress_anchor_track = snapshot.track_id.clone();
    }

    pub fn set_status(&mut self, key: &'static str, args: &[String]) {
        self.set_status_text(self.localized(key, args));
    }

    /// 浅色/深色模式切换：记录目标并用渲染帧连续过渡主题色。
    pub(crate) fn toggle_light_mode(&mut self, cx: &mut Context<Self>) {
        self.settings.light_mode = !self.settings.light_mode;
        let to = if self.settings.light_mode { 1.0 } else { 0.0 };
        let from = theme::light_mix();
        self.light_mode_anim = Some((from, to, std::time::Instant::now()));
        if let Err(error) = self.settings.save() {
            let message = self.failure_text("保存主题设置失败", &error, SAVE_TEXT);
            self.set_status_text(message);
        }
        cx.notify();
    }

    fn tick_light_mode(&mut self, window: &mut Window) {
        let Some((from, to, started)) = self.light_mode_anim else {
            return;
        };
        let raw = (started.elapsed().as_secs_f32() / 0.42).clamp(0.0, 1.0);
        let eased = ui::motion::ease_in_out_cubic(raw);
        theme::set_light_mix(from + (to - from) * eased);
        if raw >= 1.0 {
            self.light_mode_anim = None;
        } else {
            window.request_animation_frame();
        }
    }

    /// 随设置项（总开关/全局应用）或当前页切入切出，平滑淡入/淡出流光与磨砂感。
    fn tick_flow_mix(&mut self, window: &mut Window) {
        let target = if self.settings.flowing_background
            && (self.settings.flowing_background_global || self.player_view_open)
        {
            1.0
        } else {
            0.0
        };
        if (self.flow_target - target).abs() > f32::EPSILON {
            self.flow_target = target;
            self.flow_mix_anim = Some((theme::flow_mix(), target, std::time::Instant::now()));
        }
        let Some((from, to, started)) = self.flow_mix_anim else {
            return;
        };
        let raw = (started.elapsed().as_secs_f32() / 0.42).clamp(0.0, 1.0);
        let eased = ui::motion::ease_in_out_cubic(raw);
        theme::set_flow_mix(from + (to - from) * eased);
        if raw >= 1.0 {
            self.flow_mix_anim = None;
        } else {
            window.request_animation_frame();
        }
    }

    /// 任务栏快捷播控开关：立刻起停宿主，并写入配置。
    pub(crate) fn toggle_taskbar_player(&mut self, cx: &mut Context<Self>) {
        self.settings.taskbar_player = !self.settings.taskbar_player;
        let enabled = self.settings.taskbar_player;
        let root = cx.entity();
        crate::taskbar_player::set_enabled(cx, root, enabled);
        match self.settings.save() {
            Ok(()) => {
                let text = if enabled {
                    self.tr("任务栏播放控制已开启")
                } else {
                    self.tr("任务栏播放控制已关闭")
                };
                self.set_status_text(text);
            }
            Err(error) => {
                let message = self.failure_text(
                    "保存任务栏播放控制失败",
                    &error,
                    "任务栏播放控制设置没有写入",
                );
                self.set_status_text(message);
            }
        }
        cx.notify();
    }

    /// 普通操作反馈：只进入底部状态行，不触发全局 message。
    ///
    /// 只写状态，不负责重绘——调用方照常 `cx.notify()`（现有写入点都在用户动作或
    /// 后台回调里，本身就会跟着重绘）。
    pub fn set_status_text(&mut self, text: impl Into<String>) {
        self.remember_status(text);
    }

    /// 写入状态行。
    fn remember_status(&mut self, text: impl Into<String>) -> String {
        let text = text.into();
        self.status_line.set(text.clone());
        text
    }

    /// 技术原文的唯一去处：诊断日志（默认开，写在本机日志目录）。
    /// 界面文案一律不用它——用户只看人话。
    ///
    /// 话题是 `app`：设置保存、文件选择这类非播放链路的失败都归这里。
    /// 播放链路的失败另有 `player` 话题的装载轨迹，不靠这条。
    pub(crate) fn log_failure(&self, context: &str, error: impl std::fmt::Display) {
        let detail = error.to_string();
        log_event(
            LogLevel::Error,
            LogTopic::App,
            "failure",
            &[("context", context), ("detail", detail.as_str())],
        );
    }

    /// 记录技术原文，并返回给用户看的那一句。
    ///
    /// 界面文案一律走这里，不再复述 `{err}`：普通用户看 `os error 32`
    /// 得不到任何行动信息，而它也不该被藏进折叠区——原文的去处是日志。
    pub(crate) fn failure_text(
        &self,
        context: &str,
        error: impl std::fmt::Display,
        text: &'static str,
    ) -> String {
        self.log_failure(context, error);
        self.tr(text).to_string()
    }

    /// 同一来源的警告只保留一条。
    pub fn notify_warning_once(&mut self, key: &str, text: impl Into<String>) -> u64 {
        self.notify_message(ui::message::MessageKind::Warning, text.into(), Some(key))
    }

    /// 同一来源的重复错误替换旧提示，避免轮询/重试堆成消息墙。
    pub fn notify_error_once(&mut self, key: &str, text: impl Into<String>) -> u64 {
        self.notify_message(ui::message::MessageKind::Error, text.into(), Some(key))
    }

    fn notify_message(
        &mut self,
        kind: ui::message::MessageKind,
        text: String,
        key: Option<&str>,
    ) -> u64 {
        // 两条通道拿同一句文案：轻提示 3 秒就走，状态行留到 5 秒。
        let message = self.remember_status(text);
        self.push_message_with_key(kind, message, Some(ui::message::DEFAULT_DURATION), key)
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Windows 自绘标题栏（系统标题栏被 WM_NCCALCSIZE 去掉）；macOS 用系统红绿灯。
        // Linux 交给配置项：默认 auto 跟随桌面协商结果。
        let own_titlebar = if cfg!(target_os = "windows") {
            true
        } else if cfg!(target_os = "macos") {
            false
        } else {
            sodam_core::config::TitlebarMode::from_key(&self.settings.titlebar).draws_titlebar(
                matches!(window.window_decorations(), Decorations::Client { .. }),
            )
        };
        // 没有元素聚焦时把焦点收回根元素：gpui 的上下文栈取自聚焦节点的路径，
        // 空栈会让带谓词的快捷键绑定（播放三键）全部失效。
        if window.focused(cx).is_none() {
            window.focus(&self.workspace_focus, cx);
        }
        // 开发期帧耗时探针（`SODAM_FRAME_LOG=1` 把 app 话题提到 DEBUG）：渲染超过 16ms 就打一行。
        let frame_started = std::time::Instant::now();
        // 颜色过渡和歌词高亮都按显示刷新率推进：渲染一帧就前进一步，
        // 不再用 16ms 定时器把动画限制在 60fps。
        self.tick_palette(window);
        self.tick_light_mode(window);
        self.tick_flow_mix(window);
        self.step_scroll_anims(window);
        self.sync_now_playing(window);
        if self.player_view_open {
            self.sync_lyrics_position(cx);
            self.tick_lyrics_center(window, cx);
            if let Some((at, _)) = self.lyrics_scroll_stagger {
                if std::time::Instant::now().duration_since(at).as_secs_f32() >= 0.85 {
                    self.lyrics_scroll_stagger = None;
                } else {
                    window.request_animation_frame();
                }
            }
        }
        let playing = self.engine.snapshot().playing;
        let now = std::time::Instant::now();
        let flow_phase = self.flow_clock.advance(
            now,
            playing && self.settings.flowing_background && theme::flow_mix() > 0.001,
        );
        if playing {
            // 进度条/歌词的插值位置每帧都在变，播放期间持续请求下一帧。
            window.request_animation_frame();
        }
        let element = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            // 根元素持焦点 + 挂基础上下文：带谓词的快捷键绑定要有非空上下文栈才生效。
            .track_focus(&self.workspace_focus)
            .key_context(crate::shortcuts::BASE_CONTEXT)
            .text_color(ui::theme::text())
            .on_key_down(
                cx.listener(|root, event: &gpui::KeyDownEvent, _window, cx| {
                    if event.keystroke.key == "escape" && root.player_view_open {
                        root.close_player_view(cx);
                    }
                }),
            )
            // 全窗背景：多色流光光斑与基础底色，所有透明磨砂面板都叠在它上面。
            .child(ui::backdrop::render(flow_phase))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(ui::sidebar::render(self, window, cx, own_titlebar))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            // macOS：让红绿灯悬浮在内容之上，内容整体下移 28px。
                            .when(cfg!(target_os = "macos"), |this| this.pt(px(28.0)))
                            .child(views::render(self, window, cx, own_titlebar)),
                    ),
            )
            // 队列是覆盖在内容之上的 420 宽侧板（汽水音乐的 side-panel），
            // 不挤压页面；贴右边缘，底部让出播放栏。
            // 关闭时先保留面板播完滑出动画，再卸载（queue_closing）。
            .when(self.queue_open || self.queue_closing, |this| {
                let closing = self.queue_closing && !self.queue_open;
                let sequence = self.queue_anim_seq;
                // `right` 是距父容器右边缘的距离：正值向左、负值向右。
                // 打开：-宽度 → 0（从右侧滑入）；关闭：0 → -宽度（向右侧滑出）。
                let width = ui::player_bar::QUEUE_WIDTH;
                let (from, to) = if closing {
                    (0.0, -width)
                } else {
                    (-width, 0.0)
                };
                let duration = if closing {
                    ui::motion::duration::DRAWER_CLOSE
                } else {
                    ui::motion::duration::DRAWER_OPEN
                };
                // 只在开合动画窗口内挂 `with_animation`。窗口外直接落最终位置：
                // 后续任何重绘（扫描完成、色板过渡）都不会让动画状态重来一次，
                // 面板不会突然回到屏外。
                let animating = self.queue_anim_at.is_some_and(|at| {
                    at.elapsed() < duration + std::time::Duration::from_millis(80)
                });
                let panel = div()
                    .id("queue-panel")
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(theme::PLAYER_H))
                    .right(px(to))
                    .w(px(ui::player_bar::QUEUE_WIDTH))
                    // Windows 的原生标题栏命中也必须停在侧板这一层。
                    .when(cfg!(target_os = "windows"), |panel| panel.occlude())
                    .when(!cfg!(target_os = "windows"), |panel| {
                        panel.block_mouse_except_scroll()
                    })
                    .child(ui::player_bar::queue_drawer(self, cx));
                let panel = if animating {
                    panel
                        .with_animation(
                            gpui::ElementId::Name(format!("queue-slide-{sequence}").into()),
                            ui::motion::standard(duration),
                            move |this, delta| this.right(px(from + (to - from) * delta)),
                        )
                        .into_any_element()
                } else {
                    panel.into_any_element()
                };
                this.child(panel)
            })
            .child(ui::player_bar::render(self, cx))
            // 普通操作反馈的固定出口：播放栏上方的一行状态。绝对定位，出现与消失
            // 都不推挤页面；层级在弹层之下，所以菜单、遮罩、message 都压得住它。
            .when_some(ui::status_line::render(self, cx), |this, status| {
                this.child(status)
            })
            .when_some(ui::message::render(self, cx), |this, message| {
                this.child(message)
            })
            .when(self.queue_menu.is_some(), |this| {
                // 透明遮罩：点任何地方都关掉右键菜单；
                // 菜单本身层级更高（priority 2 > 1），点菜单不会走到这里。
                this.child(
                    gpui::deferred(
                        div()
                            .id("menu-scrim")
                            .absolute()
                            .top(px(0.0))
                            .left(px(0.0))
                            .size_full()
                            .occlude()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|root, _event, _window, cx| {
                                    root.close_queue_menu(cx);
                                }),
                            ),
                    )
                    .with_priority(1),
                )
            });
        if log_outlet::enabled(LogLevel::Debug, LogTopic::App) {
            let elapsed = frame_started.elapsed().as_millis();
            // 240Hz 的预算是 4.2ms；6ms 已经意味着掉到 166fps 以下。
            if elapsed > 6 {
                let rows = self
                    .local_library
                    .as_ref()
                    .map(|library| library.track_count())
                    .unwrap_or(0);
                let elapsed_ms = elapsed.to_string();
                let rows = rows.to_string();
                log_event(
                    LogLevel::Debug,
                    LogTopic::App,
                    "frame_slow",
                    &[("elapsed_ms", elapsed_ms.as_str()), ("rows", rows.as_str())],
                );
            }
        }
        element
    }
}

#[cfg(test)]
mod tests {
    use super::{RowAnim, SidebarRowAnim};
    use std::time::{Duration, Instant};

    #[test]
    fn row_anim_settles_and_next_retarget_starts_from_endpoint() {
        let now = Instant::now();
        let mut anim = RowAnim::new(0.0);
        anim.retarget(0.07, Duration::from_millis(160), now);
        assert!(!anim.settle(now + Duration::from_millis(80)));
        assert!(anim.settle(now + Duration::from_millis(200)));
        assert!((anim.from - 0.07).abs() < f32::EPSILON);
        assert!(anim.duration.is_zero());

        anim.retarget(
            0.0,
            Duration::from_millis(180),
            now + Duration::from_millis(200),
        );
        assert!((anim.from - 0.07).abs() < f32::EPSILON);
        assert_eq!(anim.to, 0.0);
    }

    #[test]
    fn row_hover_leave_always_clears_background() {
        let now = Instant::now();
        let mut row = SidebarRowAnim::new(false, false);
        assert!(row.set_hovered(true, false, now));
        assert_eq!(row.bg.to, 0.07);
        // 指针离开的另一行先报「进入」也不影响：这一行自己收到离开就清掉。
        assert!(row.set_hovered(false, false, now));
        assert_eq!(row.bg.to, 0.0);
        assert!(!row.set_hovered(false, false, now));

        // 选中行悬停进出只记状态，背景保持 13% 白。
        let mut selected = SidebarRowAnim::new(true, false);
        selected.set_hovered(true, true, now);
        selected.set_hovered(false, true, now);
        assert_eq!(selected.bg.to, 0.13);
    }
}
