//! 当前曲目的封面：从音频标签里取图、落盘缓存、算出整页色板。
//!
//! 这是 UI 侧的装饰层：任何一步失败（没封面、解码失败、缓存目录不可写）都只是
//! 退回中性色板，不向用户报错，也不阻塞渲染。解码在后台线程，结果回 UI 线程。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use sodam_core::artwork::{self, Palette};

/// 一张已经准备好的封面：原图给播放栏/封面卡片，色板驱动整页颜色。
#[derive(Clone, Debug)]
pub struct ArtworkView {
    /// 对应曲目的 id（`local:` 前缀），用来丢弃过期的异步结果。
    pub track_id: String,
    /// 原图缓存文件（JPEG/PNG/WebP）。
    pub cover_path: PathBuf,
    pub palette: Palette,
}

/// 方块缩略图 + 从同一张图算出的色板 + 顺手落盘的全尺寸封面。
pub struct PreparedSquare {
    pub path: PathBuf,
    pub palette: Palette,
    /// 原图字节写成的缓存文件；写盘失败时为 `None`，切歌时退回读标签。
    pub cover_path: Option<PathBuf>,
}

/// 后台线程调用：准备全尺寸封面与色板。
///
/// `cached_cover` / `cached_palette` 来自缩略图预热：预热时已经读过标签、
/// 解码过一次，并把手里的原图落盘。切歌时直接读缓存文件，不再走
/// `lofty::read_from_path`；色板也有缓存时甚至不需要解码。
///
/// 返回 `None` 表示这首歌没有可用的封面；调用方用中性色板。
pub fn prepare(
    track_id: &str,
    audio_path: &Path,
    cached_cover: Option<&Path>,
    cached_palette: Option<Palette>,
) -> Option<ArtworkView> {
    let cache_dir = cache_dir()?;
    std::fs::create_dir_all(&cache_dir).ok()?;

    // 优先读预热时落盘的原图；没有缓存才回退到标签解析。
    let bytes = cached_cover
        .and_then(|path| std::fs::read(path).ok())
        .or_else(|| artwork::load_cover_data(audio_path).map(|cover| cover.bytes))?;

    let mut hasher = DefaultHasher::new();
    audio_path.hash(&mut hasher);
    bytes.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());

    let extension = extension_for(&bytes, None);
    let cover_path = cache_dir.join(format!("cover-{key}.{extension}"));

    // 缓存命中直接复用；`exists()` 不保证完整，但写入走临时文件 + rename，
    // 半截文件不会出现在最终路径上。
    if !cover_path.is_file() && write_atomic(&cover_path, &bytes).is_err() {
        // 并发写同一首时 rename 可能失败；只要目标已经出现就继续。
        if !cover_path.is_file() {
            return None;
        }
    }

    // 已有缩略图就用小图算色板，避免为了背景再次解码原图。
    let palette = match cached_palette {
        Some(palette) => palette,
        None => {
            let sampled = image::open(cache_dir.join(format!("thumb-{key}.png")))
                .ok()
                .or_else(|| {
                    image::load_from_memory(&bytes)
                        .ok()
                        .map(|image| image.thumbnail(256, 256))
                })?;
            let rgba = sampled.to_rgba8();
            artwork::palette_from_rgba(rgba.as_raw(), sampled.width(), sampled.height())
        }
    };

    Some(ArtworkView {
        track_id: track_id.to_string(),
        cover_path,
        palette,
    })
}

/// 列表缩略图：96×96 方块缓存 + 色板。
///
/// 主列表一屏十几行，直接拿几 MB 的原图给 GPUI 解码会卡；缩略图在后台预生成，
/// 行渲染时只读一个几十 KB 的 PNG。色板顺手从同一张解码图上算好，
/// 切歌时先用它启动背景过渡，不用等全尺寸封面。
pub fn prepare_thumb(audio_path: &Path) -> Option<PreparedSquare> {
    prepare_square(audio_path, 96, "thumb")
}

