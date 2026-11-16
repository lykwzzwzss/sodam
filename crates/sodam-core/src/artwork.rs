//! 专辑封面：从音频文件里取内嵌封面，并算出可以驱动整窗氛围的主题色。
//!
//! 取色不是「全图平均」：平均会把封面里的高光、暗部、文字一起搅成灰。
//! 这里先把封面缩到小图，在 **OKLab** 里做 k-means 聚类，再按
//! 「占比 × 彩度 × 中间亮度」挑活力色；灰白封面退化为无彩色，由界面用品牌绿兜底。
//!
//! 领域层只输出字节和 RGB，不写文件、不碰 GPUI；落盘缓存与模糊由应用层做。

use std::path::Path;

use lofty::file::TaggedFileExt;

/// 从封面算出的调色板（sRGB，0-255）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// 活力色：占比不小、彩度较高、亮度不过暗/过亮，界面强调色用它。
    /// 灰白封面时等于 `dominant`，由调用方决定是否兜底。
    pub accent: [u8; 3],
    /// 占比最大的颜色（可能是背景色/暗部）。
    pub dominant: [u8; 3],
    /// 环境色：活力色压暗到适合做深色背景的亮度；灰白封面时是中性深灰。
    pub ambient: [u8; 3],
    /// 9 阶主题色（base-1 最亮 → base-9 最暗），对齐汽水音乐的服务端色板。
    ///
    /// 背景用 `base[6]`（base-7），渐变用 `base[4]`（base-5），
    /// 进度/高亮用 `base[2]`（base-3）；灰白封面回退到中性灰阶。
    pub base: [[u8; 3]; 9],
    /// 封面多色流光光斑（最多 4 组在 OKLab 中拉开色距的封面色）。
    /// 灰白封面时保持细腻中性冷灰，不凭空染假彩。
    pub glow: [[u8; 3]; 4],
    /// 封面是否有可用的彩色（最大彩度达到阈值）。
    pub colorful: bool,
}

impl Palette {
    /// 「没有封面」的中性色板。
    ///
    /// 页面背景用 `base[4] → base[7]`，侧栏/播放栏用半透明的 `base[6]`：
    /// 这组值让页面回到中性深灰，切换封面时结构不跳变，只做颜色插值。
    pub const NEUTRAL_DARK: Palette = Palette {
        accent: [0x46, 0x46, 0x49],
        dominant: [0x12, 0x12, 0x14],
        ambient: [0x0E, 0x0E, 0x10],
        base: [
            [0x3A, 0x3A, 0x3E],
            [0x30, 0x30, 0x34],
            [0x26, 0x26, 0x2A],
            [0x1A, 0x1A, 0x1E],
            [0x0E, 0x0E, 0x10],
            [0x14, 0x14, 0x17],
            [0x1E, 0x1E, 0x22],
            [0x05, 0x05, 0x06],
            [0x03, 0x03, 0x04],
        ],
        glow: [
            [0x34, 0x3A, 0x48],
            [0x2A, 0x30, 0x3D],
            [0x3E, 0x3B, 0x4A],
            [0x26, 0x2B, 0x36],
        ],
        colorful: false,
    };

    /// 两套调色板之间的插值（切换封面/歌单时让背景与强调色一起过渡）。
    ///
    /// 在 OKLab 里插值：直接混 sRGB 会让过渡中段发灰；OKLab 保持色相与
    /// 亮度感知均匀得多。`colorful` 在过渡期间取并集，避免中途突然从
    /// 主题色模式掉回灰阶、产生一次跳色。
    pub fn lerp(from: Self, to: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        if t <= 0.0 {
            return from;
        }
        if t >= 1.0 {
            return to;
        }
        fn mix(from: [u8; 3], to: [u8; 3], t: f32) -> [u8; 3] {
            let from = rgb_to_oklab(from[0], from[1], from[2]);
            let to = rgb_to_oklab(to[0], to[1], to[2]);
            oklab_to_rgb([
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
                from[2] + (to[2] - from[2]) * t,
            ])
        }
        Self {
            accent: mix(from.accent, to.accent, t),
            dominant: mix(from.dominant, to.dominant, t),
            ambient: mix(from.ambient, to.ambient, t),
            base: std::array::from_fn(|index| mix(from.base[index], to.base[index], t)),
            glow: std::array::from_fn(|index| mix(from.glow[index], to.glow[index], t)),
            colorful: from.colorful || to.colorful,
        }
    }
}

