//! Likely-secret path heuristics.
//!
//! Discovery tools omit these paths. An explicit `read_file` still works but
//! requires per-call approval. Template / example env files stay readable.

use std::path::Path;

/// True when `rel` (project-relative, forward slashes preferred) looks like a
/// secret or credential file that must not appear in discovery results.
pub fn is_sensitive_path(rel: &str) -> bool {
    let name = Path::new(rel)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if name.is_empty() {
        return false;
    }
    let lower = name.to_ascii_lowercase();

    // Explicit allowlist for template env files.
    if matches!(
        lower.as_str(),
        ".env.example" | ".env.sample" | ".env.template" | ".env.sample.local"
    ) {
        return false;
    }

    if lower == ".env" || lower.starts_with(".env.") {
        return true;
    }

    if lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower.ends_with(".keystore")
    {
        return true;
    }

    if matches!(
        lower.as_str(),
        "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
            | "id_rsa_old"
            | ".netrc"
            | "credentials.json"
            | "credentials.xml"
            | "auth.json"
            | "service-account.json"
    ) {
        return true;
    }

    if lower.ends_with(".credentials.json") || lower.ends_with("-credentials.json") {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_env_and_keys_sensitive() {
        assert!(is_sensitive_path(".env"));
        assert!(is_sensitive_path("config/.env.local"));
        assert!(is_sensitive_path("certs/server.pem"));
        assert!(is_sensitive_path("id_rsa"));
        assert!(is_sensitive_path("secrets/credentials.json"));
    }

    #[test]
    fn allows_env_example() {
        assert!(!is_sensitive_path(".env.example"));
        assert!(!is_sensitive_path("deploy/.env.sample"));
        assert!(!is_sensitive_path("src/main.rs"));
        assert!(!is_sensitive_path("id_rsa.pub"));
    }
}
