//! The result of a single `verify` check.
//!
//! Every check returns a [`CheckResult`] with a [`Severity`]. The verifier
//! aggregates these: any `Error` severity makes `verify` exit non-zero
//! (design §5.2 step 4, §8.1). `Warn` is reported but does not fail the run.
//! `Skipped` is for the pure-library invocation test (design §5.1) — not a
//! failure, just an explicit "nothing to do here."

/// One check outcome. Ordered by escalating severity; `derive` on `PartialOrd`
/// would be fragile, so we implement the ordering explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Nothing to check (e.g. pure-library invocation test). Does not fail.
    Skipped,
    /// Check passed.
    Pass,
    /// Potential problem; surfaced but non-blocking.
    Warn,
    /// Critical failure; fails `verify` (and blocks `init` from writing).
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skipped => "skip",
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Error => "fail",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Machine-readable check id, e.g. `discovery.description_length`.
    /// Surfaced by the machine-readable report format consumed by tests;
    /// not yet read by the human renderer.
    #[allow(dead_code)]
    pub check_id: String,
    /// Human label, e.g. `SKILL.md description is under 1,536 characters`.
    pub check_name: String,
    pub severity: Severity,
    /// Short status line. For failures this is the *what's wrong*.
    pub message: String,
    /// The actionable fix, per design §8.1 "Errors teach, not just complain".
    /// Populated on `Warn`/`Error`; the one-liner suggestion.
    pub suggestion: Option<String>,
    /// File + line where the problem lives, when we can pin it down. Line is
    /// 1-based, `None` if file-level only.
    pub location: Option<(String, Option<usize>)>,
}

impl CheckResult {
    /// A clean pass with no suggestion needed.
    pub fn pass(check_id: &str, check_name: &str, message: impl Into<String>) -> Self {
        Self {
            check_id: check_id.to_string(),
            check_name: check_name.to_string(),
            severity: Severity::Pass,
            message: message.into(),
            suggestion: None,
            location: None,
        }
    }

    pub fn warn(
        check_id: &str,
        check_name: &str,
        message: impl Into<String>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self {
            check_id: check_id.to_string(),
            check_name: check_name.to_string(),
            severity: Severity::Warn,
            message: message.into(),
            suggestion: Some(suggestion.into()),
            location: None,
        }
    }

    pub fn fail(
        check_id: &str,
        check_name: &str,
        message: impl Into<String>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self {
            check_id: check_id.to_string(),
            check_name: check_name.to_string(),
            severity: Severity::Error,
            message: message.into(),
            suggestion: Some(suggestion.into()),
            location: None,
        }
    }

    pub fn skipped(check_id: &str, check_name: &str, message: impl Into<String>) -> Self {
        Self {
            check_id: check_id.to_string(),
            check_name: check_name.to_string(),
            severity: Severity::Skipped,
            message: message.into(),
            suggestion: None,
            location: None,
        }
    }

    /// True if this result fails `verify` (critical severity).
    pub fn is_critical_failure(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }
}

/// Aggregate of every check in a `verify` run. Knows whether the run passed
/// and can render a human report (design §5.2 step 4).
#[derive(Debug, Default)]
pub struct VerifyReport {
    pub results: Vec<CheckResult>,
}

impl VerifyReport {
    pub fn push(&mut self, r: CheckResult) {
        self.results.push(r);
    }

    /// True iff at least one result is `Error`. This is what gates the exit
    /// code and the `init` pre-commit write.
    pub fn has_critical_failure(&self) -> bool {
        self.results.iter().any(CheckResult::is_critical_failure)
    }

    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut pass = 0;
        let mut warn = 0;
        let mut fail = 0;
        let mut skip = 0;
        for r in &self.results {
            match r.severity {
                Severity::Pass => pass += 1,
                Severity::Warn => warn += 1,
                Severity::Error => fail += 1,
                Severity::Skipped => skip += 1,
            }
        }
        (pass, warn, fail, skip)
    }

    /// A 0-100 score for the plugin distribution's agent-discoverability.
    /// Each check contributes a weighted credit toward a denominator of
    /// eligible (non-skipped) checks:
    ///   Pass  = 1.0  Warn = 0.5  Error = 0.0
    /// Skipped checks are excluded entirely (they represent "nothing to
    /// verify here," not a quality signal). When no checks are eligible
    /// (all skipped, or zero checks ran) the score is 0 — an honest
    /// "nothing verified," not a misleading 100.
    pub fn discoverability_score(&self) -> u8 {
        let mut credits = 0.0_f32;
        let mut eligible = 0;
        for r in &self.results {
            match r.severity {
                Severity::Pass => {
                    credits += 1.0;
                    eligible += 1;
                }
                Severity::Warn => {
                    credits += 0.5;
                    eligible += 1;
                }
                Severity::Error => {
                    eligible += 1;
                }
                Severity::Skipped => {}
            }
        }
        if eligible == 0 {
            return 0;
        }
        (credits / eligible as f32 * 100.0).round() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(id: &str, sev: Severity) -> CheckResult {
        CheckResult {
            check_id: id.to_string(),
            check_name: id.to_string(),
            severity: sev,
            message: String::new(),
            suggestion: None,
            location: None,
        }
    }

    #[test]
    fn score_all_pass_is_100() {
        let mut r = VerifyReport::default();
        r.push(mk("a", Severity::Pass));
        r.push(mk("b", Severity::Pass));
        assert_eq!(r.discoverability_score(), 100);
    }

    #[test]
    fn score_one_warn_out_of_two_is_75() {
        let mut r = VerifyReport::default();
        r.push(mk("a", Severity::Pass));
        r.push(mk("b", Severity::Warn));
        // 1.0 + 0.5 = 1.5 credits / 2 eligible = 75%
        assert_eq!(r.discoverability_score(), 75);
    }

    #[test]
    fn score_error_lowers_below_100() {
        let mut r = VerifyReport::default();
        r.push(mk("a", Severity::Pass));
        r.push(mk("b", Severity::Pass));
        r.push(mk("c", Severity::Error));
        // 2.0 / 3 * 100 = 66.67 -> 67
        assert_eq!(r.discoverability_score(), 67);
    }

    #[test]
    fn score_all_skipped_is_zero() {
        let mut r = VerifyReport::default();
        r.push(mk("a", Severity::Skipped));
        r.push(mk("b", Severity::Skipped));
        assert_eq!(r.discoverability_score(), 0);
    }

    #[test]
    fn score_empty_report_is_zero() {
        let r = VerifyReport::default();
        assert_eq!(r.discoverability_score(), 0);
    }

    #[test]
    fn score_skipped_excluded_from_denominator() {
        let mut r = VerifyReport::default();
        r.push(mk("a", Severity::Skipped));
        r.push(mk("b", Severity::Warn));
        // 0.5 / 1 * 100 = 50, skipped doesn't dilute
        assert_eq!(r.discoverability_score(), 50);
    }

    #[test]
    fn score_all_errors_is_zero() {
        let mut r = VerifyReport::default();
        r.push(mk("a", Severity::Error));
        r.push(mk("b", Severity::Error));
        assert_eq!(r.discoverability_score(), 0);
    }
}
