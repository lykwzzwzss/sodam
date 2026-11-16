//! 主题：颜色、尺寸、字号、圆角、图标尺寸。
//!
//! 深色主题用封面取色铺开背景，浅色主题以中性色为底，保留少量封面色。
//! 磨砂面板、大圆角和大号控件沿用同一套尺寸。颜色分两类：
//!
//! 1. **结构色**用于文字、描边和交互状态，随深浅主题过渡；
//! 2. **主题色**来自当前封面（`Palette`）的 9 阶色板，用于背景和强调色。
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
/// 封面是否有彩色；无彩色时界面用品牌绿。
static ACCENT_COLORFUL: AtomicU8 = AtomicU8::new(0);
/// 浅色模式插值系数（0.0 = 深色，1.0 = 浅色），存 f32::to_bits。
static LIGHT_MIX_BITS: AtomicU32 = AtomicU32::new(0);
/// 流光展示插值系数（0.0 = 静态氛围底色，1.0 = 流动光斑与磨砂通透感）。
static FLOW_MIX_BITS: AtomicU32 = AtomicU32::new(0x3F80_0000);
/// 设置浅色模式插值系数（0.0..=1.0），用于深浅切换动画逐帧过渡。
pub fn set_light_mix(mix: f32) {
    LIGHT_MIX_BITS.store(mix.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

/// 当前浅色模式插值系数（0.0 = 深色，1.0 = 浅色）。
pub fn light_mix() -> f32 {
    f32::from_bits(LIGHT_MIX_BITS.load(Ordering::Relaxed)).clamp(0.0, 1.0)
}

/// 设置流光可见强度（0.0..=1.0），用于开关流光或切换页面时平滑淡入淡出。
pub fn set_flow_mix(mix: f32) {
    FLOW_MIX_BITS.store(mix.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

pub fn flow_mix() -> f32 {
    f32::from_bits(FLOW_MIX_BITS.load(Ordering::Relaxed)).clamp(0.0, 1.0)
}

fn lerp_rgba(dark: Rgba, light: Rgba) -> Rgba {
    let t = light_mix();
    if t <= 0.0 {
        return dark;
    }
    if t >= 1.0 {
        return light;
    }
    super::motion::mix_rgba(dark, light, t)
}
/// 9 阶主题色（base-1..9）；bit31 表示这一阶有值。
static BASE_COLORS: [AtomicU32; 9] = [const { AtomicU32::new(0) }; 9];
/// 4 组封面多色流光色；bit31 表示有效。
static GLOW_COLORS: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];

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
            ACCENT_COLORFUL.store(u8::from(palette.colorful), Ordering::Relaxed);
            for (slot, color) in BASE_COLORS.iter().zip(palette.base.iter()) {
                slot.store(pack_rgb(*color), Ordering::Relaxed);
            }
            for (slot, color) in GLOW_COLORS.iter().zip(palette.glow.iter()) {
                slot.store(pack_rgb(*color), Ordering::Relaxed);
            }
        }
        None => {
            ACCENT_COLOR.store(0, Ordering::Relaxed);
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
    for (slot, color) in GLOW_COLORS.iter().zip(Palette::NEUTRAL_DARK.glow.iter()) {
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

fn with_alpha(mut color: Rgba, alpha: f32) -> Rgba {
    color.a = alpha;
    color
}

// ---------------------------------------------------------------------------
// 结构色
// ---------------------------------------------------------------------------

/// 侧栏底色：流光开启时保持细腻通透磨砂感，关闭时回到沉稳的静态底色。
pub fn chrome() -> Rgba {
    let flow = flow_mix();
    let dark = super::motion::mix_rgba(
        with_alpha(base(7), 0.84),
        super::motion::mix_rgba(rgba(10, 11, 16, 0.44), with_alpha(base(7), 0.44), 0.25),
        flow,
    );
    let light_static = light_surface(0xF1F3F6, 0.16);
    let light_flow = with_alpha(light_surface(0xF6F7FA, 0.08), 0.66);
    lerp_rgba(
        dark,
        super::motion::mix_rgba(light_static, light_flow, flow),
    )
}

pub fn toolbar() -> Rgba {
    let flow = flow_mix();
    let dark = super::motion::mix_rgba(rgba(12, 13, 18, 0.72), rgba(8, 9, 13, 0.28), flow);
    let light_static = light_surface(0xF7F8FA, 0.10);
    let light_flow = with_alpha(light_surface(0xFAFBFD, 0.06), 0.58);
    lerp_rgba(
        dark,
        super::motion::mix_rgba(light_static, light_flow, flow),
    )
}

pub fn player_chrome() -> Rgba {
    let flow = flow_mix();
    let dark = super::motion::mix_rgba(
        with_alpha(base(8), 0.88),
        super::motion::mix_rgba(rgba(10, 11, 16, 0.56), with_alpha(base(8), 0.56), 0.25),
        flow,
    );
    let light_static = light_surface(0xFAFBFD, 0.08);
    let light_flow = with_alpha(light_surface(0xFCFDFE, 0.05), 0.72);
    lerp_rgba(
        dark,
        super::motion::mix_rgba(light_static, light_flow, flow),
    )
}

/// 设置分组卡片、提示容器等的磨砂表面，让全局流光自然透出同时保证文本清晰可读。
pub fn card_surface() -> Rgba {
    let flow = flow_mix();
    let dark = super::motion::mix_rgba(with_alpha(base(7), 0.76), rgba(16, 18, 24, 0.48), flow);
    let light_static = light_surface(0xFFFFFF, 0.04);
    let light_flow = rgba(255, 255, 255, 0.66);
    lerp_rgba(
        dark,
        super::motion::mix_rgba(light_static, light_flow, flow),
    )
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

/// 取封面主色的通道差，染到浅色底上。扣掉中性色板自带的微弱蓝偏。
fn light_surface(neutral: u32, strength: f32) -> Rgba {
    let mut surface = hex(neutral);
    let seed = base(2);
    let peak = seed.r.max(seed.g).max(seed.b);
    let tint = |channel: f32| (peak - channel - 4.0 / 255.0).clamp(0.0, 0.40) * strength;
    surface.r -= tint(seed.r);
    surface.g -= tint(seed.g);
    surface.b -= tint(seed.b);
    surface
}

/// 深色页以深邃曜石底衬托多色封面光斑（避免过亮发泥）；
/// 关闭流光时保留温和的封面深阶渐变；浅色页保持明净微暖白。
pub fn page_background() -> Background {
    let flow = flow_mix();
    let dark_flow_top = super::motion::mix_rgba(hex(0x0D0E14), base(7), 0.34);
    let dark_flow_bottom = super::motion::mix_rgba(hex(0x08090D), base(8), 0.28);
    let dark_static_top = super::motion::mix_rgba(hex(0x121319), base(5), 0.56);
    let dark_static_bottom = super::motion::mix_rgba(hex(0x0B0C10), base(7), 0.50);
    let dark_top = super::motion::mix_rgba(dark_static_top, dark_flow_top, flow);
    let dark_bottom = super::motion::mix_rgba(dark_static_bottom, dark_flow_bottom, flow);
    linear_gradient(
        180.0,
        linear_color_stop(lerp_rgba(dark_top, light_surface(0xF9FAFC, 0.14)), 0.0),
        linear_color_stop(lerp_rgba(dark_bottom, light_surface(0xF3F5F9, 0.06)), 1.0),
    )
}

/// 进度条填充：深色页用 base-3（过暗时回退到 accent）；浅色页用 accent（过亮时压暗）。
pub fn progress_fill() -> Rgba {
    let fill = base(2);
    let luminance = 0.2126 * fill.r + 0.7152 * fill.g + 0.0722 * fill.b;
    if colorful() && luminance >= 0.28 {
        lerp_rgba(fill, light_accent(accent_color()))
    } else {
        accent()
    }
}

/// 抬升面：搜索框、卡片、播放栏按钮。
pub fn surface_elevated() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.07), rgba(49, 66, 91, 0.055))
}

/// 行悬停。
pub fn surface_hover() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.09), rgba(49, 66, 91, 0.07))
}

