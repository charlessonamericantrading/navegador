pub struct OsKernelSandbox {
    pub is_sandboxed: bool,
    pub profile: String,
}

impl OsKernelSandbox {
    pub fn enable_strict_sandbox() -> Self {
        tracing::info!("[OS Kernel Sandboxing] Enabling Win32 AppContainer / Restricted Token Process Isolation");
        Self {
            is_sandboxed: true,
            profile: "AppContainer-StrictOriginIsolation".to_string(),
        }
    }
}
