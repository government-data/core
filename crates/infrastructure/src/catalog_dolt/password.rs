//! `DOLT_URL` with its password apart (decision 0080's amendment of
//! 2026-11-24, point 5): an installation's agent generates the password as
//! a file, not a URL, so the URL names the user and the password comes
//! from `DOLT_PASSWORD` (or its `_FILE`), written into the URL's user part.

/// `url` (`mysql://user@host:port?…`) with `password` after its user,
/// percent-encoded; refused if the URL has no user or already a password.
pub(crate) fn with_password(url: &str, password: &str) -> Result<String, String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or("DOLT_URL: no scheme (mysql://user@host:port)")?;
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (user, host) = authority
        .rsplit_once('@')
        .ok_or("DOLT_URL: no user before the host, with DOLT_PASSWORD set")?;
    if user.contains(':') || user.is_empty() {
        return Err("DOLT_URL: a password in the URL and in DOLT_PASSWORD".into());
    }
    Ok(format!(
        "{scheme}://{user}:{}@{host}{tail}",
        encode(password)
    ))
}

/// Every byte but unreserved characters as `%XX`.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_password_goes_after_the_user_and_the_options_stay() {
        let url = "mysql://qd_app@dolt:3306?ssl-mode=VERIFY_CA&ssl-ca=/run/qd-node/ca.crt";
        assert_eq!(
            with_password(url, "aB3-_x").unwrap(),
            "mysql://qd_app:aB3-_x@dolt:3306?ssl-mode=VERIFY_CA&ssl-ca=/run/qd-node/ca.crt"
        );
        assert_eq!(
            with_password("mysql://qd_app@dolt", "p").unwrap(),
            "mysql://qd_app:p@dolt"
        );
    }

    #[test]
    fn a_password_with_reserved_characters_is_encoded() {
        assert_eq!(
            with_password("mysql://u@h:1/", "a@b:c/d%").unwrap(),
            "mysql://u:a%40b%3Ac%2Fd%25@h:1/"
        );
    }

    #[test]
    fn a_url_without_a_user_or_with_a_password_is_refused() {
        assert!(with_password("mysql://dolt:3306", "p").is_err());
        assert!(with_password("mysql://u:old@dolt:3306", "p").is_err());
        assert!(with_password("dolt:3306", "p").is_err());
    }
}
