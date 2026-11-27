// The explicit public type contract remains stable without broadening or replacing its API.
#![allow(clippy::trait_duplication_in_bounds)]

use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Metadata the bot-operation middleware can inspect without buffering a response body.
///
/// The summary is deliberately a fixed server string rather than handler prose, which can
/// contain caller-provided names or values that do not belong in the operation ledger.
#[derive(Debug, Clone, Copy)]
pub struct OperationResponseMeta {
    pub business_code: i32,
    pub error_summary: Option<&'static str>,
}

#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    /// The stable `error-mapping-v1.md` discriminant for an [`crate::error::ApiError::Typed`]
    /// rejection (its `error_code` wire field) -- carried on this one envelope shape instead of
    /// [`crate::error::ApiError`]'s `Typed` arm hand-building a parallel JSON body. Always `None`
    /// on a success response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<&'static str>,
    /// Structured, per-`error_code` extra fields (`error-mapping-v1.md`'s `details` column, e.g.
    /// `limit_exceeded`'s `{limit_kind,limit,observed?}` or `stale_frontier`'s
    /// `{current_seq,current_frontier}`). Always `None` on a success response and on any
    /// rejection whose stable code carries no extra structure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Json<Self> {
        Json(Self {
            code: 0,
            message: "success".into(),
            data: Some(data),
            error_code: None,
            details: None,
        })
    }
}

impl ApiResponse<()> {
    pub fn error(code: i32, msg: impl Into<String>) -> Json<Self> {
        Json(Self {
            code,
            message: msg.into(),
            data: None,
            error_code: None,
            details: None,
        })
    }

    pub fn ok() -> Json<Self> {
        Json(Self {
            code: 0,
            message: "success".into(),
            data: None,
            error_code: None,
            details: None,
        })
    }
}

#[derive(Serialize)]
pub struct PaginatedData<T: Serialize> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

impl<T: Serialize> PaginatedData<T> {
    pub const fn from_items(items: Vec<T>) -> Self {
        // Vec length cannot exceed isize::MAX, which fits in i64 on every supported target.
        #[allow(clippy::cast_possible_wrap)]
        let total = items.len() as i64;
        Self {
            items,
            total,
            page: 1,
            per_page: total,
            total_pages: 1,
        }
    }
}

/// The `page` and `per_page` query parameters of a paged list.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct PageQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// A page request with its defaults applied: `page` from 1, `per_page` between 1 and the list's
/// maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paging {
    pub page: i64,
    pub per_page: i64,
}

impl PageQuery {
    pub fn resolve(self, default_per_page: i64, max_per_page: i64) -> Paging {
        Paging {
            page: self.page.unwrap_or(1).max(1),
            per_page: self.per_page.unwrap_or(default_per_page).clamp(1, max_per_page),
        }
    }
}

impl Paging {
    /// Rows to skip before this page.
    pub const fn offset(self) -> i64 {
        (self.page - 1).saturating_mul(self.per_page)
    }
}

impl<T: Serialize> PaginatedData<T> {
    /// One page of a list of `total` entries.
    pub const fn page(items: Vec<T>, total: i64, paging: Paging) -> Self {
        Self {
            items,
            total,
            page: paging.page,
            per_page: paging.per_page,
            total_pages: if total == 0 {
                0
            } else {
                total / paging.per_page + (total % paging.per_page != 0) as i64
            },
        }
    }
}

#[cfg(test)]
mod paging_tests {
    use super::{PageQuery, PaginatedData, Paging};

    #[test]
    fn defaults_and_limits_apply() {
        let none = PageQuery::default().resolve(20, 100);
        assert_eq!(none, Paging { page: 1, per_page: 20 });
        assert_eq!(none.offset(), 0);
        let odd = PageQuery {
            page: Some(0),
            per_page: Some(5000),
        }
        .resolve(20, 100);
        assert_eq!(odd, Paging { page: 1, per_page: 100 });
        let third = PageQuery {
            page: Some(3),
            per_page: Some(30),
        }
        .resolve(20, 100);
        assert_eq!(third.offset(), 60);
    }

    #[test]
    fn total_pages_round_up() {
        let paging = Paging { page: 2, per_page: 20 };
        let data = PaginatedData::<u8>::page(Vec::new(), 41, paging);
        assert_eq!((data.total, data.page, data.per_page, data.total_pages), (41, 2, 20, 3));
        assert_eq!(PaginatedData::<u8>::page(Vec::new(), 0, paging).total_pages, 0);
    }
}
