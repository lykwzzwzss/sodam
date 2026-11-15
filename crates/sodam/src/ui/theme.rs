//! 主题：颜色、尺寸、字号、圆角、图标尺寸。
//!
//! 视觉基调对齐汽水音乐桌面端：固定深色、低对比的磨砂面板、封面取色驱动的
//! 整页氛围、大圆角与大号控件。颜色分两类：
//!
//! 1. **结构色**（文字/描边/悬停）是固定的白色透明度阶梯；
//! 2. **主题色**来自当前封面（`Palette`）的 9 阶色板，铺满整页背景，
//!    并让侧栏/播放栏以半透明磨砂的形式透出这层颜色。
//!
//! 所有尺寸从同一组常量派生，页面里不再散落 magic number。

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use gpui::{linear_color_stop, linear_gradient, px, Background, Pixels, Rgba};
use sodam_core::artwork::Palette;

pub const fn hex(value: u32) -> Rgba {
    Rgba {
        r: ((value >> 16) & 0xFF) as f32 / 255.0,
        g: ((value >> 8) & 0xFF) as f32 / 255.0,
        b: (value & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a,
    }
}

/// 封面主题色（bit31 表示存在；低 24 位 RGB）。
static ACCENT_COLOR: AtomicU32 = AtomicU32::new(0);
/// 封面环境色（压暗后的主色，用于背景深处的第二层色场）。
static AMBIENT_COLOR: AtomicU32 = AtomicU32::new(0);
/// 封面占比最大的颜色：和强调色错开，让整页不是一个单色块。
static DOMINANT_COLOR: AtomicU32 = AtomicU32::new(0);
/// 封面是否有彩色；无彩色时界面用品牌绿。
static ACCENT_COLORFUL: AtomicU8 = AtomicU8::new(0);
/// 9 阶主题色（base-1..9）；bit31 表示这一阶有值。
static BASE_COLORS: [AtomicU32; 9] = [const { AtomicU32::new(0) }; 9];

/// 汽水音乐的品牌绿：没有封面 / 灰白封面时的强调色。
pub const BRAND_GREEN: u32 = 0x00CB64;

/// 把当前封面的调色板写进全局主题（渲染线程调用）。
///
/// `None` 表示没有封面：强调色回到品牌绿，base 色板写入深色中性板
/// （而不是清零）——页面/面板始终从 base 取色，清零会造成切换时闪白。
pub fn set_ambient_palette(palette: Option<Palette>) {
    match palette {
        Some(palette) => {
            ACCENT_COLOR.store(pack_rgb(palette.accent), Ordering::Relaxed);
            AMBIENT_COLOR.store(pack_rgb(palette.ambient), Ordering::Relaxed);
            DOMINANT_COLOR.store(pack_rgb(palette.dominant), Ordering::Relaxed);
            ACCENT_COLORFUL.store(u8::from(palette.colorful), Ordering::Relaxed);
            for (slot, color) in BASE_COLORS.iter().zip(palette.base.iter()) {
                slot.store(pack_rgb(*color), Ordering::Relaxed);
            }
        }
        None => {
            ACCENT_COLOR.store(0, Ordering::Relaxed);
            AMBIENT_COLOR.store(0, Ordering::Relaxed);
            DOMINANT_COLOR.store(0, Ordering::Relaxed);
            ACCENT_COLORFUL.store(0, Ordering::Relaxed);
            seed_neutral_base();
        }
    }
}

/// 写入深色中性 9 阶色板（无封面 / 灰阶封面的页面与面板底色）。
fn seed_neutral_base() {
    for (slot, color) in BASE_COLORS.iter().zip(Palette::NEUTRAL_DARK.base.iter()) {
        slot.store(pack_rgb(*color), Ordering::Relaxed);
    }
}

fn pack_rgb(rgb: [u8; 3]) -> u32 {
    0x8000_0000 | (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2])
}

fn unpack_rgb(packed: u32) -> Option<Rgba> {
    if packed & 0x8000_0000 == 0 {
        return None;
    }
    Some(rgba(
        ((packed >> 16) & 0xFF) as u8,
        ((packed >> 8) & 0xFF) as u8,
        (packed & 0xFF) as u8,
        1.0,
    ))
}

fn accent_seed() -> Option<Rgba> {
    unpack_rgb(ACCENT_COLOR.load(Ordering::Relaxed))
}

fn ambient_seed() -> Option<Rgba> {
    unpack_rgb(AMBIENT_COLOR.load(Ordering::Relaxed))
}

