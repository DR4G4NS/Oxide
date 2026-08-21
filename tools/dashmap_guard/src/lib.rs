//! Deterministic DashMap guard/reentrancy analyzer.
//!
//! `run_check` performs the whole analysis: a record pass that collects
//! per-function direct DashMap effects + call edges and reports direct
//! in-function conflicts (DM001/DM002/DM003/DM005); effect propagation through
//! the intra-crate call graph to a fixed point; then a transitive pass that
//! reports helper-mediated conflicts (DM003/DM004) and unresolved-effect
//! warnings (DM900). Output is deterministically sorted text or JSON.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]

pub mod ast;
pub mod callgraph;
pub mod diagnostics;
pub mod effects;
pub mod identity;
pub mod linter;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use ast::CrateIndex;
use callgraph::CallGraph;
use diagnostics::{is_tool_code, parse_suppressions, sort_diagnostics, Diagnostic, Suppression};
use effects::ToolCode;
use linter::{Mode, Walker};

pub use effects::DmCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone)]
pub struct CheckConfig {
    pub root: PathBuf,
    /// Directories (relative to root) to scan, e.g. ["src", "tests"].
    pub paths: Vec<String>,
    pub format: OutputFormat,
    /// When true, DM900/DM901 warnings also cause a non-zero exit.
    pub deny_warnings: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Summary {
    pub tool: String,
    pub resolved_dashmap: String,
    pub files_checked: usize,
    pub functions_checked: usize,
    pub errors: usize,
    pub warnings: usize,
    pub suppressed: usize,
    pub tool_failures: usize,
}

#[derive(Debug)]
pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
    pub summary: Summary,
    pub text: String,
    pub json: String,
}

pub const RESOLVED_DASHMAP: &str = "5.5.3";

fn walk_rs_files(root: &Path, paths: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        let dir = root.join(p);
        if !dir.exists() {
            continue;
        }
        if dir.is_file() {
            if p.ends_with(".rs") {
                out.push(dir);
            }
            continue;
        }
        for entry in WalkDir::new(&dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !(e.file_type().is_dir() && (name == "target" || name.starts_with('.')))
            })
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.file_type().is_file()
                && entry.path().extension().map(|e| e == "rs").unwrap_or(false)
            {
                out.push(entry.path().to_path_buf());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn run_check(cfg: &CheckConfig) -> CheckResult {
    let files = walk_rs_files(&cfg.root, &cfg.paths);

    let mut index = CrateIndex::new();
    let mut src_lines: Vec<Vec<String>> = Vec::new();
    let mut file_paths: Vec<String> = Vec::new();
    let mut suppressions: Vec<(usize, Vec<Suppression>)> = Vec::new();
    let mut malformed: Vec<(usize, Vec<Diagnostic>)> = Vec::new();
    let mut diags: Vec<Diagnostic> = Vec::new();

    for f in &files {
        let rel = f
            .strip_prefix(&cfg.root)
            .unwrap_or(f)
            .to_string_lossy()
            .to_string();
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                diags.push(Diagnostic::new_tool(
                    ToolCode::Tool002,
                    &rel,
                    1,
                    1,
                    format!("cannot read file: {e}"),
                ));
                continue;
            }
        };
        let file_idx = index.files.len();
        let (supps, bad) = parse_suppressions(&src);
        index.add_file(&rel, &cfg.root, &src);
        if index.files[file_idx].has_parse_error {
            let info = &index.files[file_idx];
            diags.push(Diagnostic::new_tool(
                ToolCode::Tool002,
                &rel,
                info.parse_line.unwrap_or(1),
                info.parse_column.unwrap_or(1),
                format!(
                    "parse error: {}",
                    info.parse_error.as_deref().unwrap_or("unknown")
                ),
            ));
        }
        suppressions.push((file_idx, supps));
        malformed.push((file_idx, bad));
        src_lines.push(src.lines().map(|l| l.to_string()).collect());
        file_paths.push(rel);
    }
    index.finalize();

    let n_fns = index.fns.len();

    // ---- pass 1: record direct effects/calls + direct conflicts ----
    let mut w1 = Walker::new(
        &index,
        None,
        Mode::RecordAndDirect,
        &src_lines,
        &file_paths,
        n_fns,
    );
    w1.run_all();
    diags.extend(w1.diags.clone());
    let span_groups = group_spans_with_paths(w1.node_spans.clone(), &file_paths);

    // ---- propagate effects ----
    let build = CallGraph::build(&index, w1.output_direct.clone());
    diags.extend(build.tool_errors);
    let callgraph = match build.graph {
        Some(g) => g,
        None => {
            let tool_failures = diags.iter().filter(|d| is_tool_code(&d.code)).count();
            let errors = diags
                .iter()
                .filter(|d| d.severity == "error" && !d.suppressed)
                .count();
            let warnings = diags
                .iter()
                .filter(|d| d.severity == "warning" && !d.suppressed)
                .count();
            let summary = Summary {
                tool: "dashmap_guard".to_string(),
                resolved_dashmap: RESOLVED_DASHMAP.to_string(),
                files_checked: files.len(),
                functions_checked: n_fns,
                errors,
                warnings,
                suppressed: 0,
                tool_failures,
            };
            sort_diagnostics(&mut diags);
            let text = render_text(&summary, &diags);
            let json = render_json(&summary, &diags);
            return CheckResult {
                diagnostics: diags,
                summary,
                text,
                json,
            };
        }
    };

    // ---- pass 2: transitive conflicts + DM900 ----
    let mut w2 = Walker::new(
        &index,
        Some(&callgraph),
        Mode::Transitive,
        &src_lines,
        &file_paths,
        n_fns,
    );
    w2.run_all();
    diags.extend(w2.diags);

    // ---- malformed suppression warnings (DM901) ----
    for (file_idx, bad) in &malformed {
        for mut d in bad.iter().cloned() {
            d.file = file_paths[*file_idx].clone();
            diags.push(d);
        }
    }

    // ---- apply suppressions ----
    apply_suppressions(&mut diags, &suppressions, &span_groups, &file_paths);

    // ---- deterministic ordering ----
    sort_diagnostics(&mut diags);

    let errors = diags
        .iter()
        .filter(|d| d.severity == "error" && !d.suppressed)
        .count();
    let warnings = diags
        .iter()
        .filter(|d| d.severity == "warning" && !d.suppressed)
        .count();
    let suppressed = diags.iter().filter(|d| d.suppressed).count();
    let tool_failures = diags.iter().filter(|d| is_tool_code(&d.code)).count();

    let summary = Summary {
        tool: "dashmap_guard".to_string(),
        resolved_dashmap: RESOLVED_DASHMAP.to_string(),
        files_checked: files.len(),
        functions_checked: n_fns,
        errors,
        warnings,
        suppressed,
        tool_failures,
    };

    let text = render_text(&summary, &diags);
    let json = render_json(&summary, &diags);

    CheckResult {
        diagnostics: diags,
        summary,
        text,
        json,
    }
}

