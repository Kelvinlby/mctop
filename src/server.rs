use std::fmt;

/// Default port a Minecraft Java Edition server listens on.
pub const DEFAULT_PORT: u16 = 25565;

/// A server address split into host and port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub host: String,
    pub port: u16,
}

impl Address {
    /// Parse `host`, `host:port`, or `[v6]:port`. Missing ports fall back to
    /// [`DEFAULT_PORT`].
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        Self::parse_with_default_port(input, DEFAULT_PORT)
    }

    /// As [`Address::parse`], but with a caller-chosen fallback port. RCON
    /// listens on a different port from the game itself.
    pub fn parse_with_default_port(input: &str, default_port: u16) -> anyhow::Result<Self> {
        let input = input.trim();
        anyhow::ensure!(!input.is_empty(), "server address is empty");

        let (host, port) = match input.rsplit_once(':') {
            // A bare IPv6 literal has several colons and no port.
            Some(_) if input.starts_with('[') && input.ends_with(']') => (input, None),
            Some((host, port)) => (host, Some(port)),
            None => (input, None),
        };

        let host = host.trim_start_matches('[').trim_end_matches(']');
        anyhow::ensure!(!host.is_empty(), "server address has no host");

        let port = match port {
            Some(port) => port
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid port: {port}"))?,
            None => default_port,
        };

        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_the_port() {
        let address = Address::parse("play.example.net").unwrap();
        assert_eq!(address.host, "play.example.net");
        assert_eq!(address.port, DEFAULT_PORT);
    }

    #[test]
    fn reads_an_explicit_port() {
        assert_eq!(Address::parse("10.0.0.5:25566").unwrap().port, 25566);
    }

    #[test]
    fn reads_ipv6() {
        let bare = Address::parse("[::1]").unwrap();
        assert_eq!(bare.host, "::1");
        assert_eq!(bare.port, DEFAULT_PORT);

        let with_port = Address::parse("[::1]:25566").unwrap();
        assert_eq!(with_port.host, "::1");
        assert_eq!(with_port.port, 25566);
    }

    #[test]
    fn honours_a_caller_chosen_default_port() {
        let address = Address::parse_with_default_port("127.0.0.1", 25575).unwrap();
        assert_eq!(address.port, 25575);

        // An explicit port still wins.
        let address = Address::parse_with_default_port("127.0.0.1:25580", 25575).unwrap();
        assert_eq!(address.port, 25580);
    }

    #[test]
    fn rejects_junk() {
        assert!(Address::parse("").is_err());
        assert!(Address::parse("host:port").is_err());
    }
}
