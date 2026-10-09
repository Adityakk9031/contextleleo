//! Credential scrubbing shared by the Jev client and the display paths.
//!
//! Two replacements, one scanner. [`redact`] swaps each credential for
//! [`REDACTED`] in the excerpt that is sent to the Jev API; [`mask`] swaps it
//! for [`STARS`] in anything shown to a person (`view`, `query`, retrieved
//! chunks), so a screen recording shows that a `.env`-style value exists
//! without showing it. Names stay readable (`JEV_API_KEY=********`), so the
//! reader knows which variable to set; the real value stays in the stored
//! session and is only revealed on request.
//!
//! The scanner is hand-rolled (no regex dependency) and errs toward
//! replacing: a false positive costs legibility, a false negative leaks a key.

use crate::common::{Block, Message, Tool, ToolOutput};

/// What [`mask`] puts where a credential was.
pub const STARS: &str = "********";

/// Replace every credential in `content` with [`STARS`].
#[must_use]
pub fn mask(content: &str) -> String {
    redact_with(content, STARS).0
}

/// Mask every string a message carries to a reader: text, reasoning, tool
/// arguments, and tool output. Structure, ids, and the stored session are
/// untouched; only the in-memory copy changes.
pub fn mask_message(message: &mut Message) {
    for block in &mut message.content {
        match block {
            Block::Text { text } | Block::Thinking { text, .. } => *text = mask(text),
            Block::ToolResult { content, .. } => match content {
                ToolOutput::Text(text) => *text = mask(text),
                ToolOutput::Json(value) => mask_json(value),
            },
            Block::ToolUse { tool, .. } => mask_tool(tool),
            Block::Image { .. } | Block::Artifact { .. } => {}
        }
    }
}

/// Mask each message of a body in place.
pub fn mask_messages(messages: &mut [Message]) {
    for message in messages {
        mask_message(message);
    }
}

fn mask_tool(tool: &mut Tool) {
    // Typed tools round-trip through their canonical JSON so every string
    // field (command, file content, edit text) is covered without a match
    // arm per variant; a tool that does not round-trip is left as it was.
    let Ok(mut value) = serde_json::to_value(&*tool) else {
        return;
    };
    mask_json(&mut value);
    if let Ok(masked) = serde_json::from_value::<Tool>(value) {
        *tool = masked;
    }
}

fn mask_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => *text = mask(text),
        serde_json::Value::Array(items) => items.iter_mut().for_each(mask_json),
        serde_json::Value::Object(map) => map.values_mut().for_each(mask_json),
        _ => {}
    }
}

/// Stands in for anything [`redact`] removed: the text keeps its shape, so a
/// relevance judgment can still see that a value was there and where.
pub const REDACTED: &str = "[redacted]";

/// A credential format to scrub: literal prefix, then the minimum run of
/// token characters that has to follow it before it counts. Prefixes are the
/// vendors' own published formats, so no guessing about what "looks random"
/// is needed — and a false positive costs only legibility, while a false
/// negative sends a live key to a third party.
struct SecretShape {
    prefix: &'static str,
    min_suffix: usize,
}

/// Vendor credential prefixes scrubbed from every candidate excerpt.
const SECRET_SHAPES: &[SecretShape] = &[
    SecretShape {
        prefix: "sk-",
        min_suffix: 16,
    },
    SecretShape {
        prefix: "sk_",
        min_suffix: 16,
    },
    SecretShape {
        prefix: "AKIA",
        min_suffix: 16,
    },
    SecretShape {
        prefix: "ASIA",
        min_suffix: 16,
    },
    SecretShape {
        prefix: "ghp_",
        min_suffix: 20,
    },
    SecretShape {
        prefix: "gho_",
        min_suffix: 20,
    },
    SecretShape {
        prefix: "ghs_",
        min_suffix: 20,
    },
    SecretShape {
        prefix: "ghr_",
        min_suffix: 20,
    },
    SecretShape {
        prefix: "github_pat_",
        min_suffix: 20,
    },
    SecretShape {
        prefix: "xoxa-",
        min_suffix: 10,
    },
    SecretShape {
        prefix: "xoxb-",
        min_suffix: 10,
    },
    SecretShape {
        prefix: "xoxp-",
        min_suffix: 10,
    },
    SecretShape {
        prefix: "xoxr-",
        min_suffix: 10,
    },
    SecretShape {
        prefix: "AIza",
        min_suffix: 30,
    },
    SecretShape {
        prefix: "ya29.",
        min_suffix: 20,
    },
    SecretShape {
        prefix: "glpat-",
        min_suffix: 20,
    },
    SecretShape {
        prefix: "npm_",
        min_suffix: 30,
    },
    SecretShape {
        prefix: "pypi-",
        min_suffix: 20,
    },
    SecretShape {
        prefix: "hf_",
        min_suffix: 30,
    },
    SecretShape {
        prefix: "SG.",
        min_suffix: 20,
    },
    SecretShape {
        prefix: "Bearer ",
        min_suffix: 12,
    },
    SecretShape {
        prefix: "bearer ",
        min_suffix: 12,
    },
    SecretShape {
        prefix: "rk_live_",
        min_suffix: 20,
    },
    SecretShape {
        prefix: "sk_live_",
        min_suffix: 20,
    },
];

