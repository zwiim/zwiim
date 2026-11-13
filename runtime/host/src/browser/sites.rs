//! Zwiim-owned: which sites a tab's page loads may reach (browser design
//! 13.2). Everything that decides is a function of its arguments; the
//! service gathers them for each document request the engine pauses.
//!
//! 1. Sites the administrator's policy file blocks: never, in any tab.
//! 2. What a person navigates to, and what frames inside a page load: yes.
//! 3. The session's own dev servers: yes.
//! 4. A session with a strict site list: only the sites on it.
//! 5. Otherwise yes, unless the person turned on "Ask before agents open
//!    new sites": then a site nobody approved for the session is asked
//!    about, local addresses included.
//!
//! A public name that resolves to a local address (DNS rebinding) is not
//! the site the person approved or listed: it is asked about, or refused.

use std::net::IpAddr;

use url::Url;

use super::policy::{self, HostRule, HostRuleAction};

pub const ERROR_SITE_BLOCKED: &str = "site_blocked";
pub const ERROR_SITE_NOT_ALLOWED: &str = "site_not_allowed";
pub const ERROR_SITE_DENIED: &str = "site_denied";

/// Most sites on a strict site list.
pub const MAX_SITES: usize = 100;

/// Who made a tab navigate: whoever acted on it last, and until someone
/// does, whoever opened it. A page that navigates on its own counts as
/// theirs too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Person,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Refused, with the error code the agent gets.
    Refuse(&'static str),
    /// Ask the person.
    Ask,
}

/// One document request, and what the service knows about its tab.
pub struct Check<'a> {
    pub url: &'a Url,
    /// The tab's own page, not a frame inside it.
    pub main_frame: bool,
    pub actor: Actor,
    /// The administrator's rules.
    pub managed: &'a [HostRule],
    /// The session's strict site list, as site patterns; empty for none.
    pub strict: &'a [String],
    /// "Ask before agents open new sites" is on.
    pub ask: bool,
    /// Sites the person allowed, for the session or always.
    pub approved: &'a [String],
    /// Sites the person denied for the session.
    pub denied: &'a [String],
    /// A loopback server running from the session's worktree.
    pub workspace_server: bool,
    /// The name resolves to a loopback, private or link-local address.
    pub resolves_local: bool,
}

pub fn decide(c: &Check) -> Verdict {
    if blocked(c.managed, c.url) {
        return Verdict::Refuse(ERROR_SITE_BLOCKED);
    }
    if c.actor == Actor::Person || !c.main_frame || c.workspace_server {
        return Verdict::Allow;
    }
    // A listed or approved name that turns out to be this machine or the
    // local network is not what was listed or approved.
    let rebound = c.resolves_local && !is_local_host(c.url);
    if !c.strict.is_empty() {
        return if listed(c.strict, c.url) && !rebound {
            Verdict::Allow
        } else {
            Verdict::Refuse(ERROR_SITE_NOT_ALLOWED)
        };
    }
    if !c.ask {
        return Verdict::Allow;
    }
    if listed(c.denied, c.url) {
        return Verdict::Refuse(ERROR_SITE_DENIED);
    }
    if listed(c.approved, c.url) && !rebound {
        return Verdict::Allow;
    }
    Verdict::Ask
}

/// The administrator's policy file blocks `url`.
pub fn blocked(managed: &[HostRule], url: &Url) -> bool {
    policy::match_host_rule(managed, url).is_some_and(|r| r.action == HostRuleAction::Block)
}

/// `url` matches one of `patterns` (`example.com`, `*.example.com`,
/// `localhost:3000`).
pub fn listed(patterns: &[String], url: &Url) -> bool {
    let rules: Vec<HostRule> = patterns
        .iter()
        .map(|p| HostRule {
            pattern: p.clone(),
            action: HostRuleAction::Builtin,
        })
        .collect();
    policy::match_host_rule(&rules, url).is_some()
}

