//! Classifying a failed turn as "the agent can't reach its provider".
//!
//! The Host decides; clients never parse messages. A turn's terminal error is
//! unreachable when it names a can't-connect errno, or when it ends in a
//! connection-flavoured error after the turn already saw connection retries.
//! An error that carries an HTTP status is never unreachable: the server
//! answered, so it was reachable.

/// The error code clients see for an unreachable turn.
pub const CODE: &str = "agent_unreachable";

/// What the user is told when the Codex watchdog gives up on a turn. Codex's
/// own text (`Connection failed: error sending request`) says less.
pub const CODEX_MESSAGE: &str = "Can't reach the server — check your internet connection";

/// Errno codes and phrases that mean the agent could not connect at all.
const CANT_CONNECT: &[&str] = &[
    "ECONNREFUSED",
    "ConnectionRefused",
    "ENOTFOUND",
    "ENETUNREACH",
    "ENETDOWN",
    "EHOSTUNREACH",
    "EHOSTDOWN",
    "EAI_AGAIN",
    "FailedToOpenSocket",
    "ERR_PROXY_TUNNEL",
    "ETIMEDOUT",
];

/// Claude's no-code can't-connect text. With a parenthesised code after it
/// (`Unable to connect to API (X)`) the code decides instead.
const UNABLE_TO_CONNECT: &str = "Unable to connect to API";

/// Failure categories that mean the provider answered.
const ANSWERED_CATEGORIES: &[&str] = &["limit", "access", "request"];

/// Whether a turn's terminal error means the agent can't reach its provider.
/// `category` is the failure category when the error came from a typed
/// failure record; `connection_retried` is whether the turn saw a connection
/// retry before it ended.
pub fn is_unreachable(message: &str, category: Option<&str>, connection_retried: bool) -> bool {
    if has_http_status(message) {
        return false;
    }
    if category.is_some_and(|c| ANSWERED_CATEGORIES.contains(&c)) {
        return false;
    }
    if names_cant_connect(message) {
        return true;
    }
    // A 5xx-style failure means the provider answered; a retry alone does
    // not make it unreachable.
    if category == Some("service") {
        return false;
    }
    connection_retried
}

/// The message itself says the agent could not connect: a can't-connect
/// errno, or the no-code `Unable to connect to API`.
fn names_cant_connect(message: &str) -> bool {
    CANT_CONNECT.iter().any(|needle| has_token(message, needle))
        || message
            .match_indices(UNABLE_TO_CONNECT)
            .any(|(at, _)| !message[at + UNABLE_TO_CONNECT.len()..].starts_with(" ("))
}

/// Whether `needle` is in `message` as a whole token: not part of a longer
/// word (`ETIMEDOUTS`, `XETIMEDOUTX`).
fn has_token(message: &str, needle: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    message.match_indices(needle).any(|(at, _)| {
        !message[..at].chars().next_back().is_some_and(word)
            && !message[at + needle.len()..]
                .chars()
                .next()
                .is_some_and(word)
    })
}

/// `API Error: NNN …`: the agent's way of reporting an HTTP status.
fn has_http_status(message: &str) -> bool {
    const PREFIX: &str = "API Error: ";
    message.match_indices(PREFIX).any(|(at, _)| {
        message[at + PREFIX.len()..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count()
            == 3
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_strings_classify_per_spec_table() {
        let unreachable = [
            "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)",
            "API Error: Can't reach the API server — check your internet or DNS (EAI_AGAIN)",
            "API Error: Can't reach the API server — check your internet or DNS (FailedToOpenSocket)",
            "API Error: No internet route — check your connection or VPN (ENETUNREACH)",
            "API Error: No internet route — check your connection or VPN (ENETDOWN)",
            "API Error: No internet route — check your connection or VPN (EHOSTUNREACH)",
            "API Error: No internet route — check your connection or VPN (EHOSTDOWN)",
            "API Error: Connection refused — a firewall or proxy may be blocking it (ECONNREFUSED)",
            "API Error: Connection refused — a firewall or proxy may be blocking it (ConnectionRefused)",
            "API Error: Couldn't connect through your proxy (ERR_PROXY_TUNNEL)",
            "API Error: Unable to connect to API (ETIMEDOUT)",
            "API Error: Unable to connect to API. Check your internet connection",
            "API Error: Unable to connect to API (ECONNREFUSED)",
        ];
        for message in unreachable {
            assert!(is_unreachable(message, None, false), "{message}");
        }
        for message in [
            "API Error: Connection dropped (ECONNRESET)",
            "API Error: Connection dropped (EPIPE)",
            "API Error: Connection dropped (StreamSuspended)",
        ] {
            assert!(!is_unreachable(message, None, false), "{message}");
        }
    }

    #[test]
    fn unable_to_connect_with_another_code_needs_a_retry() {
        let other = "API Error: Unable to connect to API (SOMETHING_ELSE)";
        assert!(!is_unreachable(other, None, false));
        assert!(is_unreachable(other, None, true));
        // The no-code form is unreachable on its own.
        assert!(is_unreachable("Unable to connect to API", None, false));
        assert!(is_unreachable(
            "API Error: Unable to connect to API.",
            None,
            false
        ));
    }

    #[test]
    fn server_errors_need_an_errno_not_just_a_retry() {
        let message = "API Error: Connection dropped (ECONNRESET)";
        assert!(!is_unreachable(message, Some("service"), true));
        assert!(is_unreachable(message, Some("connection"), true));
        assert!(is_unreachable(
            "service down (ECONNREFUSED)",
            Some("service"),
            false
        ));
    }

    #[test]
    fn dropped_connection_counts_after_retry() {
        assert!(is_unreachable(
            "API Error: Connection dropped (ECONNRESET)",
            None,
            true
        ));
    }

    /// An errno counts as a whole token, never inside a longer word.
    #[test]
    fn errno_inside_a_longer_word_does_not_classify() {
        for message in [
            "request failed: XETIMEDOUTX",
            "ETIMEDOUTS while waiting",
            "retry_ECONNREFUSED_count exceeded",
            "NOT_ENOTFOUND",
        ] {
            assert!(!is_unreachable(message, None, false), "{message}");
        }
        for message in [
            "(ETIMEDOUT)",
            "connect ECONNREFUSED 127.0.0.1:443",
            "getaddrinfo ENOTFOUND api.example.com",
            "failed: ERR_PROXY_TUNNEL.",
            "ETIMEDOUT",
        ] {
            assert!(is_unreachable(message, None, false), "{message}");
        }
    }

    #[test]
    fn http_status_is_never_unreachable() {
        for message in [
            "API Error: 429 rate limited",
            "API Error: 401 invalid credentials",
            "API Error: 500 internal server error",
        ] {
            assert!(!is_unreachable(message, None, true), "{message}");
            assert!(!is_unreachable(message, None, false), "{message}");
        }
        assert!(!is_unreachable("usage limit reached", Some("limit"), true));
        assert!(!is_unreachable(
            "Connection failed (ECONNREFUSED)",
            Some("access"),
            true
        ));
        assert!(!is_unreachable("bad request", Some("request"), true));
    }
}