/// 选中行。
pub fn surface_selected() -> Rgba {
    selection_tint(0.13)
}

/// 开关关闭态轨道底色：浅色/深色模式下与纯白滑块都有清晰区分度。
pub fn switch_off_track() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.16), rgba(15, 23, 42, 0.22))
}

pub fn selection_tint(alpha: f32) -> Rgba {
    lerp_rgba(rgba(255, 255, 255, alpha), rgba(54, 76, 108, alpha * 0.75))
}

pub fn play_button() -> Background {
    let dark = rgba(255, 255, 255, 0.085);
    linear_gradient(
        180.0,
        linear_color_stop(lerp_rgba(dark, hex(0xFFFFFF)), 0.0),
        linear_color_stop(lerp_rgba(dark, hex(0xF5F7FA)), 1.0),
    )
}

pub fn play_button_border() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.10), rgba(43, 70, 104, 0.15))
}

pub fn play_button_hover() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.14), rgba(46, 70, 103, 0.08))
}

pub fn control_emphasis_text() -> Rgba {
    lerp_rgba(hex(0xFFFFFF), hex(0x243448))
}

pub fn caption_hover() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.06), rgba(46, 70, 103, 0.10))
}

pub fn playlist_action() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.94), hex(0x1B2330))
}

pub fn playlist_action_hover() -> Rgba {
    lerp_rgba(hex(0xFFFFFF), hex(0x283345))
}

