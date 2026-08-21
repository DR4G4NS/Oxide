//! Diagnostics: DM codes, suppression parsing, deterministic sorting and
//! text/JSON rendering.

use serde::Serialize;

use crate::effects::{DmCode, ToolCode};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Loc {
    pub file: String,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: String, // "error" for DM001..DM005, "warning" otherwise
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub guard: Option<String>,
    pub acquired: Option<Loc>,
    pub conflict: Option<Loc>,
    pub effect_path: Vec<String>,
    pub hint: Option<String>,
    pub suppressed: bool,
    pub suppression_reason: Option<String>,
    pub acquired_src: Option<String>,
    pub conflict_src: Option<String>,
}

impl Diagnostic {
    pub fn new(
        code: DmCode,
        file: &str,
        line: usize,
        column: usize,
        message: impl Into<String>,
    ) -> Diagnostic {
        Diagnostic {
            code: code.as_str().to_string(),
            severity: if code.is_blocking() {
                "error".to_string()
            } else {
                "warning".to_string()
            },
            file: file.to_string(),
            line,
            column,
            message: message.into(),
            guard: None,
            acquired: None,
            conflict: None,
            effect_path: Vec::new(),
            hint: None,
            suppressed: false,
            suppression_reason: None,
            acquired_src: None,
            conflict_src: None,
        }
    }

    pub fn new_tool(
        code: ToolCode,
        file: &str,
        line: usize,
        column: usize,
        message: impl Into<String>,
    ) -> Diagnostic {
        Diagnostic {
            code: code.as_str().to_string(),
            severity: "error".to_string(),
            file: file.to_string(),
            line,
            column,
            message: message.into(),
            guard: None,
            acquired: None,
            conflict: None,
            effect_path: Vec::new(),
            hint: None,
            suppressed: false,
            suppression_reason: None,
            acquired_src: None,
            conflict_src: None,
        }
    }

    pub fn with_guard(mut self, g: &str) -> Diagnostic {
        self.guard = Some(g.to_string());
        self
    }
    pub fn with_acquired(mut self, loc: Loc) -> Diagnostic {
        self.acquired = Some(loc);
        self
    }
    pub fn with_conflict(mut self, loc: Loc) -> Diagnostic {
        self.conflict = Some(loc);
        self
    }
    pub fn with_effect_path(mut self, path: Vec<String>) -> Diagnostic {
        self.effect_path = path;
        self
    }
    pub fn with_hint(mut self, h: &str) -> Diagnostic {
        self.hint = Some(h.to_string());
        self
    }
}

// ---------------------------------------------------------------------------
// Suppressions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Suppression {
    pub line: usize,
    pub code: DmCode,
    pub reason: String,
    /// Set to the target node line during completion.
    pub target: Option<usize>,
}