/// 只取封面字节，不解码、不算色：给列表缩略图这类只需要原图的路径用。
#[derive(Clone, Debug)]
pub struct CoverData {
    pub bytes: Vec<u8>,
    pub mime: Option<String>,
}

/// 读取音频文件的第一张内嵌封面（不要求能解码）。
pub fn load_cover_data(path: &Path) -> Option<CoverData> {
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let picture = tag?.pictures().first()?;
    let data = picture.data();
    if data.is_empty() {
        return None;
    }
    Some(CoverData {
        bytes: data.to_vec(),
        mime: picture
            .mime_type()
            .map(|mime| mime.as_str().to_string())
            .or_else(|| sniff_image_mime(data).map(str::to_string)),
    })
}

/// 只算色：给「已经有图片字节」的调用方用（缓存命中、测试）。
pub fn palette_from_image_bytes(bytes: &[u8]) -> Option<Palette> {
    let image = image::load_from_memory(bytes).ok()?;
    Some(palette_from_rgba(
        image.to_rgba8().as_raw(),
        image.width(),
        image.height(),
    ))
}

/// 从 RGBA 像素算调色板。
///
/// `width * height` 必须等于 `rgba.len() / 4`，否则返回默认调色板 —— 这是内部缓存
/// 边界，不做 panic。
pub fn palette_from_rgba(rgba: &[u8], width: u32, height: u32) -> Palette {
    let Some(pixels) = usize::try_from(width)
        .ok()
        .zip(usize::try_from(height).ok())
        .and_then(|(w, h)| w.checked_mul(h))
        .filter(|count| *count > 0 && rgba.len() >= count.saturating_mul(4))
    else {
        return Palette::NEUTRAL_DARK;
    };

    // 采样上限：封面经常是 1000px+，只需要颜色分布，不需要每个像素。
    let step = ((pixels as f64 / 4096.0).sqrt().ceil() as usize).max(1);
    let mut samples: Vec<Sample> = Vec::with_capacity(4096);
    let mut index = 0usize;
    while index < pixels {
        let offset = index * 4;
        let alpha = rgba[offset + 3];
        if alpha >= 128 {
            let r = rgba[offset];
            let g = rgba[offset + 1];
            let b = rgba[offset + 2];
            samples.push(Sample {
                lab: rgb_to_oklab(r, g, b),
                rgb: [r, g, b],
            });
        }
        index += step;
    }
    if samples.is_empty() {
        return Palette::NEUTRAL_DARK;
    }

    // 小图直接全采样；大图按步长采样。k=6 足够覆盖「背景/主体/高光/暗部」。
    let clusters = kmeans(&samples, 6, 18);
    if clusters.is_empty() {
        return Palette::NEUTRAL_DARK;
    }

    let total_weight: f64 = clusters.iter().map(|cluster| cluster.weight).sum();
    let total_weight = total_weight.max(1e-6);
    let dominant_cluster = clusters
        .iter()
        .max_by(|left, right| left.weight.total_cmp(&right.weight))
        .unwrap_or(&clusters[0]);
    let dominant = dominant_cluster.center_rgb;

    // 活力色评分：占比开方（避免一块背景色压死主体）+ 彩度 + 中间亮度窗口。
    // 亮度窗口用平滑权重，纯黑/纯白/灰字不会胜出。
    let vibrant = clusters
        .iter()
        .map(|cluster| {
            let chroma = chroma_of(cluster.lab);
            let lightness = cluster.lab[0];
            let light_weight = f64::from(bell(lightness, 0.56, 0.30));
            let share = (cluster.weight / total_weight).max(1e-6);
            let score = share.powf(0.45) * (0.12 + f64::from(chroma) * 2.6) * light_weight;
            (score, cluster)
        })
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, cluster)| cluster)
        .unwrap_or(&clusters[0]);
    let max_chroma = clusters
        .iter()
        .map(|cluster| chroma_of(cluster.lab))
        .fold(0.0f32, f32::max);
    let colorful = max_chroma >= 0.045;

    // 整页主色用**占比最大的颜色**（汽水服务端的 dominant_color 同款思路），
    // 不是最鲜艳的那一块。黄色封面里几栋红房子、白底里一个小红章都只是
    // 少量面积，不会把整页染成红色；强调色仍取最鲜艳的色簇。
    let page_seed = dominant_cluster.lab;

    let accent = if colorful {
        // 聚类均值可能偏暗（封面的阴影部分也属于同一色簇）。界面上的强调色
        // 要压在深色背景上看得清，所以把亮度抬到 0.66 上下，彩度不足时补一点。
        vivid_from(vibrant.lab).unwrap_or(vibrant.center_rgb)
    } else {
        dominant
    };
    let ambient = if colorful {
        ambient_from(page_seed)
    } else {
        [0x0A, 0x0A, 0x0C]
    };
    let base = if colorful {
        base_ramp(page_seed)
    } else {
        Palette::NEUTRAL_DARK.base
    };
    let glow = if colorful {
        glow_palette(&clusters, total_weight, vibrant.lab)
    } else {
        Palette::NEUTRAL_DARK.glow
    };

    Palette {
        accent,
        dominant,
        ambient,
        base,
        glow,
        colorful,
    }
}

