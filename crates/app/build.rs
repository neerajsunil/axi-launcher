fn main() {
    if std::env::var_os("CARGO_FEATURE_DEV").is_some() {
        // Tells the Slint compiler to generate code that reloads .slint files at runtime.
        unsafe { std::env::set_var("SLINT_LIVE_PREVIEW", "1") };
    }
    // The UI is built from our own components; pinning a style keeps any
    // std-widgets fallback identical on every platform.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).unwrap();

    // Explorer and taskbar icon for the .exe.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/axi.ico")
            .set("ProductName", "Axi Launcher")
            .set("FileDescription", "Axi Launcher")
            .compile()
            .unwrap();
    }
}