fn dominant_seed() -> Option<Rgba> {
    unpack_rgb(DOMINANT_COLOR.load(Ordering::Relaxed))
}

fn with_alpha(mut color: Rgba, alpha: f32) -> Rgba {
    color.a = alpha;
    color
}

// ---------------------------------------------------------------------------
// 结构色
// ---------------------------------------------------------------------------

/// 侧栏、播放栏的半透明磨砂底：叠在整页色场上，换封面时颜色连续。
pub fn chrome() -> Rgba {
    with_alpha(base(6), 0.62)
}

/// 9 阶主题色（0 = base-1，8 = base-9）。没有封面时返回深色中性板。
pub fn base(index: usize) -> Rgba {
    let index = index.min(8);
    unpack_rgb(BASE_COLORS[index].load(Ordering::Relaxed)).unwrap_or_else(|| {
        let color = Palette::NEUTRAL_DARK.base[index];
        rgba(color[0], color[1], color[2], 1.0)
    })
}

/// 是否有封面彩色主题。
pub fn colorful() -> bool {
    ACCENT_COLORFUL.load(Ordering::Relaxed) != 0
}

/// 播放页/内容区背景：汽水同款 base-5 → base-7 纵向渐变。
pub fn page_background() -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(base(4), 0.0),
        linear_color_stop(base(6), 1.0),
    )
}

/// 进度条填充：汽水音乐深色页用 base-3；若 base-3 过暗（例如黑底+局部彩色主体封面
/// 导致 base 阶梯退化为中性深灰），则回退到活力强调色 `accent()` 保证可读性。
pub fn progress_fill() -> Rgba {
    if colorful() {
        let fill = base(2);
        let luminance = 0.2126 * fill.r + 0.7152 * fill.g + 0.0722 * fill.b;
        if luminance >= 0.28 {
            return fill;
        }
    }
    accent()
}

/// 抬升面：搜索框、卡片、播放栏按钮。
pub fn surface_elevated() -> Rgba {
    rgba(255, 255, 255, 0.07)
}

/// 行悬停。
pub fn surface_hover() -> Rgba {
    rgba(255, 255, 255, 0.09)
}

/// 选中行。
pub fn surface_selected() -> Rgba {
    rgba(255, 255, 255, 0.13)
}

/// 1px 描边：只在需要分隔时用，颜色很轻。
pub fn border() -> Rgba {
    rgba(255, 255, 255, 0.08)
}

/// 强一点的描边（浮层、菜单）。
pub fn border_strong() -> Rgba {
    rgba(255, 255, 255, 0.13)
}

/// 玻璃面板上的高光边（顶部 1px），模仿亚克力的折射线。
pub fn glass_highlight() -> Rgba {
    rgba(255, 255, 255, 0.10)
}

/// 磨砂侧板/菜单的底色（半透明，配合 backdrop_blur 使用）。
pub fn glass_panel() -> Rgba {
    rgba(18, 18, 20, 0.72)
}

/// 主文本。
pub fn text() -> Rgba {
    rgba(255, 255, 255, 0.94)
}

/// 次要文本。
pub fn text_muted() -> Rgba {
    rgba(255, 255, 255, 0.63)
}

/// 三级文本：序号、时长、占位符。
pub fn text_faint() -> Rgba {
    rgba(255, 255, 255, 0.38)
}

/// 歌词译文（非当前行）：比正文更暗，只提供理解辅助。
pub fn lyrics_translation() -> Rgba {
    rgba(255, 255, 255, 0.36)
}

/// 歌词译文（当前行）：跟随换行柔和变亮，但不追平英文正文。
pub fn lyrics_translation_active() -> Rgba {
    rgba(255, 255, 255, 0.70)
}

/// 歌词译文（距离当前行较远）：继续压暗，避免整屏译文抢视线。
pub fn lyrics_translation_faint() -> Rgba {
    rgba(255, 255, 255, 0.28)
}

// ---------------------------------------------------------------------------
// 主题色
// ---------------------------------------------------------------------------

/// 强调色：有彩色封面时用封面活力色，否则用品牌绿。
pub fn accent() -> Rgba {
    if ACCENT_COLORFUL.load(Ordering::Relaxed) == 0 {
        return hex(BRAND_GREEN);
    }
    accent_seed().unwrap_or_else(|| hex(BRAND_GREEN))
}

/// 警示色。
pub fn warning() -> Rgba {
    hex(0xFFB454)
}

/// 错误色。
pub fn danger() -> Rgba {
    hex(0xFF6B70)
}

