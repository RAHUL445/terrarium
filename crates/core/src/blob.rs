use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BlobId(String);

impl BlobId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn kid8(&self) -> &str {
        &self.0[3..11]
    }

    pub fn mac_first2(&self) -> &str {
        &self.0[12..14]
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for BlobId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        if value.len() != 76 || !value.starts_with("b1-") {
            return Err(Error::Schema(format!("invalid blob id `{value}`")));
        }
        let mut parts = value.split('-');
        let (Some("b1"), Some(kid), Some(mac), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Error::Schema(format!("invalid blob id `{value}`")));
        };
        if kid.len() != 8 || mac.len() != 64 {
            return Err(Error::Schema(format!("invalid blob id `{value}`")));
        }
        if !kid.chars().all(is_lower_hex) || !mac.chars().all(is_lower_hex) {
            return Err(Error::Schema(format!("invalid blob id `{value}`")));
        }
        Ok(BlobId(value.to_string()))
    }
}

impl TryFrom<String> for BlobId {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        value
            .parse()
            .map_err(|_| format!("invalid blob id `{value}`"))
    }
}

impl From<BlobId> for String {
    fn from(value: BlobId) -> Self {
        value.0
    }
}

fn is_lower_hex(c: char) -> bool {
    matches!(c, '0'..='9' | 'a'..='f')
}