/// 一个采样点：OKLab 用于聚类，sRGB 用于最终输出（避免 lab 反变换带来的偏移）。
#[derive(Clone, Copy)]
struct Sample {
    lab: [f32; 3],
    rgb: [u8; 3],
}

struct Cluster {
    center_rgb: [u8; 3],
    lab: [f32; 3],
    weight: f64,
}

/// 确定性 k-means++：同一张封面每次启动得到同一个调色板（快照测试与缓存都依赖它）。
fn kmeans(samples: &[Sample], k: usize, iterations: usize) -> Vec<Cluster> {
    let k = k.min(samples.len());
    if k == 0 {
        return Vec::new();
    }
    let mut rng = XorShift::new(0x9E37_79B9_7F4A_7C15);
    let mut centers: Vec<[f32; 3]> = Vec::with_capacity(k);
    // k-means++：第一个中心取一个随机点，之后按 D(x)^2 加权抽取。
    centers.push(samples[rng.next_usize(samples.len())].lab);
    let mut distances = vec![f32::INFINITY; samples.len()];
    while centers.len() < k {
        let latest = *centers.last().expect("centers 非空");
        let mut total = 0.0f64;
        for (index, sample) in samples.iter().enumerate() {
            let distance = lab_distance_sq(sample.lab, latest);
            if distance < distances[index] {
                distances[index] = distance;
            }
            total += f64::from(distances[index]);
        }
        if total <= f64::EPSILON {
            break;
        }
        let mut target = rng.next_f64() * total;
        let mut picked = samples.len() - 1;
        for (index, distance) in distances.iter().enumerate() {
            target -= f64::from(*distance);
            if target <= 0.0 {
                picked = index;
                break;
            }
        }
        centers.push(samples[picked].lab);
    }

    let mut assignments = vec![0usize; samples.len()];
    for _ in 0..iterations {
        let mut changed = false;
        for (index, sample) in samples.iter().enumerate() {
            let mut best = 0usize;
            let mut best_distance = f32::INFINITY;
            for (center_index, center) in centers.iter().enumerate() {
                let distance = lab_distance_sq(sample.lab, *center);
                if distance < best_distance {
                    best_distance = distance;
                    best = center_index;
                }
            }
            if assignments[index] != best {
                assignments[index] = best;
                changed = true;
            }
        }
        // 更新中心只用 OKLab 平均：最终输出色另在下方按 sRGB 重新求均值。
        let mut sums = vec![([0.0f64; 3], 0u64); centers.len()];
        for (index, sample) in samples.iter().enumerate() {
            let slot = &mut sums[assignments[index]];
            for axis in 0..3 {
                slot.0[axis] += f64::from(sample.lab[axis]);
            }
            slot.1 += 1;
        }
        for (center, (lab_sum, count)) in centers.iter_mut().zip(sums.iter()) {
            if *count == 0 {
                continue;
            }
            let count = *count as f64;
            let next = [
                (lab_sum[0] / count) as f32,
                (lab_sum[1] / count) as f32,
                (lab_sum[2] / count) as f32,
            ];
            if lab_distance_sq(*center, next) > 1e-7 {
                changed = true;
            }
            *center = next;
        }
        if !changed {
            break;
        }
    }

    let mut clusters = Vec::with_capacity(centers.len());
    for center_index in 0..centers.len() {
        let mut lab_sum = [0.0f64; 3];
        let mut rgb_sum = [0.0f64; 3];
        let mut weight = 0u64;
        for (index, sample) in samples.iter().enumerate() {
            if assignments[index] != center_index {
                continue;
            }
            for axis in 0..3 {
                lab_sum[axis] += f64::from(sample.lab[axis]);
                rgb_sum[axis] += f64::from(sample.rgb[axis]);
            }
            weight += 1;
        }
        if weight == 0 {
            continue;
        }
        let weight_f = weight as f64;
        let mean_rgb = [
            (rgb_sum[0] / weight_f).round().clamp(0.0, 255.0) as u8,
            (rgb_sum[1] / weight_f).round().clamp(0.0, 255.0) as u8,
            (rgb_sum[2] / weight_f).round().clamp(0.0, 255.0) as u8,
        ];
        clusters.push(Cluster {
            center_rgb: mean_rgb,
            lab: [
                (lab_sum[0] / weight_f) as f32,
                (lab_sum[1] / weight_f) as f32,
                (lab_sum[2] / weight_f) as f32,
            ],
            weight: weight as f64,
        });
    }
    clusters
}

