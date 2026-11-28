//! Public plugin index `sv.plugin-index.v1` (contract M15): the document types, validation with JSON
//! paths, limits, image header checks, URL rules and the append-only comparison against a base.
//!
//! Shared by the Sylvode API (lenient reads of a fetched index) and the index repository's CI
//! (strict checks). Pure: no network, no file or git access.
//!
//! - [`parse_index`], [`parse_publisher`], [`parse_plugin`]: bytes to a validated document; strict
//!   mode refuses unknown members, lenient mode ignores them. Findings carry JSON paths such as
//!   `$.plugins[3].versions[0].sha256`.
//! - [`validate_index`], [`validate_plugin`], [`validate_publisher`]: rules on typed documents.
//! - [`compare`]: append-only and one-way-status rules between a base and a head index.
//! - [`check_image`], [`sniff`]: image size, magic bytes and dimension rules.
//! - [`download_url_problem`] with a [`HostPolicy`]: the download URL rules.
//! - [`lowest_admitted`]: the lowest Sylvode version a `sylvode` range admits (compare with `sv.min`).

#[cfg(feature = "cli")]
pub mod cli;
pub mod compare;
pub mod error;
pub mod image;
pub mod limits;
pub mod model;
pub mod parse;
pub mod rules;
pub mod validate;

pub use compare::{Changes, Comparison, compare};
pub use error::{Issue, Issues, ValidationError};
pub use image::{ImageError, ImageInfo, ImageRole, check_image, sniff};
pub use model::*;
pub use parse::{Mode, Options, parse_index, parse_plugin, parse_publisher};
pub use rules::{
    AnyHost, ExactHosts, HostPolicy, ImageFormat, download_url_problem, lowest_admitted, parse_compat,
    parse_exact_version, web_url_problem,
};
pub use validate::{validate_index, validate_plugin, validate_publisher};

#[cfg(test)]
mod tests;
