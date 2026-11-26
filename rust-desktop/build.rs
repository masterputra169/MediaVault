fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "MediaVault");
        res.set("FileDescription", "MediaVault — 100% Pure-Rust Media Downloader");
        res.set("LegalCopyright", "MIT License");
        if let Err(err) = res.compile() {
            println!("cargo:warning=winresource skipped: {err}");
        }
    }
}
