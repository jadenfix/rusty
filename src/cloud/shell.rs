//! POSIX shell quoting, byte for byte the same as Python's `shlex.quote`.

/// One shell word that the shell reads back as exactly `s`, never as code.
pub fn quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.bytes().all(|b| b.is_ascii_alphanumeric() || b"@%+=:,./-_".contains(&b)) {
        return s.into();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// Quotes and joins words into one command line.
pub fn join<S: AsRef<str>>(words: &[S]) -> String {
    words.iter().map(|w| quote(w.as_ref())).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_shlex_quote() {
        assert_eq!(quote(""), "''");
        assert_eq!(quote("plain-word_1.2/x@y%z+=:,"), "plain-word_1.2/x@y%z+=:,");
        assert_eq!(quote("two words"), "'two words'");
        assert_eq!(quote("it's"), "'it'\"'\"'s'");
        assert_eq!(quote("$HOME"), "'$HOME'");
        assert_eq!(quote("é"), "'é'");
        assert_eq!(join(&["rusty", "--goal", "fix it"]), "rusty --goal 'fix it'");
    }

    #[test]
    fn quoted_data_round_trips_through_bash() {
        let nasty = "a'b\"c $(touch /tmp/never) `x` \\ ; | & \n{\"k\":\"v\"}";
        let out =
            std::process::Command::new("bash").arg("-c").arg(format!("printf '%s' {}", quote(nasty))).output().unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), nasty);
    }
}
