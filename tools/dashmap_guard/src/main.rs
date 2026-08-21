//! dashmap_guard — deterministic DashMap guard/reentrancy analyzer.

use std::path::PathBuf;
use std::process::ExitCode;

use dashmap_guard::{run_check, CheckConfig, OutputFormat};

const USAGE: &str = r#"dashmap_guard — deterministic DashMap guard/reentrancy analyzer

USAGE:
  dashmap_guard check <ROOT> --paths <dir1,dir2,...> [--format text|json]

OPTIONS:
  --paths dir1,dir2   Directories (relative to ROOT) to scan (default: src)
  --format fmt        text (default) or json
  --deny-warnings     Treat DM900/DM901 warnings as blocking (for migration gate)

EXIT STATUS:
  0  no blocking diagnostics (DM001-DM005, TOOLxxx) fired
  1  at least one blocking diagnostic (or a malformed invocation)
"#;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root: Option<PathBuf> = None;
    let mut paths: Vec<String> = vec!["src".to_string()];
    let mut format = OutputFormat::Text;
    let mut deny_warnings = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "check" => {
                i += 1;
            }
            "--paths" => {
                if i + 1 < args.len() {
                    paths = args[i + 1]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    i += 2;
                } else {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            }
            "--deny-warnings" => {
                deny_warnings = true;
                i += 1;
            }
            "--format" => {
                if i + 1 < args.len() {
                    format = match args[i + 1].as_str() {
                        "text" => OutputFormat::Text,
                        "json" => OutputFormat::Json,
                        other => {
                            eprintln!("unknown format `{other}`; use text or json");
                            return ExitCode::from(2);
                        }
                    };
                    i += 2;
                } else {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            }
            arg if !arg.starts_with('-') && root.is_none() => {
                root = Some(PathBuf::from(arg));
                i += 1;
            }
            arg => {
                eprintln!("dashmap_guard: unexpected argument `{arg}`");
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    let Some(root) = root else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let cfg = CheckConfig {
        root,
        paths,
        format,
        deny_warnings,
    };
    let result = run_check(&cfg);

    match format {
        OutputFormat::Text => print!("{}", result.text),
        OutputFormat::Json => println!("{}", result.json),
    }

    if result.summary.errors > 0
        || (deny_warnings && result.summary.warnings > 0)
        || result.summary.tool_failures > 0
    {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    }
}
