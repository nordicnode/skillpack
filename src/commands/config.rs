//! `skillpack config` — validate (or summarize) the committed `skillpack.toml`.
//! `--validate` exits non-zero on an invalid config (mirrors the load-time
//! structural invariants: kebab-case names, well-formed TOML).

use std::path::Path;

use skillpack::config::Config;
use skillpack::exit;

pub(crate) fn run_config(root: &Path, validate: bool) -> i32 {
    match Config::load(root) {
        Ok(Some(cfg)) => {
            let intents = cfg.to_intents();
            if validate {
                println!("skillpack.toml is valid ({} skill(s))", intents.len());
            } else {
                println!("skillpack.toml summary:");
                println!("  skills: {}", intents.len());
                for (name, intent) in &intents {
                    println!("    - {name}: {}", intent.one_line_description);
                }
                if let Some(a) = &cfg.defaults.author {
                    println!("  defaults.author: {a}");
                }
                if let Some(l) = &cfg.defaults.license {
                    println!("  defaults.license: {l}");
                }
            }
            exit::INIT_OK
        }
        Ok(None) => {
            eprintln!(
                "no skillpack.toml at {} (run `skillpack init` first)",
                Config::path(root).display()
            );
            exit::INIT_FATAL
        }
        Err(e) => {
            eprintln!("invalid skillpack.toml: {e:#}");
            exit::INIT_FATAL
        }
    }
}
