use regex::Regex;
use std::sync::LazyLock;

const REDACTED: &str = "[REDACTED]";

pub fn redact_secrets(text: &str) -> String {
    if !has_secret_indicator(text) {
        return text.to_owned();
    }

    let text = env_assignment_regex()
        .replace_all(text, |captures: &regex::Captures<'_>| {
            format!("{}={}", &captures[1], REDACTED)
        })
        .into_owned();
    let text = bearer_regex()
        .replace_all(&text, |captures: &regex::Captures<'_>| {
            format!("{} {}", &captures[1], REDACTED)
        })
        .into_owned();
    let text = key_value_regex()
        .replace_all(&text, |captures: &regex::Captures<'_>| {
            format!("{}{}", &captures[1], REDACTED)
        })
        .into_owned();
    let text = private_key_block_regex()
        .replace_all(&text, REDACTED)
        .into_owned();
    token_regex().replace_all(&text, REDACTED).into_owned()
}

fn has_secret_indicator(text: &str) -> bool {
    [
        "TOKEN",
        "token",
        "SECRET",
        "secret",
        "PASSWORD",
        "password",
        "PASS",
        "API_KEY",
        "api_key",
        "api-key",
        "ACCESS_KEY",
        "access_key",
        "PRIVATE_KEY",
        "private_key",
        "PRIVATE KEY",
        "private key",
        "BEGIN PRIVATE KEY",
        "DSN",
        "dsn",
        "COOKIE",
        "cookie",
        "Authorization",
        "authorization",
        "Bearer ",
        "bearer ",
        "sk-",
        "github_pat_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "xox",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

static ENV_ASSIGNMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b([A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|PASS|API_KEY|ACCESS_KEY|PRIVATE_KEY|DSN|COOKIE|AUTHORIZATION)[A-Z0-9_]*)\s*=\s*([^\s"']+)"#,
    )
    .expect("env assignment redaction regex compiles")
});

fn env_assignment_regex() -> &'static Regex {
    &ENV_ASSIGNMENT_REGEX
}

static KEY_VALUE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(["']?(?:token|secret|password|api[_-]?key|access[_-]?key|private[_-]?key|dsn|cookie)["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,}]+)"#,
    )
    .expect("key-value redaction regex compiles")
});

fn key_value_regex() -> &'static Regex {
    &KEY_VALUE_REGEX
}

static BEARER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(Bearer)\s+[A-Za-z0-9._~+/=-]{12,}"#)
        .expect("bearer redaction regex compiles")
});

fn bearer_regex() -> &'static Regex {
    &BEARER_REGEX
}

static TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"\b(?:sk-[A-Za-z0-9_-]{16,}|github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{16,})\b"#,
    )
    .expect("token redaction regex compiles")
});

fn token_regex() -> &'static Regex {
    &TOKEN_REGEX
}

static PRIVATE_KEY_BLOCK_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----"#)
        .expect("private key block redaction regex compiles")
});

fn private_key_block_regex() -> &'static Regex {
    &PRIVATE_KEY_BLOCK_REGEX
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(parts: &[&str]) -> String {
        parts.concat()
    }

    #[test]
    fn redacts_credential_assignments_without_redacting_plain_language() {
        let text =
            "The webhook secret was missing. API_KEY=abc123456789 SENTRY_DSN=https://public@dsn";

        let redacted = redact_secrets(text);

        assert!(redacted.contains("The webhook secret was missing."));
        assert!(redacted.contains("API_KEY=[REDACTED]"));
        assert!(redacted.contains("SENTRY_DSN=[REDACTED]"));
        assert!(!redacted.contains("abc123456789"));
        assert!(!redacted.contains("public@dsn"));
    }

    #[test]
    fn redacts_auth_headers_and_common_token_prefixes() {
        let github_pat = joined(&["github", "_pat_", "1234567890abcdefghijklmnop"]);
        let openai_key = joined(&["sk", "-", "abcdefghijklmnopqrstuvwxyz"]);
        let text = format!(
            r#"{{"authorization":"Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9","token":"{github_pat}","openai":"{openai_key}"}}"#
        );

        let redacted = redact_secrets(&text);

        assert!(redacted.contains(r#""authorization":"Bearer [REDACTED]""#));
        assert!(redacted.contains(r#""token":[REDACTED]"#));
        assert!(redacted.contains(r#""openai":"[REDACTED]""#));
        assert!(!redacted.contains("eyJhbGci"));
        assert!(!redacted.contains(&github_pat));
        assert!(!redacted.contains(&openai_key));
    }

    #[test]
    fn redacts_fixture_corpus_without_leaking_secret_values() {
        let openai_key = joined(&["sk", "-proj-", "1234567890abcdefghijklmnop"]);
        let stripe_webhook_secret = joined(&["whsec", "_", "1234567890abcdefghijklmnop"]);
        let fixtures = [
            (format!("OPENAI_API_KEY = \"{openai_key}\""), openai_key),
            (
                format!("STRIPE_WEBHOOK_SECRET='{stripe_webhook_secret}'"),
                stripe_webhook_secret,
            ),
            (
                "Authorization: Bearer abcdefghijklmnopqrstuvwxyz.1234567890".to_owned(),
                "abcdefghijklmnopqrstuvwxyz.1234567890".to_owned(),
            ),
            (
                r#"{"cookie":"sessionid=abc123456789; path=/"}"#.to_owned(),
                "sessionid=abc123456789".to_owned(),
            ),
            (
                "password : \"correct horse battery staple\"".to_owned(),
                "correct horse battery staple".to_owned(),
            ),
            (
                "-----BEGIN PRIVATE KEY-----\nabc123secret\n-----END PRIVATE KEY-----".to_owned(),
                "abc123secret".to_owned(),
            ),
        ];

        for (input, leaked_value) in fixtures {
            let redacted = redact_secrets(&input);

            assert!(
                redacted.contains(REDACTED),
                "fixture was not redacted: {input}"
            );
            assert!(
                !redacted.contains(&leaked_value),
                "fixture leaked {leaked_value}: {redacted}"
            );
        }
    }
}
