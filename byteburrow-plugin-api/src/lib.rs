use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// API version constants. Host and plugin must share the same MAJOR version.
pub const API_VERSION_MAJOR: u32 = 0;
pub const API_VERSION_MINOR: u32 = 2;

// ── File context passed to plugins ───────────────────────────────

/// Read-only context the host provides to each `classify` call.
pub struct FileContext<'a> {
    /// Relative path within the storage (e.g. "photos/2024/img_001.jpg").
    pub path: &'a str,
    /// Full filesystem path for direct I/O.
    pub full_path: &'a Path,
    /// File contents. May be empty if the plugin declared `needs_file_data() == false`.
    pub data: &'a [u8],
    /// MIME type as determined by the host (extension + magic bytes).
    pub mime_type: &'a str,
    /// File size in bytes.
    pub size: u64,
    /// Accumulated custom metadata from previous plugin passes (enables chaining).
    pub custom: &'a HashMap<String, serde_json::Value>,
}

// ── Classification result ────────────────────────────────────────

/// What a plugin returns after inspecting a file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassificationResult {
    /// Keywords to merge into the meta record.
    pub keywords: Vec<String>,
    /// Structured metadata stored in `meta.custom` under the plugin's namespace key.
    /// Key = namespace (e.g. "exif"), Value = arbitrary JSON.
    pub custom: HashMap<String, serde_json::Value>,
    /// Optional latitude (for geo-aware plugins).
    pub latitude: Option<f64>,
    /// Optional longitude (for geo-aware plugins).
    pub longitude: Option<f64>,
    /// Optional date as Unix timestamp (seconds, UTC).
    /// Using i64 to avoid chrono dependency in the API crate.
    pub date_unix: Option<i64>,
}

// ── Plugin config ────────────────────────────────────────────────

/// Key-value configuration the host passes during `init`.
pub type PluginConfig = HashMap<String, String>;

// ── The trait ────────────────────────────────────────────────────

/// Every classifier plugin implements this trait.
///
/// # ABI Safety
///
/// The trait uses safe Rust types. The FFI boundary is a single `extern "C"`
/// constructor that returns `Box<dyn ClassifierPlugin>`. Both host and plugin
/// must be compiled with the same Rust compiler version and the same
/// `byteburrow-plugin-api` crate version.
pub trait ClassifierPlugin: Send + Sync {
    /// Human-readable name (e.g. "EXIF Photo Classifier").
    fn name(&self) -> &str;

    /// Semver string for the plugin itself (e.g. "0.1.0").
    fn version(&self) -> &str;

    /// API version the plugin was built against.
    /// Host checks `major == API_VERSION_MAJOR`.
    fn api_version(&self) -> (u32, u32) {
        (API_VERSION_MAJOR, API_VERSION_MINOR)
    }

    /// MIME type prefixes this plugin cares about.
    /// Return `&["image/"]` to receive only image files.
    /// Return `&[]` (empty) to receive ALL files.
    fn mime_interests(&self) -> &[&str];

    /// Custom metadata keys that must exist before this plugin runs.
    /// Used for plugin chaining (e.g. face recognition requires `"faces"` key).
    /// Default: no requirements.
    fn custom_requires(&self) -> &[&str] {
        &[]
    }

    /// Whether the plugin needs the full file data loaded into memory.
    /// Return `false` if the plugin only needs the path to do its own I/O
    /// (e.g. shelling out to ffprobe). Default: `true`.
    fn needs_file_data(&self) -> bool {
        true
    }

    /// Called once after loading with host-provided configuration.
    fn init(&mut self, config: &PluginConfig) -> Result<(), String>;

    /// Classify a file. Called on a blocking thread (not async).
    /// Return `Ok(None)` if the plugin has nothing to say about this file.
    fn classify(&self, ctx: &FileContext) -> Result<Option<ClassificationResult>, String>;
}

// ── FFI constructor ──────────────────────────────────────────────

/// The symbol name every plugin .so must export.
pub const PLUGIN_CONSTRUCTOR_SYMBOL: &[u8] = b"byteburrow_create_plugin";

/// Signature of the constructor function.
///
/// Note: `dyn ClassifierPlugin` is not C-FFI-safe. This is intentional — both host
/// and plugin must be compiled with the same Rust compiler version. The `extern "C"`
/// is used only for a stable calling convention, not for cross-language interop.
#[allow(improper_ctypes_definitions)]
pub type PluginConstructor = unsafe extern "C" fn() -> *mut dyn ClassifierPlugin;