/// Assignment names whose *value* is a credential, whatever its shape —
/// `api_key=…`, `"password": "…"`, `AWS_SECRET_ACCESS_KEY=…`. The name is
/// kept; only the value is replaced.
const SENSITIVE_NAMES: &[&str] = &[
    "api_key",
    "api-key",
    "apikey",
    "api_secret",
    "access_key",
    "access-key",
    "secret_key",
    "secret-key",
    "client_secret",
    "private_key",
    "private-key",
    "password",
    "passwd",
    "passphrase",
    "token",
    "access_token",
    "refresh_token",
    "auth_token",
    "aws_secret_access_key",
    "database_url",
    "redis_url",
];

/// Scrub credential-shaped strings from one candidate excerpt, reporting how
/// many were replaced.
///
/// This runs on the copy that goes to Jev and nowhere else: local history is
/// never rewritten, so `view`, `export`, and the original store keep the real
/// value while the ranking request carries [`REDACTED`] in its place. The
/// scanner is hand-rolled because the crate has no regex dependency, and it
/// errs toward replacing: an over-eager hit costs legibility in a relevance
/// judgment, an under-eager one leaks a key off the machine.
#[must_use]
pub fn redact(content: &str) -> (String, usize) {
    redact_with(content, REDACTED)
}

/// [`redact`] with a caller-chosen replacement for each credential.
fn redact_with(content: &str, replacement: &str) -> (String, usize) {
    let mut out = String::with_capacity(content.len());
    let mut count = 0usize;
    let mut index = 0usize;
    while index < content.len() {
        if let Some((start, end)) = secret_at(content, index) {
            out.push_str(&content[index..start]);
            out.push_str(replacement);
            count += 1;
            index = end;
            continue;
        }
        let Some(ch) = content[index..].chars().next() else {
            break;
        };
        out.push(ch);
        index += ch.len_utf8();
    }
    (out, count)
}

/// The span of the credential starting at byte `index`, when there is one.
/// `start` is at or after `index`: a named secret replaces only its value, so
/// the name stays readable.
fn secret_at(content: &str, index: usize) -> Option<(usize, usize)> {
    let rest = &content[index..];
    // A PEM block: every line between BEGIN and the end of the END line is
    // key material, so the whole block goes. A truncated block (no END) is
    // redacted to the end of the text, since everything after BEGIN is
    // secret by construction.
    if rest.starts_with("-----BEGIN") {
        let end = rest
            .find("-----END")
            .and_then(|stop| rest[stop..].find('\n').map(|newline| stop + newline))
            .unwrap_or(rest.len());
        return Some((index, index + end));
    }
    for shape in SECRET_SHAPES {
        if rest.starts_with(shape.prefix) {
            let start = index + shape.prefix.len();
            let end = token_end(content, start);
            if end - start >= shape.min_suffix {
                return Some((index, end));
            }
        }
    }
    // A JSON Web Token: three base64url segments, the first of which always
    // begins `eyJ` (base64 of `{"`).
    if rest.starts_with("eyJ") {
        let end = token_end(content, index);
        let token = &content[index..end];
        if token.len() >= 24 && token.matches('.').count() == 2 {
            return Some((index, end));
        }
    }
    if starts_word(content, index) {
        return named_secret_at(content, index);
    }
    None
}

/// The value span of a `NAME=value` / `"NAME": "value"` assignment whose
/// name is in [`SENSITIVE_NAMES`], when one starts at `index`.
fn named_secret_at(content: &str, index: usize) -> Option<(usize, usize)> {
    let name_len = sensitive_name_len(&content[index..])?;
    let bytes = content.as_bytes();
    let mut cursor = index + name_len;
    // Quotes and spaces can sit on either side of the separator:
    // `"api_key" : "…"`, `api_key=…`, `api-key = …`.
    while cursor < bytes.len() && matches!(bytes[cursor], b'"' | b'\'' | b' ') {
        cursor += 1;
    }
    if !matches!(bytes.get(cursor), Some(b'=' | b':')) {
        return None;
    }
    cursor += 1;
    while bytes.get(cursor) == Some(&b' ') {
        cursor += 1;
    }
    // A quoted value runs to its closing quote; a bare one to whitespace or
    // a delimiter (`token=abc123;` ends at the semicolon).
    let quote = bytes
        .get(cursor)
        .filter(|byte| matches!(byte, b'"' | b'\''));
    let end = match quote {
        Some(quote) => {
            cursor += 1;
            let closing = char::from(*quote);
            content[cursor..]
                .find(closing)
                .map_or(content.len(), |offset| cursor + offset)
        }
        None => content[cursor..]
            .find(|c: char| c.is_whitespace() || matches!(c, ';' | ',' | ')' | '}' | ']'))
            .map_or(content.len(), |offset| cursor + offset),
    };
    (end > cursor).then_some((cursor, end))
}

