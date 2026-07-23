use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use byteburrow_plugin_api::{
    ClassifierPlugin, FileContext, PluginConfig, PluginConstructor, PluginDestructor,
    PLUGIN_CONSTRUCTOR_SYMBOL, PLUGIN_DESTRUCTOR_SYMBOL,
};
use libloading::{Library, Symbol};
use tracing::{error, info, warn};

mod guard;
mod merge;
use guard::{check_api_version, run_classify, PluginOutcome};
pub use merge::MergedClassification;

const MAX_PASSES: usize = 10;

/// A loaded plugin plus its backing library handle.
///
/// The FFI boundary is not C-stable (a `dyn ClassifierPlugin` fat pointer, see
/// ADR 0006); this host hardens against the coupling by owning the constructed
/// pointer through its whole lifecycle and freeing it via the plugin's own
/// destructor on drop.
struct LoadedPlugin {
    /// Held in `ManuallyDrop` because the plugin — not the host allocator —
    /// must free the box (see the `Drop` impl below).
    plugin: ManuallyDrop<Box<dyn ClassifierPlugin>>,
    /// The plugin's destructor symbol, or `None` for pre-0.3 plugins.
    destructor: Option<PluginDestructor>,
    /// Set when the plugin panics in `classify`; a panic poisons any mutex it
    /// holds, so it is skipped for the rest of the process instead of
    /// re-panicking on every subsequent file.
    disabled: AtomicBool,
    /// Kept last so it unloads *after* the plugin box is dropped.
    _library: Library,
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        // Reclaim the box without dropping it here, then hand the raw pointer
        // back to the plugin so it frees with its own allocator (or fall back
        // to a host-side drop for pre-0.3 plugins).
        // SAFETY: `plugin` is taken exactly once, here in `Drop`.
        let raw = Box::into_raw(unsafe { ManuallyDrop::take(&mut self.plugin) });
        match self.destructor {
            // SAFETY: `raw` came from this library's constructor, freed once.
            Some(destroy) => unsafe { destroy(raw) },
            // SAFETY: same-rustc/allocator assumption of the contract; freed once.
            None => unsafe { drop(Box::from_raw(raw)) },
        }
        // `_library` unloads after this (fields drop after `Drop::drop`), so the
        // plugin's code stays mapped while its destructor runs.
    }
}

/// Owns all loaded plugins and dispatches classification calls.
pub struct PluginRegistry {
    plugins: Vec<LoadedPlugin>,
}

impl PluginRegistry {
    /// Scan `dir` for `*.so` files, load each, version-check, and init.
    pub fn load_from_directory(dir: &Path, config: &PluginConfig) -> Self {
        let mut plugins = Vec::new();

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                warn!(path = %dir.display(), error = %e, "Cannot read plugin directory");
                return Self { plugins };
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("so") {
                continue;
            }

            match Self::load_one(&path, config) {
                Ok(loaded) => {
                    info!(
                        name = loaded.plugin.name(),
                        version = loaded.plugin.version(),
                        path = %path.display(),
                        "Plugin loaded"
                    );
                    plugins.push(loaded);
                }
                Err(e) => {
                    error!(path = %path.display(), error = %e, "Failed to load plugin");
                }
            }
        }

