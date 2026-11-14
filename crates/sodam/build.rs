//! Windows：嵌入应用图标，保留 GPUI 的 DPI manifest。
//! 非 Windows 目标直接跳过。

fn main() {
    use std::io::Write as _;

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        // MinGW GCC 会自动链接自己的 manifest，与 GPUI 的资源冲突。
        let specs = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("assets/mingw-endfile.specs");
        writeln!(
            std::io::stdout().lock(),
            "cargo:rustc-link-arg=-specs={}",
            specs.display()
        )
        .expect("输出 MinGW 链接参数失败");
    }

    writeln!(
        std::io::stdout().lock(),
        "cargo:rerun-if-changed=assets/brand/sodam.ico\ncargo:rerun-if-changed=assets/mingw-endfile.specs"
    )
    .expect("输出 Windows 资源依赖失败");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/brand/sodam.ico");
    res.compile().expect("编译 Windows 应用资源失败");
}
