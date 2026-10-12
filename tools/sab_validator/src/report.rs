//! The diagnostic model: a severity-tiered [`Issue`] with an engine-evidence citation, and a
//! [`Report`] that aggregates issues + scan counters. One [`Report::exit_code`] funnel decides
//! the verdict so the "which findings gate a build" policy lives in exactly one auditable place.

use std::collections::BTreeMap;
use std::fmt;

/// Three tiers. Only [`Severity::Fatal`] gates the exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The engine's loader would reject or fault on this — the asset will not load.
    Fatal,
    /// Loads, but violates an invariant the engine or cooker always upholds; likely a latent bug
    /// (bad skin weights, off-by-one mip). Worth fixing, not load-blocking.
    Warning,
    /// A heuristic / cosmetic deviation from how retail bakes look (e.g. non-sector alignment
    /// the mounter tolerates). Reported for insight; never gates.
    Advisory,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Fatal => "FATAL",
            Severity::Warning => "WARN",
            Severity::Advisory => "INFO",
        }
    }
    /// ANSI color for terminal output.
    fn color(self) -> &'static str {
        match self {
            Severity::Fatal => "\x1b[31;1m",   // red bold
            Severity::Warning => "\x1b[33;1m", // yellow bold
            Severity::Advisory => "\x1b[36m",  // cyan
        }
    }
}

/// One finding. `code` is a stable kebab id (e.g. `dtex.bad-format-code`), `engine_ref` cites the
/// decompiled function the rule is derived from so every diagnostic is auditable back to the disasm.
#[derive(Clone, Debug)]
pub struct Issue {
    pub severity: Severity,
    pub code: &'static str,
    /// Which format layer raised it: `megapack` | `sbla` | `dtex`.
    pub format: &'static str,
    /// Human path to the offending asset within the archive.
    pub location: String,
    /// `<what> — <engine consequence>`.
    pub message: String,
    /// e.g. `FUN_009bb910 @0x009bb910`.
    pub engine_ref: Option<&'static str>,
}

/// Aggregated run result.
#[derive(Default)]
pub struct Report {
    pub issues: Vec<Issue>,
    /// Per-format count of assets successfully walked (for the "N scanned, 0 fatal" summary).
    pub scanned: BTreeMap<&'static str, u64>,
    /// Per-format count of assets that raised at least one fatal.
    pub failed: BTreeMap<&'static str, u64>,
    /// Per-check count of assets the check could not be applied to (it needs data the run lacks).
    pub unchecked: BTreeMap<&'static str, u64>,
}

impl Report {
    pub fn push(&mut self, issue: Issue) {
        self.issues.push(issue);
    }
    pub fn scanned(&mut self, format: &'static str) {
        *self.scanned.entry(format).or_default() += 1;
    }
    pub fn failed(&mut self, format: &'static str) {
        *self.failed.entry(format).or_default() += 1;
    }
    pub fn unchecked(&mut self, check: &'static str) {
        *self.unchecked.entry(check).or_default() += 1;
    }
    pub fn count(&self, sev: Severity) -> usize {
        self.issues.iter().filter(|i| i.severity == sev).count()
    }
    /// The single verdict funnel: any fatal finding → 1.
    pub fn exit_code(&self) -> i32 {
        if self.count(Severity::Fatal) == 0 {
            0
        } else {
            1
        }
    }

    /// Machine-readable output (hand-rolled to keep the crate dependency-light).
    pub fn to_json(&self) -> String {
        let mut s = String::from("{\n  \"issues\": [\n");
        for (i, iss) in self.issues.iter().enumerate() {
            s.push_str("    {");
            s.push_str(&format!("\"severity\":\"{}\",", iss.severity.label()));
            s.push_str(&format!("\"code\":{},", json_str(iss.code)));
            s.push_str(&format!("\"format\":{},", json_str(iss.format)));
            s.push_str(&format!("\"location\":{},", json_str(&iss.location)));
            s.push_str(&format!("\"message\":{},", json_str(&iss.message)));
            s.push_str(&format!(
                "\"engine_ref\":{}",
                iss.engine_ref.map(json_str).unwrap_or_else(|| "null".into())
            ));
            s.push('}');
            s.push_str(if i + 1 < self.issues.len() { ",\n" } else { "\n" });
        }
        s.push_str("  ],\n  \"scanned\": {");
        push_counter_map(&mut s, &self.scanned);
        s.push_str("},\n  \"failed\": {");
        push_counter_map(&mut s, &self.failed);
        s.push_str("},\n  \"unchecked\": {");
        push_counter_map(&mut s, &self.unchecked);
        s.push_str("},\n");
        s.push_str(&format!(
            "  \"summary\": {{\"fatal\":{},\"warn\":{},\"info\":{},\"exit_code\":{}}}\n",
            self.count(Severity::Fatal),
            self.count(Severity::Warning),
            self.count(Severity::Advisory),
            self.exit_code()
        ));
        s.push('}');
        s
    }
}

fn push_counter_map(s: &mut String, m: &BTreeMap<&'static str, u64>) {
    for (i, (k, v)) in m.iter().enumerate() {
        s.push_str(&format!("{}:{}", json_str(k), v));
        if i + 1 < m.len() {
            s.push(',');
        }
    }
}

/// Minimal JSON string escaper (control chars, quote, backslash).
fn json_str(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for c in raw.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Terminal rendering (one line per issue, colored, self-capped by the caller).
impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reset = "\x1b[0m";
        write!(
            f,
            "{}{:<5}{} [{}] {}: {}",
            self.severity.color(),
            self.severity.label(),
            reset,
            self.code,
            self.location,
            self.message
        )?;
        if let Some(r) = self.engine_ref {
            write!(f, "  ({r})")?;
        }
        Ok(())
    }
}