/// 歌单头图：320px 方块缓存。头图显示 200px，96px 缩略图会糊；
/// 320px 足够清晰，又比全尺寸封面省解码。
pub fn prepare_hero_thumb(audio_path: &Path) -> Option<PathBuf> {
    prepare_square(audio_path, 320, "hero").map(|prepared| prepared.path)
}

fn prepare_square(audio_path: &Path, size: u32, prefix: &str) -> Option<PreparedSquare> {
    let cover = artwork::load_cover_data(audio_path)?;
    let cache_dir = cache_dir()?;
    std::fs::create_dir_all(&cache_dir).ok()?;

    let mut hasher = DefaultHasher::new();
    audio_path.hash(&mut hasher);
    cover.bytes.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());
    let output_path = cache_dir.join(format!("{prefix}-{key}.png"));

    // 顺手把原图落盘：切歌时 `prepare` 直接读它，不必再为同一首歌解析标签。
    let extension = extension_for(&cover.bytes, cover.mime.as_deref());
    let cover_path = cache_dir.join(format!("cover-{key}.{extension}"));
    if !cover_path.is_file() {
        let _ = write_atomic(&cover_path, &cover.bytes);
    }
    let cover_path = cover_path.is_file().then_some(cover_path);

    // 缓存命中时读回小图算色（96px 解码成本可以忽略），
    // 这样升级/重启后缓存里的封面也能立即参与切歌过渡。
    if output_path.is_file() {
        return Some(PreparedSquare {
            palette: artwork::palette_from_image_bytes(&std::fs::read(&output_path).ok()?)
                .unwrap_or(Palette::NEUTRAL_DARK),
            path: output_path,
            cover_path,
        });
    }

    let image = image::load_from_memory(&cover.bytes).ok()?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return None;
    }
    let side = width.min(height);
    let square = image.crop_imm((width - side) / 2, (height - side) / 2, side, side);
    let small = square.thumbnail(size, size);
    // 用裁好的方形小图算色，避免横图/竖图的边缘留白影响取色。
    let rgba = small.to_rgba8();
    let palette = artwork::palette_from_rgba(rgba.as_raw(), small.width(), small.height());
    let mut output = std::io::Cursor::new(Vec::new());
    small.write_to(&mut output, image::ImageFormat::Png).ok()?;
    if write_atomic(&output_path, output.get_ref()).is_err() && !output_path.is_file() {
        return None;
    }
    Some(PreparedSquare {
        path: output_path,
        palette,
        cover_path,
    })
}

fn cache_dir() -> Option<PathBuf> {
    Some(sodam_core::app_data_root().join("cache").join("artwork"))
}

fn extension_for(bytes: &[u8], mime: Option<&str>) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1A\n") {
        "png"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "webp"
    } else if bytes.starts_with(b"GIF8") {
        "gif"
    } else if matches!(mime, Some("image/png")) {
        "png"
    } else if matches!(mime, Some("image/webp")) {
        "webp"
    } else {
        "jpg"
    }
}

/// 原子写入：同目录临时文件 + rename，避免并发/崩溃留下半截缓存。
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // 同一进程内多个后台任务可能撞到同一首歌；用纳秒时间戳把并发写分开。
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp = path.with_extension(format!("tmp-{}-{stamp}", std::process::id()));
    std::fs::write(&temp, bytes)?;
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_prefers_magic_bytes_over_mime() {
        assert_eq!(
            extension_for(b"\x89PNG\r\n\x1A\nrest", Some("image/jpeg")),
            "png"
        );
        assert_eq!(extension_for(b"RIFF\0\0\0\0WEBPrest", None), "webp");
        assert_eq!(extension_for(b"\xFF\xD8\xFFrest", None), "jpg");
        assert_eq!(extension_for(b"unknown", Some("image/png")), "png");
        assert_eq!(extension_for(b"unknown", None), "jpg");
    }
}
