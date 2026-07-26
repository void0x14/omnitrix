//! Tool çağrı validasyonu — JSON-Schema + free-text shell command detection
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    Valid,
    Invalid(String),
    DetectedShellCommand(String),
}

#[derive(Debug)]
pub struct ToolValidator;

impl ToolValidator {
    /// JSON argümanları şemaya göre validate et
    pub fn validate_args(_tool_name: &str, args: &Value) -> ValidationResult {
        // Free-text shell command detection
        if let Some(text) = args.get("command").and_then(|v| v.as_str()) {
            let shell_indicators = ["sh ", "bash ", "sudo ", "rm -rf", "chmod ", "> ", "| "];
            for indicator in &shell_indicators {
                if text.contains(indicator) {
                    return ValidationResult::DetectedShellCommand(text.to_string());
                }
            }
        }

        // JSON-Schema validation (temel seviye)
        if !args.is_object() && !args.is_array() {
            return ValidationResult::Invalid("args must be object or array".into());
        }

        ValidationResult::Valid
    }

    /// Shell komutu tespiti
    pub fn contains_shell_command(text: &str) -> bool {
        let patterns = [
            "sh ", "bash ", "zsh ", "sudo ", "su ",
            "rm -rf", "mkfs.", "dd if=", "chmod 777",
            "> /dev/", "| sh", "`", "$(",
        ];
        patterns.iter().any(|p| text.contains(p))
    }
}
