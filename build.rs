fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let cfg = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
        slint_build::compile_with_config("ui/app.slint", cfg).unwrap();
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") && cfg!(windows) {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/blitz.ico")
                .set("ProductName", "Blitz 疾风加速")
                .set("FileDescription", "Blitz 疾风加速")
                .set_language(0x0804);
            res.compile().expect("resource");
        }
    }
}
