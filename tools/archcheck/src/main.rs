//! Enforces the dependency rules in AGENTS.md against `cargo metadata`.
//!
//! 1. `engine` never reaches `win32-*`, `transport`, or `ipc`, directly or not.
//! 2. Only `mouseflock-agent` depends on `win32-input` and `win32-clipboard`, and
//!    only `mouseflock-service` on `win32-session` (ADR 0006), and only
//!    `mouseflock-desktop` on `win32-shell` (ADR 0007).
//! 3. Every lib and bin root except the four `win32-*` crates has
//!    `#![forbid(unsafe_code)]`.
//!
//! Run with `cargo archcheck`.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::process::{Command, ExitCode};

use serde_json::Value;

const UNSAFE_CRATES: [&str; 4] = [
    "win32-input",
    "win32-clipboard",
    "win32-session",
    "win32-shell",
];
const ENGINE_FORBIDDEN: [&str; 6] = [
    "win32-input",
    "win32-clipboard",
    "win32-session",
    "win32-shell",
    "transport",
    "ipc",
];
/// Each `unsafe` crate and the one package allowed to depend on it.
const ONLY_LINKER: [(&str, &str); 4] = [
    ("win32-input", "mouseflock-agent"),
    ("win32-clipboard", "mouseflock-agent"),
    ("win32-session", "mouseflock-service"),
    ("win32-shell", "mouseflock-desktop"),
];

struct Package {
    deps: BTreeSet<String>,
    roots: Vec<String>,
}

fn main() -> ExitCode {
    let packages = match load() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("archcheck: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut errors = Vec::new();

    let reached = reachable(&packages, "engine");
    for name in ENGINE_FORBIDDEN.iter().filter(|n| reached.contains(**n)) {
        errors.push(format!("engine depends on `{name}`"));
    }

    for (name, pkg) in &packages {
        for (dep, linker) in ONLY_LINKER {
            if name != linker && pkg.deps.contains(dep) {
                errors.push(format!("`{name}` depends on `{dep}`; only `{linker}` may"));
            }
        }
    }

    for (name, pkg) in &packages {
        if UNSAFE_CRATES.contains(&name.as_str()) {
            continue;
        }
        for root in &pkg.roots {
            match std::fs::read_to_string(root) {
                Ok(src) if src.contains("#![forbid(unsafe_code)]") => {}
                Ok(_) => errors.push(format!("`{name}`: {root} lacks #![forbid(unsafe_code)]")),
                Err(e) => errors.push(format!("`{name}`: cannot read {root}: {e}")),
            }
        }
    }

    if errors.is_empty() {
        println!("archcheck: {} packages ok", packages.len());
        ExitCode::SUCCESS
    } else {
        for e in &errors {
            eprintln!("archcheck: {e}");
        }
        ExitCode::FAILURE
    }
}

/// Workspace packages keyed by name, with their workspace-internal dependencies.
fn load() -> Result<BTreeMap<String, Package>, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let meta: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let list = meta["packages"]
        .as_array()
        .ok_or("no packages in metadata")?;

    let names: BTreeSet<&str> = list.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut packages = BTreeMap::new();
    for p in list {
        let name = p["name"].as_str().ok_or("package without name")?;
        let deps = p["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d["name"].as_str())
            .filter(|d| names.contains(d))
            .map(str::to_owned)
            .collect();
        let roots = p["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| {
                t["kind"].as_array().is_some_and(|k| {
                    k.iter()
                        .any(|k| matches!(k.as_str(), Some("lib" | "rlib" | "bin")))
                })
            })
            .filter_map(|t| t["src_path"].as_str())
            .map(str::to_owned)
            .collect();
        packages.insert(name.to_owned(), Package { deps, roots });
    }
    Ok(packages)
}

/// All workspace packages `from` depends on, transitively.
fn reachable(packages: &BTreeMap<String, Package>, from: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from.to_owned()];
    while let Some(name) = stack.pop() {
        for dep in packages.get(&name).map(|p| &p.deps).into_iter().flatten() {
            if seen.insert(dep.clone()) {
                stack.push(dep.clone());
            }
        }
    }
    seen
}
