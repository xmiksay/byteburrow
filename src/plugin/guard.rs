//! Panic- and version-guarding around the (non-C-stable) plugin FFI boundary.
//!
//! A panic must never unwind across the `extern "C"` calls into a plugin (that
//! is UB / a process abort), and a plugin built against an incompatible API
//! version must be rejected at load. See ADR 0006.

use std::panic::{catch_unwind, AssertUnwindSafe};

use byteburrow_plugin_api::{
    ClassificationResult, ClassifierPlugin, FileContext, API_VERSION_MAJOR, API_VERSION_MINOR,
};

/// Outcome of a single guarded `classify` call.
pub(super) enum PluginOutcome {
    Classified(Option<ClassificationResult>),
    /// The plugin returned `Err`.
    Failed(String),
    /// The plugin panicked (caught before it unwound across the FFI boundary).
    Panicked,
}

/// Call `plugin.classify` under `catch_unwind` so a panic becomes a recoverable
/// outcome instead of unwinding across the `extern "C"` boundary.
pub(super) fn run_classify(plugin: &dyn ClassifierPlugin, ctx: &FileContext) -> PluginOutcome {
    match catch_unwind(AssertUnwindSafe(|| plugin.classify(ctx))) {
        Ok(Ok(result)) => PluginOutcome::Classified(result),
        Ok(Err(e)) => PluginOutcome::Failed(e),
        Err(_) => PluginOutcome::Panicked,
    }
}

/// Gate a plugin's declared API version against the host's.
///
/// Major must match exactly; the plugin's minor must be `<=` the host's, since
/// the host only guarantees features up to its own minor.
pub(super) fn check_api_version(major: u32, minor: u32) -> Result<(), String> {
    if major != API_VERSION_MAJOR {
        return Err(format!(
            "API major mismatch: plugin built against {major}.x, host provides \
             {API_VERSION_MAJOR}.{API_VERSION_MINOR}"
        ));
    }
    if minor > API_VERSION_MINOR {
        return Err(format!(
            "API minor too new: plugin built against {major}.{minor}, host provides \
             {API_VERSION_MAJOR}.{API_VERSION_MINOR}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteburrow_plugin_api::PluginConfig;
    use std::collections::HashMap;
    use std::path::Path;

    enum Behavior {
        Ok,
        Err,
        Panic,
    }

    struct FakePlugin(Behavior);

    impl ClassifierPlugin for FakePlugin {
        fn name(&self) -> &str {
            "Fake"
        }
        fn version(&self) -> &str {
            "0.0.0"
        }
        fn mime_interests(&self) -> &[&str] {
            &[]
        }
        fn init(&mut self, _config: &PluginConfig) -> Result<(), String> {
            Ok(())
        }
        fn classify(&self, _ctx: &FileContext) -> Result<Option<ClassificationResult>, String> {
            match self.0 {
                Behavior::Ok => Ok(Some(ClassificationResult::default())),
                Behavior::Err => Err("boom".to_string()),
                Behavior::Panic => panic!("plugin exploded"),
            }
        }
    }

    fn with_ctx<R>(f: impl FnOnce(&FileContext) -> R) -> R {
        let custom = HashMap::new();
        let ctx = FileContext {
            path: "test.txt",
            full_path: Path::new("/tmp/test"),
            data: &[],
            mime_type: "text/plain",
            size: 0,
            custom: &custom,
        };
        f(&ctx)
    }

    #[test]
    fn run_classify_returns_result_on_ok() {
        let plugin = FakePlugin(Behavior::Ok);
        let outcome = with_ctx(|ctx| run_classify(&plugin, ctx));
        assert!(matches!(outcome, PluginOutcome::Classified(Some(_))));
    }

    #[test]
    fn run_classify_reports_error_string() {
        let plugin = FakePlugin(Behavior::Err);
        let outcome = with_ctx(|ctx| run_classify(&plugin, ctx));
        assert!(matches!(outcome, PluginOutcome::Failed(e) if e == "boom"));
    }

    #[test]
    fn run_classify_traps_panic_instead_of_aborting() {
        // Silence the default panic hook so the intentional panic below doesn't
        // spam test output; restore it afterward.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let plugin = FakePlugin(Behavior::Panic);
        let outcome = with_ctx(|ctx| run_classify(&plugin, ctx));
        std::panic::set_hook(prev);
        assert!(matches!(outcome, PluginOutcome::Panicked));
    }

    #[test]
    fn check_api_version_accepts_exact_match() {
        assert!(check_api_version(API_VERSION_MAJOR, API_VERSION_MINOR).is_ok());
    }

    #[test]
    fn check_api_version_accepts_older_minor() {
        assert!(check_api_version(API_VERSION_MAJOR, 0).is_ok());
    }

    #[test]
    fn check_api_version_rejects_major_mismatch() {
        assert!(check_api_version(API_VERSION_MAJOR + 1, 0).is_err());
    }

    #[test]
    fn check_api_version_rejects_newer_minor() {
        assert!(check_api_version(API_VERSION_MAJOR, API_VERSION_MINOR + 1).is_err());
    }
}
