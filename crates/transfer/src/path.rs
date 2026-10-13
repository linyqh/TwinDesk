//! Manifest path validation (report §10.2).
//!
//! Every path in a received manifest is untrusted. This is the string-level
//! gate; the receiver must still open and commit relative to a directory
//! handle so a swapped directory cannot redirect writes (TOCTOU).

use thiserror::Error;

/// Deeper trees are rejected rather than risking path-length failures halfway
/// through a transfer.
pub const MAX_DEPTH: usize = 64;
pub const MAX_COMPONENT_BYTES: usize = 255;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("absolute paths, drive letters and UNC paths are not allowed")]
    Absolute,
    #[error("path contains an empty, '.' or '..' component")]
    Traversal,
    #[error("path contains a NUL or control character")]
    ControlChar,
    #[error("path component uses a reserved name or character: {0}")]
    Reserved(String),
    #[error("path component longer than {MAX_COMPONENT_BYTES} bytes")]
    ComponentTooLong,
    #[error("path deeper than {MAX_DEPTH} components")]
    TooDeep,
}

/// Validates a `/`-separated manifest path against both macOS and Windows
/// rules, since either may end up as the receiver. Returns the components.
pub fn validate_relative_path(path: &str) -> Result<Vec<&str>, PathError> {
    if path.is_empty() {
        return Err(PathError::Empty);
    }
    if path.starts_with('/') || path.starts_with('\\') || has_drive_prefix(path) {
        return Err(PathError::Absolute);
    }
    if path.contains('\\') {
        // Backslash is a separator on Windows; accepting it here would let
        // "a\..\..\x" slip past the component checks below.
        return Err(PathError::Reserved("\\".into()));
    }

    let components: Vec<&str> = path.split('/').collect();
    if components.len() > MAX_DEPTH {
        return Err(PathError::TooDeep);
    }
    for c in &components {
        if c.is_empty() || *c == "." || *c == ".." {
            return Err(PathError::Traversal);
        }
        if c.chars().any(|ch| ch.is_control()) {
            return Err(PathError::ControlChar);
        }
        if c.len() > MAX_COMPONENT_BYTES {
            return Err(PathError::ComponentTooLong);
        }
        if let Some(bad) = c
            .chars()
            .find(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        {
            return Err(PathError::Reserved(bad.to_string()));
        }
        if c.ends_with(' ') || c.ends_with('.') || is_windows_device_name(c) {
            return Err(PathError::Reserved((*c).to_string()));
        }
    }
    Ok(components)
}

fn has_drive_prefix(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn is_windows_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_nested_unicode_path() {
        assert_eq!(
            validate_relative_path("文档/报告 v2.pdf").unwrap(),
            vec!["文档", "报告 v2.pdf"]
        );
    }

    #[test]
    fn rejects_escapes() {
        for bad in ["../x", "a/../../x", "a//b", "./a", "a/."] {
            assert_eq!(
                validate_relative_path(bad),
                Err(PathError::Traversal),
                "{bad}"
            );
        }
        for bad in ["/etc/passwd", "\\\\server\\share", "C:x", "c:/x"] {
            assert_eq!(
                validate_relative_path(bad),
                Err(PathError::Absolute),
                "{bad}"
            );
        }
        assert!(validate_relative_path("a\\..\\..\\x").is_err());
    }

    #[test]
    fn rejects_windows_reserved() {
        for bad in [
            "CON",
            "aux.txt",
            "dir/com1.log",
            "LPT9",
            "trailing.",
            "space ",
        ] {
            assert!(
                matches!(validate_relative_path(bad), Err(PathError::Reserved(_))),
                "{bad}"
            );
        }
        assert!(validate_relative_path("COM0").is_ok());
        assert!(validate_relative_path("console.txt").is_ok());
    }

    #[test]
    fn rejects_control_and_depth() {
        assert_eq!(validate_relative_path("a\0b"), Err(PathError::ControlChar));
        let deep = vec!["d"; MAX_DEPTH + 1].join("/");
        assert_eq!(validate_relative_path(&deep), Err(PathError::TooDeep));
    }
}
