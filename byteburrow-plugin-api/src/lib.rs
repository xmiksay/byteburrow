use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// API version constants.
///
/// Host and plugin must share the same MAJOR version; the plugin's MINOR must be
/// `<=` the host's (the host only guarantees it provides features up to its own
/// minor). See `check_api_version` in the host's `src/plugin/mod.rs`.
pub const API_VERSION_MAJOR: u32 = 0;
pub const API_VERSION_MINOR: u32 = 3;

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
/// The trait uses safe Rust types. The FFI boundary is an `extern "C"`
/// constructor/destructor pair (see [`declare_plugin!`]) that passes a
/// `Box<dyn ClassifierPlugin>` fat pointer. This is **not** a C-stable ABI:
/// both host and plugin must be compiled with the same Rust compiler version
/// and the same `byteburrow-plugin-api` crate version. See ADR 0006 for why
/// this coupling is accepted and how the host hardens against it (version
/// gating, `catch_unwind`, plugin-side destructor).
///
/// # Panics
///
/// The host wraps every `init`/`classify` call in `catch_unwind` and treats a
/// panicking plugin as failed — a plugin that panics in `classify` is disabled
/// for the rest of the process. Do not rely on unwinding across the boundary
/// for control flow; return `Err(String)` instead.
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

// ── FFI constructor / destructor ─────────────────────────────────

/// The constructor symbol every plugin .so must export.
pub const PLUGIN_CONSTRUCTOR_SYMBOL: &[u8] = b"byteburrow_create_plugin";

/// The destructor symbol every plugin .so should export (added in API 0.3).
///
/// The plugin frees the box with *its own* allocator, which keeps host and
/// plugin from mixing allocators. Plugins built before 0.3 lack this symbol;
/// the host falls back to freeing the box itself (safe only under the
/// same-rustc/same-allocator assumption this contract already requires).
pub const PLUGIN_DESTRUCTOR_SYMBOL: &[u8] = b"byteburrow_destroy_plugin";

/// Signature of the constructor function.
///
/// Note: `dyn ClassifierPlugin` is not C-FFI-safe. This is intentional — both host
/// and plugin must be compiled with the same Rust compiler version. The `extern "C"`
/// is used only for a stable calling convention, not for cross-language interop.
#[allow(improper_ctypes_definitions)]
pub type PluginConstructor = unsafe extern "C" fn() -> *mut dyn ClassifierPlugin;

/// Signature of the destructor function: consumes the pointer returned by the
/// constructor and drops it with the plugin's allocator.
#[allow(improper_ctypes_definitions)]
pub type PluginDestructor = unsafe extern "C" fn(*mut dyn ClassifierPlugin);

/// Export the FFI constructor/destructor pair for a plugin.
///
/// Centralizes the `unsafe` ABI boundary so every plugin exports a matching,
/// correctly-attributed pair instead of copy-pasting it (a wrong signature or a
/// missing `#[no_mangle]` is a load failure or worse). Pass an expression that
/// builds the plugin value:
///
/// ```ignore
/// byteburrow_plugin_api::declare_plugin!(MyPlugin::new());
/// ```
#[macro_export]
macro_rules! declare_plugin {
    ($constructor:expr) => {
        /// Host-called constructor. Returns an owning fat pointer; free it via
        /// `byteburrow_destroy_plugin`.
        #[no_mangle]
        #[allow(improper_ctypes_definitions)]
        pub extern "C" fn byteburrow_create_plugin() -> *mut dyn $crate::ClassifierPlugin {
            let plugin: ::std::boxed::Box<dyn $crate::ClassifierPlugin> =
                ::std::boxed::Box::new($constructor);
            ::std::boxed::Box::into_raw(plugin)
        }

        /// Host-called destructor. Drops a pointer produced by
        /// `byteburrow_create_plugin` using the plugin's own allocator.
        ///
        /// # Safety
        /// `plugin` must be a pointer returned by `byteburrow_create_plugin` from
        /// this same library, dropped at most once.
        #[no_mangle]
        #[allow(improper_ctypes_definitions)]
        pub unsafe extern "C" fn byteburrow_destroy_plugin(
            plugin: *mut dyn $crate::ClassifierPlugin,
        ) {
            if !plugin.is_null() {
                drop(::std::boxed::Box::from_raw(plugin));
            }
        }
    };
}
