// Embeds the raccoon-girl icon into wuxia-local.exe (Explorer, taskbar, Alt+Tab).
// A failure here only loses the icon, never the build.
fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=иконка не встроена: {e}");
        }
    }
}