fn group_spans_with_paths(
    spans: Vec<(usize, usize, usize)>,
    file_paths: &[String],
) -> HashMap<String, Vec<(usize, usize)>> {
    let mut m: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (file_idx, start, end) in spans {
        m.entry(file_paths[file_idx].clone())
            .or_default()
            .push((start, end));
    }
    for v in m.values_mut() {
        v.sort();
        v.dedup();
    }
    m
}

/// Each suppression applies only to the next relevant statement/function:
/// the target span is the first statement span whose start is strictly after
/// the suppression comment line. A diagnostic is suppressed when its primary
/// line falls inside that target span and its code matches.
fn apply_suppressions(
    diags: &mut [Diagnostic],
    suppressions: &[(usize, Vec<Suppression>)],
    node_spans: &HashMap<String, Vec<(usize, usize)>>,
    file_paths: &[String],
) {
    // Precompute per-file (suppression code, target span).
    let mut targets: HashMap<usize, Vec<(crate::effects::DmCode, (usize, usize), String)>> =
        HashMap::new();
    for (file_idx, supps) in suppressions {
        let path = &file_paths[*file_idx];
        let spans = node_spans.get(path);
        for s in supps {
            let target = spans
                .and_then(|v| v.iter().find(|(start, _)| *start > s.line).copied())
                .unwrap_or((s.line, s.line));
            targets
                .entry(*file_idx)
                .or_default()
                .push((s.code, target, s.reason.clone()));
        }
    }

    // Map diag file path -> file_idx.
    for d in diags.iter_mut() {
        if d.suppressed {
            continue;
        }
        let Some(file_idx) = file_paths.iter().position(|p| p == &d.file) else {
            continue;
        };
        let Some(Some(code)) = Some(crate::effects::DmCode::from_str(&d.code)) else {
            continue;
        };
        let Some(list) = targets.get(&file_idx) else {
            continue;
        };
        for (supp_code, (ts, te), reason) in list {
            if *supp_code == code && d.line >= *ts && d.line <= *te {
                d.suppressed = true;
                d.suppression_reason = Some(reason.clone());
                break;
            }
        }
    }
}

fn render_text(summary: &Summary, diags: &[Diagnostic]) -> String {
    let mut out = String::new();
    for d in diags {
        out.push_str(&diagnostics::render_text(d));
        out.push('\n');
    }
    out.push_str(&format!(
        "{}: {} file(s), {} function(s), {} error(s), {} warning(s), {} suppressed, {} tool failure(s)\n",
        summary.tool,
        summary.files_checked,
        summary.functions_checked,
        summary.errors,
        summary.warnings,
        summary.suppressed,
        summary.tool_failures
    ));
    out
}

fn render_json(summary: &Summary, diags: &[Diagnostic]) -> String {
    #[derive(serde::Serialize)]
    struct Report<'a> {
        summary: &'a Summary,
        diagnostics: &'a [Diagnostic],
    }
    let rep = Report {
        summary,
        diagnostics: diags,
    };
    serde_json::to_string_pretty(&rep).unwrap_or_else(|_| "{}".to_string())
}
