//! Checking web addresses before they are opened.
//!
//! Everything that is not plainly an `http` or `https` address is
//! refused: the shell would happily run `file:` paths, UNC shares and any
//! protocol some app registered.

/// The longest address that is opened.
const MAX_LEN: usize = 2048;

/// Checks `text` and returns the address to open. A bare host ("github.com")
/// becomes an `https` address.
///
/// # Errors
/// Returns why it is refused, for the model.
pub fn validate(text: &str) -> Result<String, &'static str> {
    let text = text.trim();
    if text.len() > MAX_LEN {
        return Err("the address is too long");
    }
    // Anything else must be percent-encoded in an address, and these
    // characters could mean something to the shell or the browser's
    // command line.
    let odd = |c: char| !c.is_ascii_graphic() || matches!(c, '"' | '\\' | '<' | '>' | '^' | '`' | '|');
    if text.contains(odd) {
        return Err("the address has characters an address cannot have");
    }
    let lower = text.to_ascii_lowercase();
    let (scheme, rest) = if lower.starts_with("https://") {
        ("https", text.get(8..).unwrap_or_default())
    } else if lower.starts_with("http://") {
        ("http", text.get(7..).unwrap_or_default())
    } else {
        ("https", text)
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    let with_scheme = rest.len() != text.len();
    check_authority(authority, with_scheme)?;
    Ok(format!("{scheme}://{authority}{path}"))
}

fn check_authority(authority: &str, with_scheme: bool) -> Result<(), &'static str> {
    // "https://github.com@evil.com" goes to evil.com.
    if authority.contains('@') {
        return Err("the address has a user name in it");
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if let Some(port) = port {
        // Without http:// in front, "name:rest" could be any protocol.
        if !with_scheme {
            return Err("only http and https addresses can be opened");
        }
        if port.parse::<u16>().is_err() {
            return Err("the address has a bad port");
        }
    }
    let labels: Vec<&str> = host.split('.').collect();
    let good = |label: &&str| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    if !labels.iter().all(good) {
        return Err("only http and https addresses can be opened");
    }
    if labels.len() < 2 && !(with_scheme && host.eq_ignore_ascii_case("localhost")) {
        return Err("the address needs a full host name");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_addresses_are_accepted() {
        for (text, want) in [
            ("https://github.com", "https://github.com"),
            ("github.com", "https://github.com"),
            (" HTTP://Example.com/a?b=c#d ", "http://Example.com/a?b=c#d"),
            ("example.com/a:b", "https://example.com/a:b"),
            ("https://example.com:8443/x", "https://example.com:8443/x"),
            ("http://localhost:3000", "http://localhost:3000"),
            ("https://127.0.0.1/", "https://127.0.0.1/"),
            ("https://xn--bcher-kva.example", "https://xn--bcher-kva.example"),
            (
                "https://en.wikipedia.org/wiki/Rust_(language)",
                "https://en.wikipedia.org/wiki/Rust_(language)",
            ),
            ("https://a.com/?u=x@y.com", "https://a.com/?u=x@y.com"),
        ] {
            assert_eq!(validate(text).as_deref(), Ok(want), "{text}");
        }
    }

    #[test]
    fn everything_else_is_refused() {
        for text in [
            "",
            "file:///C:/Windows/System32/cmd.exe",
            "FILE:C:\\x",
            "file://server/share/x.exe",
            "javascript:alert(1)",
            "JavaScript:alert(1)//a.com",
            "data:text/html,x",
            "vbscript:x",
            "ftp://example.com",
            "ms-settings:privacy",
            "ms-msdt:/id",
            "shell:startup",
            "search-ms:query=x",
            "mailto:a@b.com",
            r"\\server\share",
            "//server/share",
            r"C:\Windows\System32\calc.exe",
            "C:/Windows/System32/calc.exe",
            "calc",
            "notepad",
            "https://",
            "https:///etc",
            "https:github.com",
            "https://github.com@evil.com",
            "https://a.com\\@b.com",
            "https://a.com/ --incognito",
            "https://a.com/\" --x",
            "https://a.com/\nx",
            "https://a..com",
            "https://-a.com",
            "https://a.com:99999",
            "https://a.com:x",
            "https://[::1]/",
            "https://exam\u{200B}ple.com",
            "https://аpple.com",
            "localhost",
            "http://intranet",
        ] {
            assert!(validate(text).is_err(), "{text}");
        }
        assert!(validate(&format!("https://a.com/{}", "x".repeat(MAX_LEN))).is_err());
    }
}