        info!(count = plugins.len(), "Plugins loaded");
        Self { plugins }
    }

    fn load_one(path: &Path, config: &PluginConfig) -> anyhow::Result<LoadedPlugin> {
        // SAFETY: We trust .so files placed in the configured plugin directory.
        let library = unsafe { Library::new(path)? };

        let raw = {
            let constructor: Symbol<PluginConstructor> =
                unsafe { library.get(PLUGIN_CONSTRUCTOR_SYMBOL)? };
            // A panic must not unwind across the `extern "C"` boundary (UB);
            // trap it and fail the load instead.
            catch_unwind(AssertUnwindSafe(|| unsafe { constructor() }))
                .map_err(|_| anyhow::anyhow!("plugin constructor panicked"))?
        };
        if raw.is_null() {
            anyhow::bail!("Plugin constructor returned null");
        }

        // Prefer the plugin's own destructor (0.3+); warn and fall back otherwise.
        let destructor: Option<PluginDestructor> = unsafe {
            library
                .get::<PluginDestructor>(PLUGIN_DESTRUCTOR_SYMBOL)
                .ok()
                .map(|s| *s)
        };
        if destructor.is_none() {
            warn!(
                path = %path.display(),
                "Plugin exports no destructor symbol; host will free it (rebuild with declare_plugin!)"
            );
        }

        // From here the pointer is owned by `loaded`; any early return frees it
        // through the plugin's destructor via `LoadedPlugin`'s `Drop`.
        // SAFETY: `raw` is non-null and came from this library's constructor.
        let mut loaded = LoadedPlugin {
            plugin: ManuallyDrop::new(unsafe { Box::from_raw(raw) }),
            destructor,
            disabled: AtomicBool::new(false),
            _library: library,
        };

        let (major, minor) = catch_unwind(AssertUnwindSafe(|| loaded.plugin.api_version()))
            .map_err(|_| anyhow::anyhow!("plugin api_version() panicked"))?;
        if let Err(e) = check_api_version(major, minor) {
            anyhow::bail!("{}: {}", loaded.plugin.name(), e);
        }

        catch_unwind(AssertUnwindSafe(|| loaded.plugin.init(config)))
            .map_err(|_| anyhow::anyhow!("plugin init panicked"))?
            .map_err(|e| anyhow::anyhow!("init failed: {}", e))?;

        Ok(loaded)
    }

    /// Run all applicable plugins on a file using multi-pass execution.
    ///
    /// Plugins declare dependencies via `custom_requires()`.
    /// The host runs plugins in iterative passes until no new plugins become eligible.
    pub fn classify_file(&self, ctx: &FileContext) -> MergedClassification {
        let mut merged = MergedClassification::default();
        let mut ran = vec![false; self.plugins.len()];
        let mut current_custom: HashMap<String, serde_json::Value> = ctx.custom.clone();

        for pass in 0..MAX_PASSES {
            let mut made_progress = false;

            for (i, loaded) in self.plugins.iter().enumerate() {
                if ran[i] {
                    continue;
                }
                // A plugin disabled by an earlier panic is never called again.
                if loaded.disabled.load(Ordering::Relaxed) {
                    ran[i] = true;
                    continue;
                }

                // Check MIME interest
                let interests = loaded.plugin.mime_interests();
                if !interests.is_empty()
                    && !interests
                        .iter()
                        .any(|prefix| ctx.mime_type.starts_with(prefix))
                {
                    ran[i] = true; // won't ever match, skip in future passes
                    continue;
                }

                // Check custom metadata requirements
                let required_custom = loaded.plugin.custom_requires();
                if !required_custom
                    .iter()
                    .all(|key| current_custom.contains_key(*key))
                {
                    continue; // might become eligible in a later pass
                }

                // Build updated context for this plugin
                let plugin_ctx = FileContext {
                    path: ctx.path,
                    full_path: ctx.full_path,
                    data: ctx.data,
                    mime_type: ctx.mime_type,
                    size: ctx.size,
                    custom: &current_custom,
                };

                match run_classify(&**loaded.plugin, &plugin_ctx) {
                    PluginOutcome::Classified(Some(result)) => {
                        info!(
                            plugin = loaded.plugin.name(),
                            pass,
                            keywords = ?result.keywords,
                            "Plugin classification"
                        );
                        // Update accumulated state for next plugins
                        for (k, v) in &result.custom {
                            current_custom.insert(k.clone(), v.clone());
                        }
                        merged.absorb(result);
                        made_progress = true;
                    }
                    PluginOutcome::Classified(None) => {}
                    PluginOutcome::Failed(e) => {
                        error!(
                            plugin = loaded.plugin.name(),
                            error = %e,
                            "Plugin classify error"
                        );
                    }
                    PluginOutcome::Panicked => {
                        error!(
                            plugin = loaded.plugin.name(),
                            "Plugin panicked during classify; disabling it for the rest of this process"
                        );
                        loaded.disabled.store(true, Ordering::Relaxed);
                    }
                }

                ran[i] = true;
            }

            if !made_progress {
                break;
            }
        }

        merged
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Log a startup summary of all loaded plugins.
    pub fn log_summary(&self) {
        if self.plugins.is_empty() {
            info!("No plugins loaded");
            return;
        }

        info!("Loaded {} plugin(s):", self.plugins.len());
        for loaded in &self.plugins {
            let p = &loaded.plugin;
            let mime = p.mime_interests();
            let mime_str = if mime.is_empty() {
                "*".to_string()
            } else {
                mime.join(", ")
            };
            let custom_req = p.custom_requires();
            let deps = if custom_req.is_empty() {
                "none".to_string()
            } else {
                custom_req.join(", ")
            };
            info!(
                "  - {} v{} (mime: {}, requires: {})",
                p.name(),
                p.version(),
                mime_str,
                deps,
            );
        }
    }
}
