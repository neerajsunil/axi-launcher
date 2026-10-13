fn main() {
    if std::env::var_os("CARGO_FEATURE_DEV").is_some() {
        // Tells the Slint compiler to generate code that reloads .slint files at runtime.
        unsafe { std::env::set_var("SLINT_LIVE_PREVIEW", "1") };
    }
    slint_build::compile("ui/app.slint").unwrap();
}