/// A parsed suppression comment:
/// `// dashmap-guard: allow DM900 reason="non-empty justification"`
pub fn parse_suppressions(src: &str) -> (Vec<Suppression>, Vec<Diagnostic>) {
    let mut supps = Vec::new();
    let mut malformed = Vec::new();
    for (idx, raw_line) in src.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw_line.trim_start();
        if !trimmed.contains("dashmap-guard: allow") {
            continue;
        }
        // Strip optional `//` prefix.
        let rest = trimmed.trim_start_matches('/').trim();
        // rest should be like: dashmap-guard: allow DM900 reason="..."
        let Some(rest) = rest.strip_prefix("dashmap-guard:") else {
            continue;
        };
        let rest = rest.trim();
        let Some(rest) = rest.strip_prefix("allow") else {
            malformed.push(Diagnostic::new(
                DmCode::Dm901,
                "",
                line_no,
                1,
                "malformed suppression: expected `dashmap-guard: allow DMxxxx reason=\"...\"`"
                    .to_string(),
            ));
            continue;
        };
        let rest = rest.trim();
        // Extract code: first whitespace-terminated token.
        let code_part = rest.split_whitespace().next().unwrap_or("");
        let Some(code) = DmCode::from_str(code_part) else {
            malformed.push(Diagnostic::new(
                DmCode::Dm901,
                "",
                line_no,
                1,
                format!("malformed suppression: unknown code `{code_part}`"),
            ));
            continue;
        };
        // Extract reason="..."
        let reason = extract_reason(rest);
        match reason {
            Some(reason) if !reason.trim().is_empty() => {
                if code == DmCode::Dm901 {
                    malformed.push(Diagnostic::new(
                        DmCode::Dm901,
                        "",
                        line_no,
                        1,
                        "malformed suppression: cannot suppress DM901",
                    ));
                    continue;
                }
                supps.push(Suppression {
                    line: line_no,
                    code,
                    reason,
                    target: None,
                });
            }
            _ => {
                malformed.push(Diagnostic::new(
                    DmCode::Dm901,
                    "",
                    line_no,
                    1,
                    "malformed suppression: a non-empty `reason=\"...\"` is required",
                ));
            }
        }
    }
    (supps, malformed)
}

fn extract_reason(s: &str) -> Option<String> {
    // Look for reason="..." (single or double quotes).
    for marker in ["reason=\"", "reason='"] {
        if let Some(start) = s.find(marker) {
            let rest = &s[start + marker.len()..];
            let end = rest
                .find('"')
                .or_else(|| rest.find('\''))
                .unwrap_or(rest.len());
            return Some(rest[..end].to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Deterministic ordering + rendering
// ---------------------------------------------------------------------------

/// Sort diagnostics deterministically: file, line, column, code.
pub fn sort_diagnostics(diags: &mut [Diagnostic]) {
    diags.sort_by(|a, b| {
        (&a.file, a.line, a.column, &a.code).cmp(&(&b.file, b.line, b.column, &b.code))
    });
}

/// Zero-terminated summary of a diagnostic set for the JSON report.
pub fn count_blocking(diags: &[Diagnostic]) -> usize {
    diags
        .iter()
        .filter(|d| d.severity == "error" && !d.suppressed)
        .count()
}

pub fn is_tool_code(code: &str) -> bool {
    code.starts_with("TOOL")
}

pub fn render_text(diag: &Diagnostic) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} {}: {}\n",
        diag.code,
        diag.severity,
        if diag.suppressed {
            format!(
                "{} (suppressed: {})",
                diag.message,
                diag.suppression_reason.as_deref().unwrap_or("")
            )
        } else {
            diag.message.clone()
        }
    ));
    if diag.acquired.is_none() && diag.conflict.is_none() {
        out.push_str(&format!("  at: {}:{}\n", diag.file, diag.line));
    }
    if let Some(g) = &diag.guard {
        out.push_str(&format!("  guard: {g}\n"));
    }
    if let Some(a) = &diag.acquired {
        out.push_str(&format!("  acquired: {}:{}\n", a.file, a.line));
        if let Some(src) = &diag.acquired_src {
            if !src.trim().is_empty() {
                out.push_str(&format!("    {}\n", src.trim()));
            }
        }
    }
    if let Some(c) = &diag.conflict {
        out.push_str(&format!("  conflict: {}:{}\n", c.file, c.line));
        if let Some(src) = &diag.conflict_src {
            if !src.trim().is_empty() {
                out.push_str(&format!("    {}\n", src.trim()));
            }
        }
    }
    if !diag.effect_path.is_empty() {
        out.push_str("  effect path:\n");
        let mut first = true;
        for hop in &diag.effect_path {
            if first {
                out.push_str(&format!("    {hop}\n"));
                first = false;
            } else {
                out.push_str(&format!("      -> {hop}\n"));
            }
        }
    }
    if let Some(h) = &diag.hint {
        out.push_str(&format!("  hint: {h}\n"));
    }
    out
}
