//! Study-guide PDF rendering.
//!
//! The PDF rendering itself is done by `scripts/study_guide.py` (ReportLab),
//! spawned as a child process. Rust builds a JSON payload, pipes it to the
//! script's stdin, and reads PDF bytes from stdout. This keeps the Rust crate
//! free of any Python dependency at build time — Python is only needed at
//! runtime, and only for this one feature.

use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::routes::ApiError;

/// Errors specific to PDF rendering. Surfaced to the user via `ApiError` so
/// the UI gets actionable hints (install Python, install reportlab, etc.).
#[derive(Debug)]
pub enum PdfError {
    /// `python` / `python3` not found on PATH.
    PythonNotFound,
    /// Couldn't locate `scripts/study_guide.py` near the executable or CWD.
    ScriptNotFound(PathBuf),
    /// I/O error spawning or talking to the subprocess.
    Spawn(String),
    /// The script exited non-zero. `stderr` is included for diagnostics.
    NonZero { code: Option<i32>, stderr: String },
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::PythonNotFound => write!(
                f,
                "Python not found on PATH. Install Python 3.10+ and reportlab \
                 (pip install -r requirements.txt) to use the study-guide PDF feature."
            ),
            PdfError::ScriptNotFound(p) => write!(
                f,
                "study_guide.py not found (looked near {}). Make sure scripts/study_guide.py \
                 ships alongside the binary or is in the project root.",
                p.display()
            ),
            PdfError::Spawn(e) => write!(f, "subprocess error: {e}"),
            PdfError::NonZero { code, stderr } => {
                let exit = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                write!(f, "study_guide.py exited {exit}: {}", stderr.trim())
            }
        }
    }
}

impl From<PdfError> for ApiError {
    fn from(e: PdfError) -> Self {
        ApiError::internal(e.to_string())
    }
}

/// argv-style command for invoking Python (e.g. `["python"]` or `["py", "-3"]`).
type PyCmd = Vec<String>;

/// Find a Python interpreter that has `reportlab` installed.
///
/// Strategy:
///   1. If `CISSP_PYTHON_BIN` is set, split it on whitespace and use that.
///      (e.g. `CISSP_PYTHON_BIN="C:\Python314\python.exe"`,
///       or `CISSP_PYTHON_BIN="py -3.14"`).
///   2. Probe each candidate launcher (`python3`, `python`, `py -3`) with
///      `-c "import reportlab"`. Pick the first that succeeds — this skips
///      Python installs without reportlab (e.g. Microsoft Store alias) so the
///      subprocess actually has the dependency.
///   3. Fall back to whichever candidate runs `--version` so the user gets a
///      "reportlab not installed" error rather than "python not found".
fn locate_python() -> Result<PyCmd, PdfError> {
    if let Ok(raw) = std::env::var("CISSP_PYTHON_BIN") {
        let parts: Vec<String> = raw.split_whitespace().map(|s| s.to_string()).collect();
        if !parts.is_empty() {
            return Ok(parts);
        }
    }

    let candidates: Vec<PyCmd> = vec![
        vec!["python3".into()],
        vec!["python".into()],
        vec!["py".into(), "-3".into()],
    ];

    // Pass 1: prefer the candidate that can already import reportlab.
    for cand in &candidates {
        if probe_python(cand, &["-c", "import reportlab"]) {
            return Ok(cand.clone());
        }
    }

    // Pass 2: any working Python at all (so the user hits the
    // "reportlab not installed" stderr from the script, not a vague
    // "python not found" from us).
    for cand in &candidates {
        if probe_python(cand, &["--version"]) {
            return Ok(cand.clone());
        }
    }

    Err(PdfError::PythonNotFound)
}

fn probe_python(cmd: &[String], extra_args: &[&str]) -> bool {
    if cmd.is_empty() {
        return false;
    }
    let mut c = Command::new(&cmd[0]);
    for a in &cmd[1..] {
        c.arg(a);
    }
    for a in extra_args {
        c.arg(a);
    }
    c.stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());
    matches!(c.status(), Ok(s) if s.success())
}

/// Locate `scripts/study_guide.py`. We check, in order:
///   1. `<exe_dir>/scripts/study_guide.py`         (release deployment)
///   2. `<exe_dir>/../../scripts/study_guide.py`   (cargo run from target/debug)
///   3. `./scripts/study_guide.py`                 (cwd-based dev)
fn locate_script() -> Result<PathBuf, PdfError> {
    let mut tried: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("scripts").join("study_guide.py");
            if p.exists() {
                return Ok(p);
            }
            tried.push(p);
            // target/debug/cissp-coach.exe -> ../../scripts/study_guide.py
            let p = dir
                .join("..")
                .join("..")
                .join("scripts")
                .join("study_guide.py");
            if p.exists() {
                return Ok(p);
            }
            tried.push(p);
        }
    }

    let p = PathBuf::from("scripts").join("study_guide.py");
    if p.exists() {
        return Ok(p);
    }
    tried.push(p);

    // Return the most-likely path for the error message.
    Err(PdfError::ScriptNotFound(
        tried.into_iter().next().unwrap_or_else(|| PathBuf::from(".")),
    ))
}

/// Render a study-guide PDF by piping `payload` (as JSON) to the Python
/// script's stdin and capturing PDF bytes from stdout.
pub fn render_study_guide(payload: &Value) -> Result<Vec<u8>, PdfError> {
    let py_cmd = locate_python()?;
    let script = locate_script()?;

    let payload_bytes =
        serde_json::to_vec(payload).map_err(|e| PdfError::Spawn(format!("payload serialize: {e}")))?;

    let mut cmd = Command::new(&py_cmd[0]);
    for arg in &py_cmd[1..] {
        cmd.arg(arg);
    }
    let mut child = cmd
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PdfError::Spawn(format!("spawn {}: {e}", py_cmd.join(" "))))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| PdfError::Spawn("could not open child stdin".into()))?;
        stdin
            .write_all(&payload_bytes)
            .map_err(|e| PdfError::Spawn(format!("write stdin: {e}")))?;
    }
    // Drop stdin so Python sees EOF.
    drop(child.stdin.take());

    let out = child
        .wait_with_output()
        .map_err(|e| PdfError::Spawn(format!("wait: {e}")))?;

    if !out.status.success() {
        return Err(PdfError::NonZero {
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        });
    }

    if !out.stdout.starts_with(b"%PDF-") {
        // The script claimed success but didn't emit a PDF — surface stderr.
        return Err(PdfError::NonZero {
            code: out.status.code(),
            stderr: format!(
                "script returned {} bytes but no PDF magic. stderr: {}",
                out.stdout.len(),
                String::from_utf8_lossy(&out.stderr)
            ),
        });
    }

    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_python_returns_something_or_python_not_found() {
        // We don't assume Python is installed in CI. Just verify the function
        // returns a typed result without panicking.
        match locate_python() {
            Ok(cmd) => {
                assert!(!cmd.is_empty());
                let first = &cmd[0];
                assert!(
                    first == "python3" || first == "python" || first == "py" || !first.is_empty(),
                    "unexpected python launcher: {first:?}"
                );
            }
            Err(PdfError::PythonNotFound) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