fn chroma_of(lab: [f32; 3]) -> f32 {
    (lab[1] * lab[1] + lab[2] * lab[2]).sqrt()
}

/// 把活力色压到深色背景可用的亮度，并保留一部分彩度。
fn ambient_from(lab: [f32; 3]) -> [u8; 3] {
    let chroma = chroma_of(lab);
    // 近中性色（白底/灰底）不要凭空补出颜色。
    if chroma < 0.02 {
        return [0x0A, 0x0A, 0x0C];
    }
    let scale = if chroma > 1e-5 {
        (0.085 / chroma).clamp(0.55, 1.6)
    } else {
        1.0
    };
    let ambient = [
        // 0.20 的 L 在 OKLab 里已经是明显的深色，再叠一层 80% 黑就是我们看到的
        // 汽水音乐那种「背景有颜色但绝不抢内容」的层次。
        0.20,
        lab[1] * scale,
        lab[2] * scale,
    ];
    oklab_to_rgb(ambient)
}

/// 界面强调色：保留封面色相，把亮度抬到深色背景上清晰可读的区间。
fn vivid_from(lab: [f32; 3]) -> Option<[u8; 3]> {
    let chroma = chroma_of(lab);
    if chroma < 1e-5 {
        return None;
    }
    let scale = (0.13 / chroma).clamp(0.6, 1.5);
    Some(oklab_to_rgb([
        lab[0].clamp(0.66, 0.82),
        lab[1] * scale,
        lab[2] * scale,
    ]))
}

/// 由主色生成 9 阶色板：1 最亮、7 是主背景、9 最深。
///
/// 亮度步长不是等距的：4→9 挤在深色区间（背景层次细腻），1→3 拉得开
/// （高光/进度条需要明显可见）。彩度在两端衰减，避免纯白高光或纯黑死色。
fn base_ramp(lab: [f32; 3]) -> [[u8; 3]; 9] {
    // 与汽水默认 9 阶灰（#F2…#1A）的 OKLab 明度对齐：色板是「整页的明暗阶梯」，
    // 不是一味压黑。背景用 base-5 → base-7，暗化交给 scrim。
    const LIGHTNESS: [f32; 9] = [0.96, 0.87, 0.78, 0.66, 0.50, 0.43, 0.36, 0.29, 0.20];
    let chroma = chroma_of(lab);
    // 近中性色（白底/灰底）回到中性深色板，不拿一点点色偏铺满整页。
    if chroma < 0.02 {
        return Palette::NEUTRAL_DARK.base;
    }
    // 目标彩度 0.11：能明显看出封面色相，又不会在深色端变成霓虹或越界。
    let normalize = (0.11 / chroma).min(1.5);
    let mut ramp = [[0u8; 3]; 9];
    for (index, lightness) in LIGHTNESS.iter().enumerate() {
        let edge = (1.0 - (2.0 * lightness - 1.0).abs()).sqrt();
        let scale = normalize * (0.45 + 0.55 * edge);
        ramp[index] = oklab_to_rgb([*lightness, lab[1] * scale, lab[2] * scale]);
    }
    ramp
}