pub fn playlist_action_text() -> Rgba {
    lerp_rgba(hex(0x121212), hex(0xFFFFFF))
}

/// 1px 描边：只在需要分隔时用，颜色很轻。
pub fn border() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.08), rgba(51, 73, 101, 0.10))
}

/// 强一点的描边（浮层、菜单）。
pub fn border_strong() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.13), rgba(0, 0, 0, 0.13))
}

/// 玻璃面板上的高光边（顶部 1px），模仿亚克力的折射线。
pub fn glass_highlight() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.10), rgba(255, 255, 255, 0.75))
}

/// 磨砂侧板/菜单的底色（半透明，配合 backdrop_blur 使用）。
pub fn glass_panel() -> Rgba {
    lerp_rgba(rgba(18, 18, 20, 0.72), rgba(250, 251, 253, 0.86))
}

/// 主文本。
pub fn text() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.94), rgba(18, 18, 22, 0.92))
}

/// 次要文本。
pub fn text_muted() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.63), hex(0x46515E))
}

/// 三级文本：序号、时长、占位符。
pub fn text_faint() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.38), hex(0x596372))
}

/// 当前行中尚未唱到的字。
pub fn lyrics_pending() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.56), rgba(22, 28, 38, 0.48))
}

/// 非当前歌词行的主文字颜色：比当前行未唱部分更柔和，保证「正在演唱」始终是视觉中心。
pub fn lyrics_inactive() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.40), rgba(22, 28, 38, 0.42))
}

/// 当前行中已唱到的字：使用封面活力色作为强调色，并在深/浅底色下分别做亮度约束，
/// 保证在黑白或高饱和封面、深浅模式下都与未唱部分及背景有清晰区分度。
pub fn lyrics_sung() -> Rgba {
    let seed = accent_color();
    lerp_rgba(dark_lyrics_accent(seed), light_lyrics_accent(seed))
}

fn relative_luminance(c: Rgba) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

/// 深色底下的歌词高亮色：若封面主色过暗，向亮部提亮以保证清晰可读。
fn dark_lyrics_accent(seed: Rgba) -> Rgba {
    let lum = relative_luminance(seed);
    if lum >= 0.48 {
        return seed;
    }
    let lift = ((0.48 - lum) / (1.0 - lum).max(0.01)).clamp(0.0, 0.82);
    Rgba {
        r: seed.r + (1.0 - seed.r) * lift,
        g: seed.g + (1.0 - seed.g) * lift,
        b: seed.b + (1.0 - seed.b) * lift,
        a: 1.0,
    }
}

/// 浅色底下的歌词高亮色：若封面主色过亮（如亮黄/浅青），压暗以保证与浅底和未唱灰字的对比度。
fn light_lyrics_accent(seed: Rgba) -> Rgba {
    let lum = relative_luminance(seed);
    let max_lum = 0.26;
    let scale = if lum > max_lum {
        (max_lum / lum).clamp(0.24, 0.72)
    } else {
        0.72
    };
    Rgba {
        r: seed.r * scale,
        g: seed.g * scale,
        b: seed.b * scale,
        a: 1.0,
    }
}

