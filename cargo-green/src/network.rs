use std::{fmt, str::FromStr};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Network {
    #[default]
    None,
    Default,
    Host,
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Default => write!(f, "default"),
            Self::Host => write!(f, "host"),
        }
    }
}

impl FromStr for Network {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "default" => Ok(Self::Default),
            "host" => Ok(Self::Host),
            _ => {
                let all: Vec<_> = [Self::None, Self::Default, Self::Host]
                    .into_iter()
                    .map(|x| x.to_string())
                    .collect();
                bail!("Network must be one of {all:?}")
            }
        }
    }
}