/// 从封面色彩聚类中挑出最多 4 组在 OKLab 里色距充分拉开的光斑色，
/// 用于全局动态流光的四象限配色；少于 4 簇时在同一色系内补出柔和层次。
fn glow_palette(clusters: &[Cluster], total_weight: f64, seed: [f32; 3]) -> [[u8; 3]; 4] {
    let normalize_orb = |lab: [f32; 3]| -> [f32; 3] {
        let chroma = chroma_of(lab);
        let scale = if chroma > 1e-5 {
            (0.14 / chroma).clamp(0.75, 1.55)
        } else {
            1.0
        };
        [lab[0].clamp(0.46, 0.72), lab[1] * scale, lab[2] * scale]
    };

    let candidates: Vec<(f64, [f32; 3])> = clusters
        .iter()
        .filter(|cluster| chroma_of(cluster.lab) >= 0.025)
        .map(|cluster| {
            let chroma = chroma_of(cluster.lab);
            let share = (cluster.weight / total_weight).max(1e-6);
            let base_score = share.powf(0.35) * (0.18 + f64::from(chroma) * 2.8);
            (base_score, normalize_orb(cluster.lab))
        })
        .collect();

    if candidates.is_empty() {
        return Palette::NEUTRAL_DARK.glow;
    }

    let mut chosen: Vec<[f32; 3]> = Vec::with_capacity(4);
    chosen.push(normalize_orb(seed));

    while chosen.len() < 4 {
        let mut best: Option<(f64, [f32; 3])> = None;
        for (base_score, lab) in &candidates {
            let min_dist_sq = chosen
                .iter()
                .map(|picked| lab_distance_sq(*picked, *lab))
                .fold(f32::INFINITY, f32::min);
            // 仅在还算明显不同色时继续从封面聚类吸纳新色
            if min_dist_sq < 0.004 {
                continue;
            }
            let diversity = f64::from(min_dist_sq.sqrt().min(0.32));
            let score = *base_score * (0.35 + diversity * 3.0);
            if best
                .as_ref()
                .is_none_or(|(best_score, _)| score > *best_score)
            {
                best = Some((score, *lab));
            }
        }
        match best {
            Some((_, lab)) => chosen.push(lab),
            None => break,
        }
    }

    // 若封面只有 1~2 种彩色，从已有簇里做轻微色相偏移和明暗错落补足 4 个光斑
    let base_count = chosen.len();
    let hue_offsets: [f32; 4] = [0.0, 0.28, -0.26, 0.14];
    let light_offsets: [f32; 4] = [0.0, -0.07, 0.06, -0.03];
    while chosen.len() < 4 {
        let idx = chosen.len();
        let src = chosen[idx % base_count];
        let angle = hue_offsets[idx];
        let (sin, cos) = angle.sin_cos();
        chosen.push([
            (src[0] + light_offsets[idx]).clamp(0.42, 0.76),
            src[1] * cos - src[2] * sin,
            src[1] * sin + src[2] * cos,
        ]);
    }

    std::array::from_fn(|i| oklab_to_rgb(chosen[i]))
}

fn bell(value: f32, center: f32, width: f32) -> f32 {
    let distance = (value - center) / width;
    (-(distance * distance)).exp()
}

fn lab_distance_sq(left: [f32; 3], right: [f32; 3]) -> f32 {
    let dl = left[0] - right[0];
    let da = left[1] - right[1];
    let db = left[2] - right[2];
    dl * dl + da * da + db * db
}

