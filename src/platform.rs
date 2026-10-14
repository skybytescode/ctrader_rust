//! Where the external helpers live. The app is run from the repository root
//! (the DuckDB path is relative too), so the defaults are relative to it.

/// Python interpreter and leading arguments for the ML scripts. `PYTHON`
/// overrides it, e.g. `PYTHON=.venv/bin/python`.
pub fn python_command() -> (String, Vec<String>) {
    python_command_from(std::env::var("PYTHON").ok(), cfg!(windows))
}

fn python_command_from(env: Option<String>, windows: bool) -> (String, Vec<String>) {
    match env.filter(|p| !p.trim().is_empty()) {
        Some(python) => (python, Vec::new()),
        None if windows => ("C:/Windows/py.exe".into(), vec!["-3.12".into()]),
        None => ("python3".into(), Vec::new()),
    }
}

/// Directory of the econcal FXStreet proxy, `ECONCAL_DIR` or `./econcal`.
pub fn econcal_dir() -> String {
    std::env::var("ECONCAL_DIR")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| "econcal".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_defaults_per_platform() {
        assert_eq!(python_command_from(None, false), ("python3".into(), vec![]));
        assert_eq!(python_command_from(None, true), ("C:/Windows/py.exe".into(), vec!["-3.12".into()]));
    }

    #[test]
    fn python_override_wins() {
        assert_eq!(python_command_from(Some(".venv/bin/python".into()), true), (".venv/bin/python".into(), vec![]));
        assert_eq!(python_command_from(Some(" ".into()), false).0, "python3");
    }
}
