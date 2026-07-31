pub struct CrossPlatformTarget;

impl CrossPlatformTarget {
    pub fn print_target_info() {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        tracing::info!("[Cross-Platform Native Build] Target Architecture: {}-{} (Support: Windows 10/11, macOS Apple Silicon, Linux, iOS, Android)", os, arch);
    }
}
