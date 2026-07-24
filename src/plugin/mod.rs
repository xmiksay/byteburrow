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

/// One entry in the multi-pass pipeline: a classifier plus its live "disabled"
/// state. Implemented by `LoadedPlugin` in production and by a test double in
/// the unit tests, so the ordering / dependency / `needs_file_data` mechanics
/// can be exercised without loading real `.so` files.
trait PipelineEntry {
    fn classifier(&self) -> &dyn ClassifierPlugin;
    fn is_disabled(&self) -> bool;
    fn disable(&self);
}

impl PipelineEntry for LoadedPlugin {
    fn classifier(&self) -> &dyn ClassifierPlugin {
        &**self.plugin
    }
    fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Relaxed)
    }
    fn disable(&self) {
        self.disabled.store(true, Ordering::Relaxed);
    }
}

/// Deterministic sort key: plugins requiring fewer custom keys first, then by
/// name. Keeps a stable, roughly topological execution order (see the sort in
/// `load_from_directory`).
fn plugin_sort_key(p: &dyn ClassifierPlugin) -> (usize, &str) {
    (p.custom_requires().len(), p.name())
}

/// Whether any non-disabled plugin that could run for `mime_type` needs the
/// full file bytes. Conservative: it ignores `custom_requires` (a plugin whose
/// dependencies are unmet won't actually run), so it may over-read but never
/// under-reads. Empty registry → `false`.
fn any_needs_file_data<E: PipelineEntry>(entries: &[E], mime_type: &str) -> bool {
    entries.iter().any(|e| {
        if e.is_disabled() {
            return false;
        }
        let plugin = e.classifier();
        let interests = plugin.mime_interests();
        let applies =
            interests.is_empty() || interests.iter().any(|prefix| mime_type.starts_with(prefix));
        applies && plugin.needs_file_data()
    })
}

