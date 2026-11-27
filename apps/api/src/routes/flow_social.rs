//! The client address extractor of the anonymous public-share reads.
//!
//! The anonymous share read hashes the token, hides revoked and expired links, and rate-limits per
//! client address. It is registered without auth middleware on purpose.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;

use crate::security::client_ip;

/// Client address for the public-share limiter.
///
/// `axum::serve(...into_make_service_with_connect_info)` inserts [`ConnectInfo`]. Behind a
/// trusted reverse proxy the address comes from its forwarding headers, see
/// [`crate::security::client_ip`]. A caller that reaches the handler without `ConnectInfo`
/// (direct tests) shares the unspecified address.
#[derive(Clone, Copy)]
pub struct ShareClientIp(pub IpAddr);

impl<S> FromRequestParts<S> for ShareClientIp
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
        Ok(Self(client_ip(
            &parts.headers,
            peer,
            &crate::config::runtime().trusted_proxies,
        )))
    }
}