/// RGB(0-255) → OKLab（Björn Ottosson 的标准矩阵）。
fn rgb_to_oklab(r: u8, g: u8, b: u8) -> [f32; 3] {
    let r = srgb_to_linear(f32::from(r) / 255.0);
    let g = srgb_to_linear(f32::from(g) / 255.0);
    let b = srgb_to_linear(f32::from(b) / 255.0);

    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;

    let l = l.cbrt();
    let m = m.cbrt();
    let s = s.cbrt();

    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn oklab_to_rgb(lab: [f32; 3]) -> [u8; 3] {
    // 直接把 OKLab 转成 sRGB 在深色端经常越界（例如低亮度的黄色），
    // 逐通道 clamp 会把黄色压成红褐色。这里保持明度与色相，二分降低彩度
    // 直到落回 sRGB 色域，再转成字节。
    let linear = oklab_to_linear(lab);
    if in_gamut(linear) {
        return linear_to_bytes(linear);
    }
    let [l, a, b] = lab;
    let mut low = 0.0f32;
    let mut high = 1.0f32;
    let mut best = oklab_to_linear([l, 0.0, 0.0]);
    for _ in 0..18 {
        let mid = (low + high) * 0.5;
        let candidate = oklab_to_linear([l, a * mid, b * mid]);
        if in_gamut(candidate) {
            low = mid;
            best = candidate;
        } else {
            high = mid;
        }
    }
    linear_to_bytes(best)
}

fn oklab_to_linear(lab: [f32; 3]) -> [f32; 3] {
    let l = lab[0];
    let a = lab[1];
    let b = lab[2];
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

fn in_gamut(linear: [f32; 3]) -> bool {
    linear
        .iter()
        .all(|channel| (-1e-4..=1.000_1).contains(channel))
}

fn linear_to_bytes(linear: [f32; 3]) -> [u8; 3] {
    [
        linear_to_srgb_byte(linear[0]),
        linear_to_srgb_byte(linear[1]),
        linear_to_srgb_byte(linear[2]),
    ]
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_byte(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let srgb = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0).round().clamp(0.0, 255.0) as u8
}

fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1A\n") {
        Some("image/png")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else {
        None
    }
}

/// 小尺寸、固定种子的 xorshift：只为可复现的 k-means++ 抽样，不用于安全场景。
struct XorShift(u64);

impl XorShift {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound.max(1) as u64) as usize
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            data.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        data
    }

    #[test]
    fn red_subject_on_dark_background_keeps_page_neutral() {
        // 39% 的红块 + 黑色主背景：红色只做强调色，整页仍由 dominant 黑色决定。
        let mut data = solid(64, 64, [8, 4, 6]);
        for y in 12..52 {
            for x in 12..52 {
                let offset = ((y * 64 + x) * 4) as usize;
                data[offset..offset + 3].copy_from_slice(&[196, 28, 44]);
            }
        }
        let palette = palette_from_rgba(&data, 64, 64);
        assert!(palette.colorful, "红块彩度足够，应判为彩色封面");
        assert!(
            palette.accent[0] > palette.accent[1] + 40,
            "accent 应偏红: {:?}",
            palette.accent
        );
        assert_eq!(
            palette.base,
            Palette::NEUTRAL_DARK.base,
            "黑色占大头时整页应保持中性"
        );
        // 环境色必须是深色，不能拿主色直接铺满整窗。
        assert!(
            palette.ambient[0] < 140,
            "ambient 太亮: {:?}",
            palette.ambient
        );
    }

    #[test]
    fn grayscale_cover_is_not_colorful() {
        let mut data = Vec::new();
        for y in 0..32u32 {
            for x in 0..32u32 {
                let level = ((x + y) * 4) as u8;
                data.extend_from_slice(&[level, level, level, 255]);
            }
        }
        let palette = palette_from_rgba(&data, 32, 32);
        assert!(!palette.colorful, "灰阶封面不应有彩色主色");
        assert_eq!(palette.ambient, [0x0A, 0x0A, 0x0C]);
    }

    #[test]
    fn colorful_palette_has_a_dark_balanced_nine_step_ramp() {
        let mut data = solid(48, 48, [210, 70, 90]);
        for y in 10..38 {
            for x in 10..38 {
                let offset = ((y * 48 + x) * 4) as usize;
                data[offset..offset + 3].copy_from_slice(&[230, 90, 110]);
            }
        }
        let palette = palette_from_rgba(&data, 48, 48);
        assert!(palette.colorful);
        let luma = |rgb: [u8; 3]| {
            u32::from(rgb[0]) * 299 + u32::from(rgb[1]) * 587 + u32::from(rgb[2]) * 114
        };
        // 1 最亮 → 9 最暗，且主背景阶必须是深色（否则大面积背景会发白）。
        for pair in palette.base.windows(2) {
            assert!(
                luma(pair[0]) >= luma(pair[1]),
                "色阶必须递减: {:?}",
                palette.base
            );
        }
        assert!(
            luma(palette.base[6]) < 90_000,
            "base-7 应足够深: {:?}",
            palette.base[6]
        );
        assert!(
            palette.base[2][0] > palette.base[6][0],
            "高光阶应比背景阶亮"
        );
    }

    /// 大面积白底 + 一小块红：红色只做强调色，整页必须回到中性深色。
    #[test]
    fn small_colorful_patch_does_not_tint_the_whole_page() {
        let mut data = solid(64, 64, [250, 250, 250]);
        for y in 24..40 {
            for x in 24..40 {
                let offset = ((y * 64 + x) * 4) as usize;
                data[offset..offset + 3].copy_from_slice(&[210, 30, 40]);
            }
        }
        let palette = palette_from_rgba(&data, 64, 64);
        assert!(palette.colorful, "红块足够鲜艳，强调色应可用");
        assert!(
            palette.accent[0] > palette.accent[1] + 20,
            "强调色仍应偏红: {:?}",
            palette.accent
        );
        assert_eq!(
            palette.base,
            Palette::NEUTRAL_DARK.base,
            "白底不能被一小块红染成整页红色: {:?}",
            palette.base
        );
    }

    /// 暖黄色封面里有一块红色建筑：整页应保持暖黄，而不是被红色带成通红。
    #[test]
    fn warm_yellow_cover_stays_warm_not_red() {
        let mut data = solid(64, 64, [245, 220, 140]);
        for y in 0..64 {
            for x in 0..19 {
                let offset = ((y * 64 + x) * 4) as usize;
                data[offset..offset + 3].copy_from_slice(&[180, 60, 40]);
            }
        }
        let palette = palette_from_rgba(&data, 64, 64);
        assert!(palette.colorful);
        assert!(
            palette.base[6][1] > palette.base[6][2] + 5,
            "整页应保留暖黄色的绿/蓝通道差，而不是变成纯红: {:?}",
            palette.base[6]
        );
        assert!(
            palette.base[6][0] > palette.base[6][2],
            "整页仍应是暖色: {:?}",
            palette.base[6]
        );
    }

    /// 彩色区域占大头时，整页色板跟着它走。
    #[test]
    fn large_color_area_drives_the_page_tint() {
        let mut data = solid(64, 64, [10, 8, 8]);
        for y in 0..64 {
            for x in 0..38 {
                let offset = ((y * 64 + x) * 4) as usize;
                data[offset..offset + 3].copy_from_slice(&[200, 40, 50]);
            }
        }
        let palette = palette_from_rgba(&data, 64, 64);
        assert!(palette.colorful);
        assert!(
            palette.base[6][0] > palette.base[6][2] + 8,
            "红色占大头时整页应偏红: {:?}",
            palette.base[6]
        );
    }

    #[test]
    fn grayscale_palette_uses_the_neutral_ramp() {
        let mut data = Vec::new();
        for value in [10u8, 80, 160, 240] {
            for _ in 0..400 {
                data.extend_from_slice(&[value, value, value, 255]);
            }
        }
        let palette = palette_from_rgba(&data, 40, 40);
        assert_eq!(palette.base, Palette::NEUTRAL_DARK.base);
    }

    #[test]
    fn transparent_pixels_are_ignored() {
        let mut data = solid(16, 16, [0, 200, 60]);
        for pixel in data.chunks_exact_mut(4) {
            pixel[3] = 0; // 全透明：没有可用采样，走默认调色板
        }
        assert_eq!(palette_from_rgba(&data, 16, 16), Palette::NEUTRAL_DARK);
    }

    #[test]
    fn malformed_dimensions_fall_back_to_default() {
        assert_eq!(palette_from_rgba(&[0; 16], 8, 8), Palette::NEUTRAL_DARK);
        assert_eq!(palette_from_rgba(&[], 0, 0), Palette::NEUTRAL_DARK);
    }

    #[test]
    fn palette_is_deterministic_for_same_pixels() {
        let mut data = Vec::new();
        for y in 0..48u32 {
            for x in 0..48u32 {
                let r = (x * 5) as u8;
                let g = (y * 5) as u8;
                let b = ((x + y) * 2) as u8;
                data.extend_from_slice(&[r, g, b, 255]);
            }
        }
        let first = palette_from_rgba(&data, 48, 48);
        let second = palette_from_rgba(&data, 48, 48);
        assert_eq!(first, second, "同一张封面必须得到同一个调色板");
    }

    #[test]
    fn neutral_dark_ramp_stays_dark_for_transitions() {
        let luma = |rgb: [u8; 3]| {
            u32::from(rgb[0]) * 299 + u32::from(rgb[1]) * 587 + u32::from(rgb[2]) * 114
        };
        // 过渡用的是 base-5（页面顶部）和 base-7（页面底部/侧栏），
        // 必须保持在深色区间，不能是中灰。
        assert!(luma(Palette::NEUTRAL_DARK.base[4]) < 40_000);
        assert!(luma(Palette::NEUTRAL_DARK.base[6]) < 35_000);
    }

    #[test]
    fn palette_lerp_hits_both_ends_and_keeps_color_mode() {
        // 红色占大头，整页色板本身必须是有颜色的。
        let mut red = solid(32, 32, [210, 40, 60]);
        for y in 8..16 {
            for x in 8..16 {
                let offset = ((y * 32 + x) * 4) as usize;
                red[offset..offset + 3].copy_from_slice(&[30, 6, 10]);
            }
        }
        let colored = palette_from_rgba(&red, 32, 32);
        let neutral = Palette::NEUTRAL_DARK;
        assert_eq!(Palette::lerp(neutral, colored, 0.0), neutral);
        assert_eq!(Palette::lerp(neutral, colored, 1.0), colored);

        let middle = Palette::lerp(neutral, colored, 0.5);
        // 过渡中段仍保持主题色模式，免得 UI 中途切回灰阶再切回来。
        assert!(middle.colorful);
        // 中间色确实是两端的混合，而不是停在某一端。
        assert_ne!(middle, neutral);
        assert_ne!(middle, colored);
        assert_ne!(middle.base[4], neutral.base[4]);
        assert_ne!(middle.base[4], colored.base[4]);
        // 红色封面和灰阶兜底之间的每一帧都不应引入品牌绿。
        for step in 0..=20 {
            let [red, green, _] = Palette::lerp(neutral, colored, step as f32 / 20.0).accent;
            assert!(red >= green);
        }
    }

    #[test]
    fn glow_extracts_distinct_colors_from_multicolor_cover() {
        let mut data = solid(48, 48, [220, 45, 95]);
        for y in 0..48u32 {
            for x in 0..48u32 {
                let offset = ((y * 48 + x) * 4) as usize;
                let color = if x >= 24 && y < 24 {
                    [40, 110, 235]
                } else if (12..36).contains(&x) && y >= 24 {
                    [255, 190, 70]
                } else {
                    [220, 45, 95]
                };
                data[offset..offset + 3].copy_from_slice(&color);
            }
        }
        let palette = palette_from_rgba(&data, 48, 48);
        assert!(palette.colorful);
        assert_ne!(palette.glow[0], palette.glow[1]);
        assert!(palette.glow.iter().any(|c| c[0] > c[2] + 30));
        assert!(palette.glow.iter().any(|c| c[2] > c[0] + 30));
    }
}
