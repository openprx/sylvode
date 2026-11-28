//! Who may send a request body above 2 MiB to the HTTP transports.
//!
//! A large body holds one of a few process-wide permits for up to two minutes, so the permit must
//! not be free for anyone who can type `Bearer x`. A large-body request is admitted only when
//!
//! * its client address holds no other large-body request (one per address; a client whose address
//!   cannot be told, behind a trusted proxy that sent no `X-Real-IP`, shares one slot), and
//! * its credential is verified: a delegated token was verified by the inbound layer already, and
//!   any other token is checked against the API (`GET /api/v1/auth/me`) through the client's own
//!   token-verdict cache (the workspace cache, keyed by token hash), within [`VERIFY_TIMEOUT`].
//!
//! The check runs before a byte of the body is read. A credential the API refuses counts against the
//! client address in the same [`RefusalLimiter`] the listen hubs use, so made-up tokens cannot make
//! the API do unbounded work either.

use crate::client::OpenPrClient;
use crate::client::{ApiErrorClass, classify_api_error, is_backend_failure};
use crate::listen::{Client, RefusalLimiter};
use crate::oauth::Caller;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

/// Longest the API is given to say whether a token is valid.
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);
/// `Retry-After` of a refusal for a busy address.
pub const RETRY_AFTER_SECS: u64 = 30;

/// Why a large body was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The address already has a large body in flight, or has had too many credentials refused.
    Busy,
    /// The API does not vouch for the credential.
    Unauthorized,
    /// The API could not be asked in time.
    Unavailable,
}

/// The per-address bookkeeping of large-body requests.
#[derive(Default)]
pub struct LargeBodies {
    holders: Mutex<HashSet<Client>>,
    limiter: RefusalLimiter,
}

/// A client address's place among the large-body requests; released when dropped.
pub struct AddressSlot {
    owner: Arc<LargeBodies>,
    client: Client,
}

impl Drop for AddressSlot {
    fn drop(&mut self) {
        self.owner.holders.lock().remove(&self.client);
    }
}

impl LargeBodies {
    /// Takes the one place `client` may hold, or `None` when it holds it already.
    pub fn reserve(self: &Arc<Self>, client: Client) -> Option<AddressSlot> {
        let client = client.grouped();
        self.holders.lock().insert(client).then(|| AddressSlot {
            owner: Arc::clone(self),
            client,
        })
    }

    /// Whether `caller` may be admitted: a delegated one already was, any other is asked of the API.
    /// The caller holds the client's [`AddressSlot`], so one client has one check at a time.
    pub async fn verify(&self, client: Client, caller: &Caller, api: &OpenPrClient) -> Result<(), Refusal> {
        let token = match caller {
            Caller::Delegated(_) => return Ok(()),
            Caller::Legacy(token) => token.clone(),
        };
        let attempt = self.limiter.admit(client).ok_or(Refusal::Busy)?;
        let asked = tokio::time::timeout(VERIFY_TIMEOUT, api.acting_as(token).workspace()).await;
        match asked {
            Ok(Ok(_)) => {
                attempt.give_back();
                Ok(())
            }
            Ok(Err(error)) if is_backend_failure(&error) && classify_api_error(&error) == ApiErrorClass::Other => {
                attempt.give_back();
                Err(Refusal::Unavailable)
            }
            // The credential is refused: the attempt stays counted against the address.
            Ok(Err(_)) => Err(Refusal::Unauthorized),
            Err(_) => {
                attempt.give_back();
                Err(Refusal::Unavailable)
            }
        }
    }
}
