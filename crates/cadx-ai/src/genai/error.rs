//! Normalization of untrusted provider error text into bounded messages.

const MAX_PROVIDER_ERROR_CHARS: usize = 800;

pub(super) fn provider_error_message(message: &str) -> String {
    if message.contains("text/html") || message.contains("<!DOCTYPE html") {
        return "provider returned HTML instead of JSON; verify provider.endpoint is an API base URL and gateway API authentication is enabled".into();
    }

    let mut compact = String::new();
    let mut previous_was_whitespace = false;
    let mut truncated = false;
    for (character_count, character) in message.chars().enumerate() {
        if character_count >= MAX_PROVIDER_ERROR_CHARS {
            truncated = true;
            break;
        }
        if character.is_whitespace() {
            if !previous_was_whitespace {
                compact.push(' ');
            }
            previous_was_whitespace = true;
        } else {
            compact.push(character);
            previous_was_whitespace = false;
        }
    }
    let compact = compact.trim();
    if truncated {
        format!("{compact}...")
    } else {
        compact.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_errors_hide_html_and_bound_untrusted_response_text() {
        let html = provider_error_message(
            "Response content type 'text/html' is not JSON: <!DOCTYPE html><title>Login</title>",
        );
        assert!(html.contains("returned HTML instead of JSON"));
        assert!(!html.contains("<title>"));

        let long = provider_error_message(&"provider failure ".repeat(200));
        assert!(long.ends_with("..."));
        assert!(long.chars().count() <= 803);
    }
}
