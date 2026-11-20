//! The edition the application is delivered in.

use serde::Serialize;

/// How the application is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Edition {
    /// On the customer's network.
    #[default]
    OnPremise,
    /// In the cloud, several companies per cell.
    Online,
}

impl Edition {
    /// `onpremise` or `online` (`ORQ_EDITION`).
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "onpremise" | "on-premise" => Ok(Self::OnPremise),
            "online" => Ok(Self::Online),
            other => Err(texts::tr!(
                "app.platform.editionExpected",
                other = format!("{:?}", other)
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editions_parse() {
        assert_eq!(Edition::parse("onpremise").unwrap(), Edition::OnPremise);
        assert_eq!(Edition::parse("ONLINE").unwrap(), Edition::Online);
        assert_eq!(Edition::parse("").unwrap(), Edition::OnPremise);
        assert!(Edition::parse("cloud").is_err());
    }
}
