pub fn package_root() -> std::path::PathBuf {
    std::env::var_os("TERRA_PLUGIN_SOURCE")
        .map(std::path::PathBuf::from)
        .expect("plugin integration requires TERRA_PLUGIN_SOURCE=/path/to/plugins/packages")
}