/// Run every eligible plugin over `ctx` using iterative multi-pass execution.
///
/// Entries are visited in their (already deterministic) slice order each pass;
/// a plugin whose `custom_requires` are not yet satisfied is deferred to a
/// later pass, and the loop stops once a pass makes no progress.
fn run_pipeline<E: PipelineEntry>(entries: &[E], ctx: &FileContext) -> MergedClassification {
    let mut merged = MergedClassification::default();
    let mut ran = vec![false; entries.len()];
    let mut current_custom: HashMap<String, serde_json::Value> = ctx.custom.clone();

    for pass in 0..MAX_PASSES {
        let mut made_progress = false;

        for (i, entry) in entries.iter().enumerate() {
            if ran[i] {
                continue;
            }
            // A plugin disabled by an earlier panic is never called again.
            if entry.is_disabled() {
                ran[i] = true;
                continue;
            }
            let plugin = entry.classifier();

            // Check MIME interest
            let interests = plugin.mime_interests();
            if !interests.is_empty()
                && !interests
                    .iter()
                    .any(|prefix| ctx.mime_type.starts_with(prefix))
            {
                ran[i] = true; // won't ever match, skip in future passes
                continue;
            }

            // Check custom metadata requirements
            let required_custom = plugin.custom_requires();
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

            match run_classify(plugin, &plugin_ctx) {
                PluginOutcome::Classified(Some(result)) => {
                    info!(
                        plugin = plugin.name(),
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
                        plugin = plugin.name(),
                        error = %e,
                        "Plugin classify error"
                    );
                }
                PluginOutcome::Panicked => {
                    error!(
                        plugin = plugin.name(),
                        "Plugin panicked during classify; disabling it for the rest of this process"
                    );
                    entry.disable();
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

        // Filesystem read order (`read_dir`) is not stable, which let multi-pass
        // classification results depend on directory listing order. Establish a
        // deterministic, roughly topological order: plugins requiring fewer
        // custom keys (closer to the pipeline roots) run first, ties broken by
        // name. `run_pipeline`'s multi-pass loop then resolves the remaining
        // dependencies, so this only needs to be stable, not perfectly sorted.
        plugins
            .sort_by(|a, b| plugin_sort_key(a.classifier()).cmp(&plugin_sort_key(b.classifier())));

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
        run_pipeline(&self.plugins, ctx)
    }

    /// Whether the host must load the full file bytes before classifying a file
    /// of `mime_type` — i.e. some plugin that could run for it declares
    /// `needs_file_data()`. Lets the caller honor the API contract and skip the
    /// read entirely for plugins that do their own path-based I/O.
    pub fn needs_file_data(&self, mime_type: &str) -> bool {
        any_needs_file_data(&self.plugins, mime_type)
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

#[cfg(test)]
mod tests {
    use super::*;
    use byteburrow_plugin_api::{ClassificationResult, PluginConfig};

    /// Configurable fake classifier: on `classify` it emits one keyword (its
    /// name) and writes each key in `provides` into `custom`.
    struct FakePlugin {
        name: &'static str,
        mime: Vec<&'static str>,
        requires: Vec<&'static str>,
        provides: Vec<&'static str>,
        needs_data: bool,
    }

    impl FakePlugin {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                mime: vec![],
                requires: vec![],
                provides: vec![],
                needs_data: true,
            }
        }
        fn mime(mut self, m: &'static str) -> Self {
            self.mime.push(m);
            self
        }
        fn requires(mut self, k: &'static str) -> Self {
            self.requires.push(k);
            self
        }
        fn provides(mut self, k: &'static str) -> Self {
            self.provides.push(k);
            self
        }
        fn needs_data(mut self, v: bool) -> Self {
            self.needs_data = v;
            self
        }
    }

    impl ClassifierPlugin for FakePlugin {
        fn name(&self) -> &str {
            self.name
        }
        fn version(&self) -> &str {
            "0.0.0"
        }
        fn mime_interests(&self) -> &[&str] {
            &self.mime
        }
        fn custom_requires(&self) -> &[&str] {
            &self.requires
        }
        fn needs_file_data(&self) -> bool {
            self.needs_data
        }
        fn init(&mut self, _config: &PluginConfig) -> Result<(), String> {
            Ok(())
        }
        fn classify(&self, _ctx: &FileContext) -> Result<Option<ClassificationResult>, String> {
            let custom = self
                .provides
                .iter()
                .map(|k| (k.to_string(), serde_json::json!(self.name)))
                .collect();
            Ok(Some(ClassificationResult {
                keywords: vec![self.name.to_string()],
                custom,
                ..Default::default()
            }))
        }
    }

    struct TestEntry {
        plugin: Box<dyn ClassifierPlugin>,
        disabled: AtomicBool,
    }

    impl PipelineEntry for TestEntry {
        fn classifier(&self) -> &dyn ClassifierPlugin {
            &*self.plugin
        }
        fn is_disabled(&self) -> bool {
            self.disabled.load(Ordering::Relaxed)
        }
        fn disable(&self) {
            self.disabled.store(true, Ordering::Relaxed);
        }
    }

    fn entry(p: FakePlugin) -> TestEntry {
        TestEntry {
            plugin: Box::new(p),
            disabled: AtomicBool::new(false),
        }
    }

    /// Sort entries with the same key `load_from_directory` uses.
    fn sorted(mut entries: Vec<TestEntry>) -> Vec<TestEntry> {
        entries
            .sort_by(|a, b| plugin_sort_key(a.classifier()).cmp(&plugin_sort_key(b.classifier())));
        entries
    }

    fn ctx_for<'a>(
        mime: &'a str,
        custom: &'a HashMap<String, serde_json::Value>,
    ) -> FileContext<'a> {
        FileContext {
            path: "test.jpg",
            full_path: Path::new("/tmp/test.jpg"),
            data: &[],
            mime_type: mime,
            size: 0,
            custom,
        }
    }

    #[test]
    fn sort_key_orders_by_dependency_count_then_name() {
        let a = FakePlugin::new("b");
        let b = FakePlugin::new("a");
        let dep = FakePlugin::new("a-dep").requires("x");
        let mut v: Vec<&dyn ClassifierPlugin> = vec![&dep, &a, &b];
        v.sort_by(|x, y| plugin_sort_key(*x).cmp(&plugin_sort_key(*y)));
        let names: Vec<_> = v.iter().map(|p| p.name()).collect();
        assert_eq!(names, vec!["a", "b", "a-dep"]);
    }

    #[test]
    fn pipeline_resolves_dependencies_regardless_of_slice_order() {
        // Consumer is listed *before* its producer; the multi-pass loop must
        // still run it, in a later pass, once "faces" exists.
        let entries = vec![
            entry(
                FakePlugin::new("consumer")
                    .requires("faces")
                    .provides("match"),
            ),
            entry(FakePlugin::new("producer").provides("faces")),
        ];
        let custom = HashMap::new();
        let merged = run_pipeline(&entries, &ctx_for("image/jpeg", &custom));
        assert!(merged.custom.contains_key("faces"), "producer must run");
        assert!(
            merged.custom.contains_key("match"),
            "consumer must run after its dependency is satisfied"
        );
    }

    #[test]
    fn sorted_pipeline_is_order_independent() {
        // Two producers writing keywords, plus a dependent consumer. Whatever
        // the directory listing order, sorting yields the same run order and
        // therefore the same accumulated keywords.
        let build = || {
            vec![
                entry(FakePlugin::new("alpha").provides("a")),
                entry(FakePlugin::new("bravo").provides("b")),
                entry(FakePlugin::new("consumer").requires("a").requires("b")),
            ]
        };
        let forward = run_pipeline(&sorted(build()), &ctx_for("image/jpeg", &HashMap::new()));
        let mut reversed_input = build();
        reversed_input.reverse();
        let reversed = run_pipeline(
            &sorted(reversed_input),
            &ctx_for("image/jpeg", &HashMap::new()),
        );
        assert_eq!(forward.keywords, reversed.keywords);
        assert_eq!(forward.keywords, vec!["alpha", "bravo", "consumer"]);
    }

    #[test]
    fn needs_file_data_true_only_for_applicable_data_plugins() {
        let entries = vec![entry(FakePlugin::new("img").mime("image/"))];
        assert!(any_needs_file_data(&entries, "image/jpeg"));
        // MIME does not match -> plugin won't run -> no read required.
        assert!(!any_needs_file_data(&entries, "text/plain"));
    }

    #[test]
    fn needs_file_data_false_when_only_path_plugins_apply() {
        let entries = vec![entry(FakePlugin::new("probe").needs_data(false))];
        assert!(!any_needs_file_data(&entries, "video/mp4"));
    }

    #[test]
    fn needs_file_data_false_for_empty_registry() {
        let entries: Vec<TestEntry> = vec![];
        assert!(!any_needs_file_data(&entries, "image/jpeg"));
    }

    #[test]
    fn needs_file_data_ignores_disabled_plugins() {
        let entries = vec![entry(FakePlugin::new("img").mime("image/"))];
        entries[0].disable();
        assert!(!any_needs_file_data(&entries, "image/jpeg"));
    }

    // ── pipeline edge cases (GH #43) ───────────────────────────────
    //
    // D2's tests above cover ordering, dependency resolution, and
    // needs_file_data. These add the remaining behaviors the multi-pass loop
    // must guarantee: MIME-prefix skip during execution, unmet-dependency
    // loop termination, and that an Erring plugin neither disables itself nor
    // blocks later plugins in the same pass.

    #[test]
    fn pipeline_skips_plugin_whose_mime_does_not_match() {
        // An image-only plugin contributes nothing for text/plain and is not
        // re-checked in later passes.
        let entries = vec![entry(
            FakePlugin::new("img").mime("image/").provides("saw-me"),
        )];
        let merged = run_pipeline(&entries, &ctx_for("text/plain", &HashMap::new()));
        assert!(
            !merged.custom.contains_key("saw-me"),
            "image-only plugin must not run for text/plain"
        );
    }

    #[test]
    fn pipeline_runs_plugin_whose_mime_matches_prefix() {
        let entries = vec![entry(FakePlugin::new("img").mime("image/").provides("ran"))];
        let merged = run_pipeline(&entries, &ctx_for("image/jpeg", &HashMap::new()));
        assert!(merged.custom.contains_key("ran"));
    }

    #[test]
    fn unmet_dependency_terminates_without_running() {
        // A plugin whose requirement is never produced is skipped and the loop
        // ends (no infinite passes); the producer still runs.
        let entries = vec![
            entry(FakePlugin::new("producer").provides("have")),
            entry(FakePlugin::new("waiter").requires("never")),
        ];
        let merged = run_pipeline(&sorted(entries), &ctx_for("image/jpeg", &HashMap::new()));
        assert!(merged.custom.contains_key("have"), "producer ran");
        assert!(
            !merged.keywords.iter().any(|k| k == "waiter"),
            "waiter never ran (unmet dependency)"
        );
    }

    /// A fake that returns Err on classify, used to verify the host does not
    /// disable on a plain error (only on a panic) and keeps the pass going.
    struct FailingPlugin {
        name: &'static str,
    }

    impl ClassifierPlugin for FailingPlugin {
        fn name(&self) -> &str {
            self.name
        }
        fn version(&self) -> &str {
            "0.0.0"
        }
        fn mime_interests(&self) -> &[&str] {
            &[]
        }
        fn needs_file_data(&self) -> bool {
            false
        }
        fn init(&mut self, _config: &PluginConfig) -> Result<(), String> {
            Ok(())
        }
        fn classify(&self, _ctx: &FileContext) -> Result<Option<ClassificationResult>, String> {
            Err("boom".to_string())
        }
    }

    #[test]
    fn failed_plugin_does_not_disable_and_pass_continues() {
        // A returning-Err plugin must NOT be disabled (only panics do that), and
        // a later plugin in the same pass still runs.
        let entries: Vec<TestEntry> = vec![
            TestEntry {
                plugin: Box::new(FailingPlugin { name: "failer" }),
                disabled: AtomicBool::new(false),
            },
            entry(FakePlugin::new("after").provides("after-ran")),
        ];
        let merged = run_pipeline(&entries, &ctx_for("image/jpeg", &HashMap::new()));
        // The failing plugin is not disabled…
        assert!(!entries[0].disabled.load(Ordering::Relaxed));
        // …and the later plugin still ran.
        assert!(merged.custom.contains_key("after-ran"));
    }
}
