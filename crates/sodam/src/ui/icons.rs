//! 图标：内置 Lucide 线性 SVG（ISC 许可），统一从 `path()` 引用。
//!
//! 全部同一套描边、同一个尺寸档（`theme::ICON` 等），不再用 emoji 或文字符号 ——
//! emoji 的字形、基线、颜色由字体决定，大小和对齐都没法控。

use std::borrow::Cow;

use gpui::{AssetSource, DevicePixels, Result, SharedString, SvgSize};

const BRAND_LOGO_SVG: &[u8] = include_bytes!("../../assets/brand/sodam-logo.svg");

pub(crate) fn brand_logo(cx: &gpui::App, side: i32) -> Option<std::sync::Arc<gpui::RenderImage>> {
    let renderer = cx.svg_renderer();
    let svg = renderer.parse_svg(BRAND_LOGO_SVG).ok()?;
    let size = gpui::size(DevicePixels(side), DevicePixels(side));
    renderer.render_parsed(&svg, SvgSize::ExactSize(size)).ok()
}

const IMAGES: &[(&str, &[u8])] = &[(
    "sodam-logo",
    include_bytes!("../../assets/brand/sodam-logo.png"),
)];

macro_rules! icon_set {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[
            $(
                (
                    $name,
                    include_bytes!(concat!("../../assets/icons/", $name, ".svg")),
                ),
            )*
        ];

        /// 注册给 GPUI 的资产源（`Application::new().with_assets(Assets)`）。
        pub struct Assets;

        impl AssetSource for Assets {
            fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
                if path == "icons/sodam-logo.svg" {
                    return Ok(Some(Cow::Borrowed(BRAND_LOGO_SVG)));
                }

                let name = path.strip_prefix("icons/").unwrap_or(path);
                let name = name
                    .strip_suffix(".svg")
                    .or_else(|| name.strip_suffix(".png"))
                    .unwrap_or(name);
                let table = if path.ends_with(".png") { IMAGES } else { ICONS };
                if let Some(bytes) = table
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, bytes)| Cow::Borrowed(*bytes))
                {
                    return Ok(Some(bytes));
                }
                Ok(None)
            }

            fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
                Ok(ICONS
                    .iter()
                    .map(|(name, _)| SharedString::from(format!("icons/{name}.svg")))
                    .collect())
            }
        }
    };
}

icon_set!(
    "list-music",
    "folder-music",
    "play",
    "pause",
    "skip-back",
    "skip-forward",
    "repeat",
    "repeat-1",
    "shuffle",
    "volume-2",
    "volume-x",
    "list-end",
    "music",
    "spinner",
    "chevron-left",
    "chevron-right-wide",
    "refresh",
    "folder-plus",
    "expand",
    "taskbar",
    "collapse",
    "search",
    "x",
    "windows-minimize",
    "windows-maximize",
    "windows-restore",
    "windows-close",
    "disc-3",
    "sun",
    "moon",
    "settings",
);

/// 图标的资产路径，喂给 `svg().path(icons::path("play"))`。
pub fn path(name: &str) -> SharedString {
    SharedString::from(format!("icons/{name}.svg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_source_loads_sodam_brand_png() {
        let bytes = Assets
            .load("icons/sodam-logo.png")
            .expect("brand asset load")
            .expect("brand PNG bytes");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn asset_source_loads_multicolor_sodam_brand_svg() {
        let bytes = Assets
            .load("icons/sodam-logo.svg")
            .expect("brand SVG asset load")
            .expect("brand SVG bytes");
        assert!(bytes.starts_with(b"<?xml"));
    }

    /// 顶栏「选择文件夹」图标；未注册时 svg 会静默不渲染。
    #[test]
    fn folder_action_icon_is_registered() {
        let bytes = Assets
            .load(&path("folder-plus"))
            .expect("folder-plus asset load")
            .expect("folder-plus SVG bytes");
        assert!(bytes.starts_with(b"<svg"));
    }

    /// 自绘标题栏的窗口控制图标必须全部注册，否则会静默丢图标。
    #[test]
    fn window_control_icons_are_registered() {
        for name in [
            "windows-minimize",
            "windows-maximize",
            "windows-restore",
            "windows-close",
        ] {
            let bytes = Assets
                .load(&path(name))
                .unwrap_or_else(|error| panic!("{name} asset load: {error}"))
                .unwrap_or_else(|| panic!("{name} missing"));
            assert!(bytes.starts_with(b"<svg"), "{name} 不是 SVG");
        }
    }
}
