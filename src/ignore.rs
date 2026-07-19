/// Parse a comma-separated ignore patterns string into a Vec of trimmed, non-empty patterns.
pub fn parse_patterns(patterns: &str) -> Vec<String> {
    patterns
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Check if a path should be ignored based on patterns.
/// Matches individual path components (split by '/') against the pattern list.
pub fn is_ignored(path: &str, patterns: &[String]) -> bool {
    path.split('/')
        .any(|component| patterns.iter().any(|pattern| component == pattern.as_str()))
}
