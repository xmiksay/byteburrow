use std::collections::HashMap;
use std::path::Path;

use byteburrow_plugin_api::{
    ClassificationResult, ClassifierPlugin, FileContext, PluginConfig, PluginConstructor,
    API_VERSION_MAJOR, PLUGIN_CONSTRUCTOR_SYMBOL,
};
use libloading::{Library, Symbol};
use tracing::{error, info, warn};

const MAX_PASSES: usize = 10;

/// A loaded plugin plus its backing library handle.
struct LoadedPlugin {
    plugin: Box<dyn ClassifierPlugin>,
    _library: Library,
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

        let constructor: Symbol<PluginConstructor> =
            unsafe { library.get(PLUGIN_CONSTRUCTOR_SYMBOL)? };

        let raw = unsafe { constructor() };
        if raw.is_null() {
            anyhow::bail!("Plugin constructor returned null");
        }
        let mut plugin = unsafe { Box::from_raw(raw) };

        // Version gate
        let (major, _minor) = plugin.api_version();
        if major != API_VERSION_MAJOR {
            anyhow::bail!(
                "API version mismatch: plugin {} has major {}, host expects {}",
                plugin.name(),
                major,
                API_VERSION_MAJOR
            );
        }

        plugin
            .init(config)
            .map_err(|e| anyhow::anyhow!("init failed: {}", e))?;

        Ok(LoadedPlugin {
            plugin,
            _library: library,
        })
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

                match loaded.plugin.classify(&plugin_ctx) {
                    Ok(Some(result)) => {
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
                    Ok(None) => {}
                    Err(e) => {
                        error!(
                            plugin = loaded.plugin.name(),
                            error = %e,
                            "Plugin classify error"
                        );
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

/// Aggregated results from all plugins for a single file.
#[derive(Debug, Default, Clone)]
pub struct MergedClassification {
    pub keywords: Vec<String>,
    pub custom: serde_json::Map<String, serde_json::Value>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub date_unix: Option<i64>,
}

impl MergedClassification {
    fn absorb(&mut self, r: ClassificationResult) {
        self.keywords.extend(r.keywords);
        for (k, v) in r.custom {
            self.custom.insert(k, v);
        }
        if r.latitude.is_some() {
            self.latitude = r.latitude;
        }
        if r.longitude.is_some() {
            self.longitude = r.longitude;
        }
        if r.date_unix.is_some() {
            self.date_unix = r.date_unix;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_with(
        keywords: &[&str],
        custom: &[(&str, serde_json::Value)],
    ) -> ClassificationResult {
        ClassificationResult {
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            custom: custom
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn absorb_accumulates_keywords_across_calls() {
        let mut merged = MergedClassification::default();
        merged.absorb(result_with(&["cat", "sunset"], &[]));
        merged.absorb(result_with(&["beach"], &[]));

        assert_eq!(merged.keywords, vec!["cat", "sunset", "beach"]);
    }

    #[test]
    fn absorb_merges_custom_metadata_by_key() {
        let mut merged = MergedClassification::default();
        merged.absorb(result_with(
            &[],
            &[("exif", serde_json::json!({"iso": 100}))],
        ));
        merged.absorb(result_with(&[], &[("faces", serde_json::json!(2))]));

        assert_eq!(merged.custom.len(), 2);
        assert_eq!(merged.custom["exif"], serde_json::json!({"iso": 100}));
        assert_eq!(merged.custom["faces"], serde_json::json!(2));
    }

    #[test]
    fn absorb_later_custom_value_overwrites_earlier_for_same_key() {
        let mut merged = MergedClassification::default();
        merged.absorb(result_with(
            &[],
            &[("exif", serde_json::json!({"iso": 100}))],
        ));
        merged.absorb(result_with(
            &[],
            &[("exif", serde_json::json!({"iso": 200}))],
        ));

        assert_eq!(merged.custom["exif"], serde_json::json!({"iso": 200}));
    }

    #[test]
    fn absorb_sets_geo_and_date_fields() {
        let mut merged = MergedClassification::default();
        merged.absorb(ClassificationResult {
            latitude: Some(50.1),
            longitude: Some(14.4),
            date_unix: Some(1_700_000_000),
            ..Default::default()
        });

        assert_eq!(merged.latitude, Some(50.1));
        assert_eq!(merged.longitude, Some(14.4));
        assert_eq!(merged.date_unix, Some(1_700_000_000));
    }

    #[test]
    fn absorb_keeps_previous_geo_when_later_plugin_has_none() {
        let mut merged = MergedClassification::default();
        merged.absorb(ClassificationResult {
            latitude: Some(50.1),
            longitude: Some(14.4),
            ..Default::default()
        });
        merged.absorb(ClassificationResult::default());

        assert_eq!(merged.latitude, Some(50.1));
        assert_eq!(merged.longitude, Some(14.4));
    }
}
