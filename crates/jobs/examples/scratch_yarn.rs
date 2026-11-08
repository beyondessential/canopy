//! Scratch: parse a yarn.lock from a local checkout the way the scan does.
use std::{collections::HashMap, path::PathBuf};

use jobs::vulnerabilities::lockfile::yarn;

fn main() {
	let root = PathBuf::from(std::env::args().nth(1).expect("checkout path"));
	let text = std::fs::read_to_string(root.join("yarn.lock")).expect("yarn.lock");
	let parsed = match yarn::read(&text) {
		Ok(p) => p,
		Err(e) => {
			println!("READ FAILED: {e}");
			return;
		}
	};
	let workspaces = parsed.workspaces();
	println!("workspaces: {}", workspaces.len());
	let mut texts: HashMap<String, String> = HashMap::new();
	for ws in &workspaces {
		let path = if ws.is_empty() {
			root.join("package.json")
		} else {
			root.join(ws).join("package.json")
		};
		match std::fs::read_to_string(&path) {
			Ok(t) => {
				texts.insert((*ws).to_owned(), t);
			}
			Err(e) => println!("MISSING MANIFEST for workspace {ws:?} at {path:?}: {e}"),
		}
	}
	let manifests: HashMap<String, &str> = texts
		.iter()
		.map(|(k, v)| (k.clone(), v.as_str()))
		.collect();
	match parsed.parse("tupaia", &manifests) {
		Ok(packages) => {
			println!("packages: {}", packages.len());
			let unreached = packages.iter().filter(|p| p.paths.is_empty()).count();
			println!("with no path: {unreached}");
			for p in packages.iter().take(5) {
				println!("  {} {} paths={}", p.name, p.version, p.paths.len());
			}
		}
		Err(e) => println!("PARSE FAILED: {e}"),
	}
}
