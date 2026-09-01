//! Parsing of the services manifest.
//!
//! # Why webtop reads a file it does not own
//!
//! webtop is a general-purpose machine monitor. It has no business knowing
//! that port 8001 is a model replica or that `gateway` needs PostgreSQL
//! first — that knowledge belongs to the stack being watched. So the stack
//! describes itself and webtop reads it: `--services-manifest <path>`,
//! defaulting to `~/.webtop/services.json`, which the owning stack symlinks
//! into place.
//!
//! The file is JSON, not the stack's own TOML — the stack (macosctl) manages
//! its services across several TOML fragments merged from `conf.d/`, and
//! writes the merged result as JSON on every `apply` so webtop never has to
//! know about fragment boundaries. See macosctl's design doc, "D14", for the
//! full contract this parser implements.
//!
//! The consequence worth stating plainly: **everything the services panel
//! shows is declared, not discovered.** A daemon missing from the manifest is
//! invisible here no matter how much memory it is holding. That is the correct
//! trade for this panel — it answers "is my stack healthy", and a stack is
//! precisely the set of things someone decided belongs to it. The process
//! manager remains the place to see everything running.
//!
//! Only the fields the dashboard renders are modelled. The manifest carries
//! plenty more (`managed`, `lifecycle`, `source`, `working_directory`) that
//! exists for macosctl's own reconciliation; those are ignored here rather
//! than mirrored, so the two consumers can evolve without dragging each other
//! along.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The only manifest schema version this parser understands. macosctl bumps
/// this when the contract changes shape; webtop must refuse to guess at an
/// unfamiliar one rather than silently misreading it.
const SUPPORTED_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct ServiceDef {
    pub name: String,
    pub label: String,
    /// TCP port this service listens on, when it listens at all. Absent means
    /// liveness can only be judged by "is the process there".
    pub port: Option<u16>,
    /// Display grouping — `infra`, `models`, `edge`, `dashboard`.
    pub group: String,
    /// Declared memory ceiling in bytes.
    pub mem_budget: Option<u64>,
    /// Services that must be up first. launchd has no ordering primitive, so
    /// this is documentation plus the dashboard's dependency column — the
    /// actual enforcement is each launcher polling its dependencies' ports.
    pub depends_on: Vec<String>,
}

/// The manifest as it appears on disk. Unknown keys are ignored by serde's
/// default, which is what lets macosctl add fields freely.
#[derive(Debug, Deserialize)]
struct RawManifest {
    schema: Option<u32>,
    #[serde(default)]
    services: Vec<RawService>,
}

#[derive(Debug, Deserialize)]
struct RawService {
    name: String,
    label: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    mem_budget: Option<u64>,
    #[serde(default)]
    depends_on: Vec<String>,
}

/// Read and parse the manifest.
///
/// A missing file is not an error the caller needs to distinguish — the
/// services panel simply has nothing to show, which is the right outcome on a
/// machine that has no managed stack. Both cases return `Err` with a message
/// suitable for showing in the UI, so the panel can explain itself rather than
/// rendering an unexplained empty list.
pub fn load(path: &Path) -> Result<Vec<ServiceDef>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    parse(&text)
}

pub fn parse(text: &str) -> Result<Vec<ServiceDef>, String> {
    let raw: RawManifest =
        serde_json::from_str(text).map_err(|e| format!("invalid manifest: {e}"))?;

    match raw.schema {
        Some(SUPPORTED_SCHEMA) => {}
        Some(other) => {
            return Err(format!(
                "manifest schema {other} is not supported (expected {SUPPORTED_SCHEMA}) — \
                 rebuild webtop against the current macosctl contract"
            ));
        }
        None => return Err("manifest is missing its schema field".to_string()),
    }

    Ok(raw
        .services
        .into_iter()
        .map(|s| ServiceDef {
            name: s.name,
            label: s.label,
            port: s.port,
            group: s
                .group
                .filter(|g| !g.is_empty())
                .unwrap_or_else(|| "other".into()),
            mem_budget: s.mem_budget,
            depends_on: s.depends_on,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_realistic_manifest() {
        let defs = parse(
            r#"{
                "schema": 1,
                "defaults": { "log_dir": "/Users/example/Library/Logs/services" },
                "services": [
                    { "name": "model-worker", "label": "com.example.model-worker",
                      "port": 8001, "group": "models", "managed": false,
                      "lifecycle": "active", "mem_budget": 47244640256,
                      "depends_on": [], "working_directory": "/Users/example/services",
                      "source": "30-ai.toml" },
                    { "name": "gateway", "label": "com.example.gateway",
                      "port": 4000, "group": null, "managed": true,
                      "lifecycle": "active", "mem_budget": null,
                      "depends_on": ["model-worker"],
                      "working_directory": "/Users/example/services",
                      "source": "30-ai.toml" }
                ]
            }"#,
        )
        .expect("manifest should parse");

        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].mem_budget, Some(44 * 1024 * 1024 * 1024));
        assert_eq!(defs[0].group, "models");
        // Unspecified group falls back rather than failing the whole parse.
        assert_eq!(defs[1].group, "other");
        assert_eq!(defs[1].depends_on, vec!["model-worker"]);
        assert_eq!(defs[1].mem_budget, None);
    }

    #[test]
    fn unsupported_schema_is_rejected() {
        let err = parse(r#"{"schema": 2, "services": []}"#).unwrap_err();
        assert!(err.contains("schema"), "got: {err}");
    }

    #[test]
    fn missing_schema_is_rejected() {
        let err = parse(r#"{"services": []}"#).unwrap_err();
        assert!(err.contains("schema"), "got: {err}");
    }

    /// A regression fixture for the bug this parser was introduced to fix:
    /// services declared in a conf.d fragment other than the stack's own
    /// (here, `image_gen`'s `service.toml`, merged in by macosctl) were
    /// invisible because webtop used to read a single un-merged TOML file
    /// directly. This is a trimmed copy of the real merged manifest's shape.
    #[test]
    fn services_from_a_merged_fragment_are_visible() {
        let defs = parse(
            r#"{
                "schema": 1,
                "defaults": { "log_dir": "/Users/example/Library/Logs/services" },
                "services": [
                    { "name": "gateway", "label": "com.example.gateway",
                      "port": 4000, "group": "edge", "managed": true,
                      "lifecycle": "active", "mem_budget": null,
                      "depends_on": ["postgresql", "phoenix"],
                      "working_directory": "/Users/example/services",
                      "source": "30-ai.toml" },
                    { "name": "worker-api", "label": "com.example.worker-api",
                      "port": 7861, "group": "dashboard", "managed": true,
                      "lifecycle": "active", "mem_budget": null, "depends_on": [],
                      "working_directory": "/Users/example/services",
                      "source": "30-service.toml" },
                    { "name": "worker-jobs",
                      "label": "com.example.worker-jobs",
                      "port": 7862, "group": "dashboard", "managed": true,
                      "lifecycle": "active", "mem_budget": null, "depends_on": [],
                      "working_directory": "/Users/example/services",
                      "source": "30-service.toml" }
                ]
            }"#,
        )
        .expect("manifest should parse");

        let web = defs
            .iter()
            .find(|s| s.name == "worker-api")
            .expect("worker-api should be present");
        assert_eq!(web.port, Some(7861));
        assert_eq!(web.group, "dashboard");

        let worker = defs
            .iter()
            .find(|s| s.name == "worker-jobs")
            .expect("worker-jobs should be present");
        assert_eq!(worker.port, Some(7862));
        assert_eq!(worker.group, "dashboard");
    }
}
