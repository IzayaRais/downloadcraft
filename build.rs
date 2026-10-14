fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "Download Craft — Download Manager");
        res.set("ProductName", "Download Craft");
        if let Err(e) = res.compile() {
            eprintln!("winresource failed: {e:#}");
        }
    }
}