/// 歌词译文（非当前行）：比正文更暗，只提供理解辅助。
pub fn lyrics_translation() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.34), rgba(18, 22, 30, 0.38))
}

/// 歌词译文（当前行）：跟随换行柔和变亮，但不追平英文正文。
pub fn lyrics_translation_active() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.76), rgba(18, 22, 30, 0.76))
}

/// 歌词译文（距离当前行较远）：继续压暗，避免整屏译文抢视线。
pub fn lyrics_translation_faint() -> Rgba {
    lerp_rgba(rgba(255, 255, 255, 0.24), rgba(18, 22, 30, 0.26))
}

/// 歌词行悬停背景（虚选中）：使用封面强调色轻染，并区分当前行与非当前行。
pub fn lyrics_hover_bg(is_active: bool) -> Rgba {
    let seed = accent_color();
    let dark_tint = dark_lyrics_accent(seed);
    // 浅色底保留封面色相鲜明度，只对高亮度浅黄等极亮色微压以免融入白底。
    let seed_lum = relative_luminance(seed);
    let light_tint = if seed_lum > 0.55 {
        let k = (0.55 / seed_lum).clamp(0.65, 1.0);
        Rgba {
            r: seed.r * k,
            g: seed.g * k,
            b: seed.b * k,
            a: 1.0,
        }
    } else {
        seed
    };
    let dark = Rgba {
        a: if is_active { 0.22 } else { 0.12 },
        ..dark_tint
    };
    let light = Rgba {
        a: if is_active { 0.16 } else { 0.095 },
        ..light_tint
    };
    lerp_rgba(dark, light)
}

// ---------------------------------------------------------------------------
// 主题色
// ---------------------------------------------------------------------------

/// 强调色：有彩色封面时用封面活力色，否则用品牌绿。
pub fn accent() -> Rgba {
    let seed = accent_color();
    lerp_rgba(seed, light_accent(seed))
}

/// 设置页选中项使用一组配对的文字和底色，随封面和主题切换保持可读。
pub fn selected_choice() -> (Rgba, Rgba) {
    let seed = accent_color();
    let tint = super::motion::mix_rgba(hex(0xE8EEF6), light_accent(seed), 0.22);
    let background = lerp_rgba(seed, tint);
    (background, contrasting_text(background))
}

fn contrasting_text(background: Rgba) -> Rgba {
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(background.r)
        + 0.7152 * linear(background.g)
        + 0.0722 * linear(background.b);
    // 黑字和白字在这个相对亮度下具有相同的对比度。
    if luminance >= 0.17913 {
        hex(0x000000)
    } else {
        hex(0xFFFFFF)
    }
}

fn accent_color() -> Rgba {
    if colorful() {
        accent_seed().unwrap_or_else(|| hex(BRAND_GREEN))
    } else {
        hex(BRAND_GREEN)
    }
}

fn light_accent(seed: Rgba) -> Rgba {
    let lum = relative_luminance(seed);
    let max_lum = 0.34;
    let scale = if lum > max_lum {
        (max_lum / lum).clamp(0.25, 0.68)
    } else {
        0.68
    };
    Rgba {
        r: seed.r * scale,
        g: seed.g * scale,
        b: seed.b * scale,
        a: seed.a,
    }
}

/// 警示色。
pub fn warning() -> Rgba {
    hex(0xFFB454)
}

/// 错误色。
pub fn danger() -> Rgba {
    hex(0xFF6B70)
}

