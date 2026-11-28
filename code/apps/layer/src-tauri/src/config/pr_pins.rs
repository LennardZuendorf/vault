//! Local checkout PR association and its persisted concurrency marker.

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use super::Checkout;
use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrPin {
    pub repo: String,
    pub number: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPrPin {
    #[serde(flatten)]
    pub pin: PrPin,
    /// Kept out of the MCP pin response. An old pin never had this proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<PrPinIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrPinIdentity {
    pub api_origin: String,
    pub id: NonZeroU64,
}

impl Checkout {
    pub(crate) fn replace_pr_pin(&mut self, pin: Option<StoredPrPin>) -> Result<bool> {
        if self.pr_pin == pin {
            return Ok(false);
        }
        self.pr_pin_revision = self
            .pr_pin_revision
            .checked_add(1)
            .ok_or_else(|| Error::Other("The checkout's pin revision cannot be advanced.".into()))?;
        self.pr_pin = pin;
        Ok(true)
    }
}