/// Length of the sensitive name that starts `rest`, if it does: either one of
/// [`SENSITIVE_NAMES`] standing alone, or a whole `ENV_STYLE_IDENTIFIER` that
/// names a credential — `JEV_API_KEY`, `STRIPE_SECRET`, `DB_PASSWORD`,
/// `GITHUB_TOKEN` — which the listed names alone would miss because they only
/// match at the start of a word.
fn sensitive_name_len(rest: &str) -> Option<usize> {
    if let Some(name) = SENSITIVE_NAMES.iter().find(|name| {
        rest.get(..name.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(name))
            && rest[name.len()..].starts_with(|c: char| !c.is_alphanumeric() && c != '_')
    }) {
        return Some(name.len());
    }
    let len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let identifier = rest[..len].to_ascii_lowercase();
    let names_a_credential = identifier.ends_with("_key")
        || identifier.ends_with("apikey")
        || ["secret", "password", "passwd", "credential"]
            .iter()
            .any(|word| identifier.contains(word))
        // `max_tokens=4000` counts tokens, it is not a token.
        || (identifier.contains("token") && !identifier.ends_with("tokens"));
    (len > 0 && names_a_credential).then_some(len)
}

/// Whether a word starts at `index`, so `api_key` matches in `api_key=…` but
/// never inside `my_api_key_name`.
fn starts_word(content: &str, index: usize) -> bool {
    index == 0
        || content
            .as_bytes()
            .get(index - 1)
            .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
}

/// Just past the run of token characters starting at `start` — the alphabet a
/// credential value uses (`A-Za-z0-9`, plus `-_.+/=`). Multibyte characters
/// are never token bytes, so the result is always a character boundary.
fn token_end(content: &str, start: usize) -> usize {
    let bytes = content.as_bytes();
    let mut end = start;
    while bytes.get(end).is_some_and(|byte| is_token_byte(*byte)) {
        end += 1;
    }
    end
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+' | b'/' | b'=')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{Role, ToolOutput};
    use chrono::Utc;

    #[test]
    fn env_style_names_keep_their_name_and_lose_their_value() {
        let dotenv = "JEV_API_KEY=jev_live_abc123def456ghi789\nDB_PASSWORD=hunter2hunter2\n\
                      GITHUB_TOKEN: \"not-a-known-shape-value\"\nSTRIPE_SECRET=whsec_zzzzzzzz";
        let masked = mask(dotenv);
        assert!(masked.contains("JEV_API_KEY=********"), "{masked}");
        assert!(masked.contains("DB_PASSWORD=********"), "{masked}");
        assert!(masked.contains("GITHUB_TOKEN: \"********\""), "{masked}");
        assert!(masked.contains("STRIPE_SECRET=********"), "{masked}");
        for leaked in ["abc123def456", "hunter2", "not-a-known-shape", "whsec_zzzz"] {
            assert!(!masked.contains(leaked), "{leaked} leaked: {masked}");
        }
    }

    #[test]
    fn ordinary_assignments_survive() {
        for text in [
            "max_tokens=4000",
            "PORT=3000",
            "monkey=banana",
            "the key idea is simple",
            "keyboard: qwerty",
            "RUST_LOG=debug",
        ] {
            assert_eq!(mask(text), text, "{text}");
        }
    }

    #[test]
    fn mask_and_redact_use_their_own_replacement() {
        let text = "sk-abcdefghijklmnopqrstuvwxyz012345";
        assert_eq!(mask(text), STARS);
        assert_eq!(redact(text), (REDACTED.to_string(), 1));
    }

    #[test]
    fn mask_message_covers_text_tool_arguments_and_tool_output() {
        let secret = "sk-abcdefghijklmnopqrstuvwxyz012345";
        let mut message = Message {
            role: Role::Assistant,
            content: vec![
                Block::Text {
                    text: format!("export OPENAI_API_KEY={secret}"),
                },
                Block::ToolUse {
                    id: "t1".into(),
                    tool: Tool::Bash {
                        command: format!("echo JEV_API_KEY=plain-secret-value-1234 {secret}"),
                        workdir: None,
                        timeout_ms: None,
                        description: None,
                        run_in_background: false,
                    },
                },
                Block::ToolResult {
                    tool_use_id: "t1".into(),
                    content: ToolOutput::Json(
                        serde_json::json!({"out": format!("token={secret}")}),
                    ),
                    is_error: false,
                },
            ],
            timestamp: Utc::now(),
            model: None,
            stop_reason: None,
            usage: None,
        };
        mask_message(&mut message);
        let rendered = format!("{message:?}");
        assert!(!rendered.contains(secret), "{rendered}");
        assert!(!rendered.contains("plain-secret-value"), "{rendered}");
        assert!(rendered.contains("OPENAI_API_KEY=********"), "{rendered}");
        assert!(rendered.contains("JEV_API_KEY=********"), "{rendered}");
    }
}