/// 4 组动态流光光斑颜色：有彩色封面时使用 OKLab 拉开色距的四簇封面色，
/// 浅色模式转为明亮水彩粉彩且在黑白封面时保持明净冷白珠光，不再脏灰。
pub fn backdrop_glow_colors() -> [Rgba; 4] {
    let flow = flow_mix();
    let is_colorful = colorful();
    let light_neutral_orbs = [hex(0xD8E2F2), hex(0xE8DFF2), hex(0xD6EAE8), hex(0xE4E7F0)];
    let dark_alphas = if is_colorful {
        [0.54, 0.48, 0.42, 0.36]
    } else {
        [0.24, 0.20, 0.18, 0.14]
    };
    let light_alphas = if is_colorful {
        [0.44, 0.40, 0.36, 0.30]
    } else {
        [0.26, 0.22, 0.20, 0.16]
    };
    std::array::from_fn(|index| {
        let orb_rgb = unpack_rgb(GLOW_COLORS[index].load(Ordering::Relaxed)).unwrap_or_else(|| {
            let [r, g, b] = Palette::NEUTRAL_DARK.glow[index];
            rgba(r, g, b, 1.0)
        });
        let dark = with_alpha(orb_rgb, dark_alphas[index] * flow);
        let light_base = if is_colorful {
            super::motion::mix_rgba(orb_rgb, hex(0xFFFFFF), 0.34)
        } else {
            light_neutral_orbs[index]
        };
        let light = with_alpha(light_base, light_alphas[index] * flow);
        lerp_rgba(dark, light)
    })
}

/// 色场之上的暗化/柔化渐变：保证播放栏与边缘控件清晰可读。
pub fn backdrop_scrim() -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(
            lerp_rgba(rgba(6, 7, 10, 0.10), rgba(255, 255, 255, 0.04)),
            0.0,
        ),
        linear_color_stop(
            lerp_rgba(rgba(5, 6, 9, 0.34), rgba(255, 255, 255, 0.0)),
            1.0,
        ),
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

    /// 主题色是进程级全局状态，修改它的测试必须串行。
    static THEME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn selected_text_uses_contrast_with_the_actual_fill() {
        assert_eq!(contrasting_text(hex(0x375583)), hex(0xFFFFFF));
        assert_eq!(contrasting_text(hex(0xCFDAEB)), hex(0x000000));
        assert_eq!(contrasting_text(hex(BRAND_GREEN)), hex(0x000000));
    }

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
            glow: Palette::NEUTRAL_DARK.glow,
            colorful: true,
        }));
        let accent = accent();
        assert!(accent.r > accent.g && accent.r > accent.b);
        set_ambient_palette(None);
    }

    #[test]
    fn light_surface_follows_dominant_color_and_falls_back_to_neutral() {
        let _guard = THEME_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let neutral = hex(0xFCFCFD);
        let mut palette = Palette::NEUTRAL_DARK;
        palette.colorful = true;
        palette.accent = [201, 45, 75];
        set_ambient_palette(Some(palette));
        let gray_cover = light_surface(0xFCFCFD, 0.23);
        palette.base[2] = [238, 124, 106];
        set_ambient_palette(Some(palette));
        let warm = light_surface(0xFCFCFD, 0.23);
        palette.base[2] = [102, 149, 240];
        set_ambient_palette(Some(palette));
        let cool = light_surface(0xFCFCFD, 0.23);
        set_ambient_palette(None);
        assert_eq!(gray_cover, neutral);
        assert!(warm.r > warm.b && warm.b > 0.88);
        assert!(cool.b > cool.r && cool.r > 0.88);
        assert_eq!(light_surface(0xFCFCFD, 0.23), neutral);
    }

    #[test]
    fn lyrics_sung_keeps_readable_contrast_in_dark_and_light_modes() {
        let _guard = THEME_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut palette = Palette::NEUTRAL_DARK;
        palette.colorful = true;
        // 极暗封面色在深色模式下应提亮
        palette.accent = [20, 30, 80];
        set_ambient_palette(Some(palette));
        set_light_mix(0.0);
        let dark_lum = relative_luminance(lyrics_sung());
        assert!(dark_lum >= 0.40, "dark sung luminance too low: {dark_lum}");

        // 极亮封面色在浅色模式下应压暗
        palette.accent = [250, 240, 120];
        set_ambient_palette(Some(palette));
        set_light_mix(1.0);
        let light_lum = relative_luminance(lyrics_sung());
        assert!(
            light_lum <= 0.32,
            "light sung luminance too high: {light_lum}"
        );

        // 当前行未唱词应与非当前行区分开，浅色下主强调色也不能过亮发灰
        set_light_mix(1.0);
        assert!(lyrics_pending().a > lyrics_inactive().a);
        assert!(relative_luminance(accent()) <= 0.36);

        set_light_mix(0.0);
        assert!(lyrics_pending().a > lyrics_inactive().a);
        set_ambient_palette(None);
    }
}