/// 封面主色的柔光：把最抢眼的颜色铺到右上角。
pub fn backdrop_glow() -> Background {
    let glow = accent_seed().unwrap_or_else(|| hex(BRAND_GREEN));
    linear_gradient(
        135.0,
        linear_color_stop(with_alpha(glow, 0.20), 0.0),
        linear_color_stop(with_alpha(glow, 0.0), 0.72),
    )
}

/// 封面环境色的另一层柔光：左下角更沉，和强调色错开。
pub fn backdrop_ambient_glow() -> Background {
    let glow = ambient_seed()
        .or_else(dominant_seed)
        .unwrap_or_else(|| base(7));
    linear_gradient(
        315.0,
        linear_color_stop(with_alpha(glow, 0.40), 0.0),
        linear_color_stop(with_alpha(glow, 0.0), 0.66),
    )
}

/// 色场之上的暗化渐变：上方轻、下方重，保证播放栏文字可读。
pub fn backdrop_scrim() -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(rgba(5, 5, 7, 0.04), 0.0),
        linear_color_stop(rgba(4, 4, 6, 0.44), 1.0),
    )
}

// ---------------------------------------------------------------------------
// 尺寸
// ---------------------------------------------------------------------------

/// 侧边栏宽度（对齐汽水音乐实机 1430 逻辑宽窗口下的视觉比例，约 18%）。
pub const SIDEBAR_W: f32 = 256.0;
/// 顶部工具条高度。
pub const TOOLBAR_H: f32 = 60.0;
/// 底部播放栏高度（汽水音乐 88px × 1.25 视觉缩放）。
pub const PLAYER_H: f32 = 112.0;
/// 底部状态行的最大宽度。
pub const STATUS_LINE_MAX_W: f32 = 520.0;
/// 播放页大封面边长。
pub const HERO_COVER: f32 = 360.0;

/// 图标尺寸：导航 20，播放控制 20~26。
pub const ICON_SM: f32 = 16.0;
pub const ICON: f32 = 20.0;
pub const ICON_LG: f32 = 26.0;
pub const ICON_XL: f32 = 32.0;

/// 间距梯度：4 / 8 / 12 / 16 / 24 / 32。
pub mod space {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// 行高与控件尺寸。
pub mod size {
    /// 侧栏导航行。
    pub const NAV_ROW: f32 = 44.0;
    /// 列表行（歌曲）。
    pub const LIST_ROW: f32 = 56.0;
    /// 常规按钮 / 输入框。
    pub const CONTROL: f32 = 36.0;
    /// 大按钮（播放全部）。
    pub const CONTROL_LG: f32 = 48.0;
    /// 播放/暂停胶囊（实机约 99×57 逻辑像素）。
    pub const PLAY_BTN_W: f32 = 100.0;
    pub const PLAY_BTN_H: f32 = 56.0;
    /// 播放栏封面。
    pub const PLAYER_THUMB: f32 = 68.0;
    /// 列表行封面。
    pub const ROW_ART: f32 = 44.0;
}

/// 圆角梯度：8 / 12 / 16 / 999。
pub mod radius {
    pub const ART: f32 = 8.0;
    pub const ROW: f32 = 8.0;
    pub const CARD: f32 = 12.0;
    pub const PILL: f32 = 999.0;
}

/// 字号梯度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    Tiny,
    Small,
    Body,
    Large,
    Title,
    Hero,
}

impl Text {
    pub fn size(self) -> Pixels {
        px(match self {
            Text::Tiny => 11.0,
            Text::Small => 13.0,
            Text::Body => 15.0,
            Text::Large => 18.0,
            Text::Title => 26.0,
            Text::Hero => 32.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 主题色是进程级全局状态；这两个测试会互相覆盖，必须串行。
    static THEME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn accent_falls_back_to_brand_green_without_cover() {
        let _guard = THEME_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        set_ambient_palette(None);
        let accent = accent();
        assert!((accent.g - 0.796).abs() < 0.01, "无封面时应是品牌绿");
    }

    #[test]
    fn colorful_palette_drives_accent() {
        let _guard = THEME_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        set_ambient_palette(Some(Palette {
            accent: [210, 110, 90],
            dominant: [30, 20, 20],
            ambient: [50, 10, 8],
            base: [
                [240, 220, 214],
                [210, 180, 174],
                [180, 140, 132],
                [140, 96, 90],
                [80, 50, 46],
                [64, 40, 37],
                [50, 30, 28],
                [34, 20, 19],
                [22, 12, 12],
            ],
            colorful: true,
        }));
        let accent = accent();
        assert!(accent.r > accent.g && accent.r > accent.b);
        set_ambient_palette(None);
    }
}