/// The pattern that approves `url`'s site alone: its host and port, so a
/// subdomain is a site of its own.
pub fn site_of(url: &Url) -> Option<String> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    let port = url.port_or_known_default()?;
    (!host.is_empty()).then(|| format!("{host}:{port}"))
}

/// A strict site list entry as the person typed it, checked and trimmed.
pub fn site_pattern(raw: &str) -> Option<String> {
    let trimmed = raw.trim_matches(|c: char| c.is_ascii_whitespace());
    // Pasted addresses work too: `https://example.com/x` is `example.com`.
    let pattern = match Url::parse(trimmed) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => {
            let host = u.host_str()?.to_ascii_lowercase();
            match u.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            }
        }
        _ => trimmed.to_ascii_lowercase(),
    };
    policy::valid_pattern(&pattern).then_some(pattern)
}

/// `url` names this machine or the local network itself: an address in
/// those ranges, `localhost` or under it, or a `.local` name.
pub fn is_local_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(a)) => is_local_addr(IpAddr::V4(a)),
        Some(url::Host::Ipv6(a)) => is_local_addr(IpAddr::V6(a)),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost") || name.ends_with(".local")
        }
        None => false,
    }
}

/// Loopback, private, link-local (the cloud metadata address among them),
/// unspecified, or one of those inside an IPv4-mapped IPv6 address.
pub fn is_local_addr(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(a) => {
            a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                // Carrier-grade NAT, 100.64.0.0/10.
                || (a.octets()[0] == 100 && (a.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_local_addr(IpAddr::V4(v4));
            }
            let first = a.segments()[0];
            a.is_loopback()
                || a.is_unspecified()
                // Unique local, fc00::/7, and link-local, fe80::/10.
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn check<'a>(url: &'a Url) -> Check<'a> {
        Check {
            url,
            main_frame: true,
            actor: Actor::Agent,
            managed: &[],
            strict: &[],
            ask: false,
            approved: &[],
            denied: &[],
            workspace_server: false,
            resolves_local: false,
        }
    }

    #[test]
    fn by_default_an_agent_goes_anywhere_but_where_the_administrator_blocks() {
        let url = u("https://example.com/");
        assert_eq!(decide(&check(&url)), Verdict::Allow);
        let local = u("http://169.254.169.254/latest/");
        assert_eq!(decide(&check(&local)), Verdict::Allow);

        let managed = [HostRule {
            pattern: "*.bad.test".into(),
            action: HostRuleAction::Block,
        }];
        let bad = u("https://www.bad.test/");
        for actor in [Actor::Person, Actor::Agent] {
            for main_frame in [true, false] {
                let c = Check {
                    managed: &managed,
                    actor,
                    main_frame,
                    ..check(&bad)
                };
                assert_eq!(decide(&c), Verdict::Refuse(ERROR_SITE_BLOCKED));
            }
        }
    }

    #[test]
    fn a_strict_list_limits_the_agent_and_never_the_person() {
        let strict = strings(&["github.com", "*.example.com", "localhost:3000"]);
        let verdict = |url: &str, actor: Actor| {
            let url = u(url);
            decide(&Check {
                strict: &strict,
                actor,
                ..check(&url)
            })
        };
        assert_eq!(
            verdict("https://github.com/x", Actor::Agent),
            Verdict::Allow
        );
        assert_eq!(
            verdict("https://a.example.com/", Actor::Agent),
            Verdict::Allow
        );
        assert_eq!(
            verdict("http://localhost:3000/", Actor::Agent),
            Verdict::Allow
        );
        for refused in [
            "https://api.github.com/",
            "https://example.com/",
            "http://localhost:3001/",
        ] {
            assert_eq!(
                verdict(refused, Actor::Agent),
                Verdict::Refuse(ERROR_SITE_NOT_ALLOWED),
                "{refused}"
            );
        }
        assert_eq!(
            verdict("https://api.github.com/", Actor::Person),
            Verdict::Allow
        );

        // Frames inside an allowed page, and the session's own dev server.
        let url = u("https://cdn.other.test/embed");
        let frame = Check {
            strict: &strict,
            main_frame: false,
            ..check(&url)
        };
        assert_eq!(decide(&frame), Verdict::Allow);
        let url = u("http://localhost:5173/");
        let server = Check {
            strict: &strict,
            workspace_server: true,
            ..check(&url)
        };
        assert_eq!(decide(&server), Verdict::Allow);

        // A listed name that resolves to this machine is not what was listed.
        let url = u("https://github.com/");
        let rebound = Check {
            strict: &strict,
            resolves_local: true,
            ..check(&url)
        };
        assert_eq!(decide(&rebound), Verdict::Refuse(ERROR_SITE_NOT_ALLOWED));
    }

    #[test]
    fn asking_covers_new_sites_and_local_addresses() {
        let approved = strings(&["example.com:443", "localhost:8080"]);
        let denied = strings(&["evil.test:443"]);
        let verdict = |url: &str, resolves_local: bool| {
            let url = u(url);
            decide(&Check {
                ask: true,
                approved: &approved,
                denied: &denied,
                resolves_local,
                ..check(&url)
            })
        };
        assert_eq!(verdict("https://example.com/a", false), Verdict::Allow);
        assert_eq!(verdict("http://localhost:8080/", true), Verdict::Allow);
        // Subdomains and other ports are sites of their own.
        assert_eq!(verdict("https://www.example.com/", false), Verdict::Ask);
        assert_eq!(verdict("http://example.com/", false), Verdict::Ask);
        assert_eq!(verdict("http://localhost:9000/", true), Verdict::Ask);
        assert_eq!(verdict("http://169.254.169.254/", true), Verdict::Ask);
        assert_eq!(
            verdict("https://evil.test/", false),
            Verdict::Refuse(ERROR_SITE_DENIED)
        );
        // Approved by name, then rebound to this machine.
        assert_eq!(verdict("https://example.com/a", true), Verdict::Ask);
    }

    #[test]
    fn a_site_is_its_host_and_port() {
        assert_eq!(
            site_of(&u("https://Example.com./x")).as_deref(),
            Some("example.com:443")
        );
        assert_eq!(
            site_of(&u("http://localhost:5173/")).as_deref(),
            Some("localhost:5173")
        );
        assert_eq!(
            site_of(&u("http://[::1]:3000/")).as_deref(),
            Some("[::1]:3000")
        );
        assert_eq!(site_of(&u("about:blank")), None);
        let site = site_of(&u("http://[::1]:3000/")).unwrap();
        assert!(listed(&[site], &u("http://[::1]:3000/other")));
    }

    #[test]
    fn list_entries_are_patterns_or_pasted_addresses() {
        assert_eq!(site_pattern(" GitHub.com ").as_deref(), Some("github.com"));
        assert_eq!(
            site_pattern("*.example.com").as_deref(),
            Some("*.example.com")
        );
        assert_eq!(
            site_pattern("https://docs.rs/serde").as_deref(),
            Some("docs.rs")
        );
        assert_eq!(
            site_pattern("http://localhost:3000/").as_deref(),
            Some("localhost:3000")
        );
        for bad in ["", "exa mple.com", "file:///etc", "a..b", "example.com:0"] {
            assert_eq!(site_pattern(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn local_hosts_and_addresses() {
        for local in [
            "http://localhost/",
            "http://app.localhost/",
            "http://printer.local/",
            "http://127.0.0.2/",
            "http://10.1.2.3/",
            "http://172.16.0.1/",
            "http://192.168.1.1/",
            "http://169.254.169.254/",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            assert!(is_local_host(&u(local)), "{local}");
        }
        for public in [
            "https://example.com/",
            "http://8.8.8.8/",
            "http://172.32.0.1/",
            "http://[2001:db8::1]/",
        ] {
            assert!(!is_local_host(&u(public)), "{public}");
        }
    }
}
