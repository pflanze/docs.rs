use crate::{
    db::PoolError,
    storage::PathNotFoundError,
    web::{cache::CachePolicy, encode_url_path, releases::Search},
};
use anyhow::anyhow;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response as AxumResponse},
    Json,
};
use std::borrow::Cow;

use super::AxumErrorPage;

#[derive(Debug, thiserror::Error)]
pub enum AxumNope {
    #[error("Requested resource not found")]
    ResourceNotFound,
    #[error("Requested build not found")]
    BuildNotFound,
    #[error("Requested crate not found")]
    CrateNotFound,
    #[error("Requested owner not found")]
    OwnerNotFound,
    #[error("Requested crate does not have specified version")]
    VersionNotFound,
    #[error("Search yielded no results")]
    NoResults,
    #[error("internal error")]
    InternalError(anyhow::Error),
    #[error("bad request")]
    BadRequest(anyhow::Error),
    #[error("redirect")]
    Redirect(String, CachePolicy),
}

// FUTURE: Ideally, the split between the 3 kinds of responses would
// be done by having 3 kinds of enums in the first place instead of
// just `AxumNope`, to keep the line statically type-checked
// throughout instead of having the conversion?

impl AxumNope {
    fn into_error_response(self) -> ErrorResponse {
        match self {
            AxumNope::ResourceNotFound => {
                // user tried to navigate to a resource (doc page/file) that doesn't exist
                ErrorResponse::ErrorInfo(ErrorInfo {
                    title: "The requested resource does not exist",
                    message: "no such resource".into(),
                    status: StatusCode::NOT_FOUND,
                })
            }
            AxumNope::BuildNotFound => ErrorResponse::ErrorInfo(ErrorInfo {
                title: "The requested build does not exist",
                message: "no such build".into(),
                status: StatusCode::NOT_FOUND,
            }),
            AxumNope::CrateNotFound => {
                // user tried to navigate to a crate that doesn't exist
                // TODO: Display the attempted crate and a link to a search for said crate
                ErrorResponse::ErrorInfo(ErrorInfo {
                    title: "The requested crate does not exist",
                    message: "no such crate".into(),
                    status: StatusCode::NOT_FOUND,
                })
            }
            AxumNope::OwnerNotFound => ErrorResponse::ErrorInfo(ErrorInfo {
                title: "The requested owner does not exist",
                message: "no such owner".into(),
                status: StatusCode::NOT_FOUND,
            }),
            AxumNope::VersionNotFound => {
                // user tried to navigate to a crate with a version that does not exist
                // TODO: Display the attempted crate and version
                ErrorResponse::ErrorInfo(ErrorInfo {
                    title: "The requested version does not exist",
                    message: "no such version for this crate".into(),
                    status: StatusCode::NOT_FOUND,
                })
            }
            AxumNope::NoResults => {
                // user did a search with no search terms
                ErrorResponse::Search(Search {
                    title: "No results given for empty search query".to_owned(),
                    status: StatusCode::NOT_FOUND,
                    ..Default::default()
                })
            }
            AxumNope::BadRequest(source) => ErrorResponse::ErrorInfo(ErrorInfo {
                title: "Bad request",
                message: Cow::Owned(source.to_string()),
                status: StatusCode::BAD_REQUEST,
            }),
            AxumNope::InternalError(source) => {
                crate::utils::report_error(&source);
                ErrorResponse::ErrorInfo(ErrorInfo {
                    title: "Internal Server Error",
                    message: Cow::Owned(source.to_string()),
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                })
            }
            AxumNope::Redirect(target, cache_policy) => {
                match super::axum_cached_redirect(&encode_url_path(&target), cache_policy) {
                    Ok(response) => ErrorResponse::Redirect(response),
                    // Recurse 1 step:
                    Err(err) => AxumNope::InternalError(err).into_error_response(),
                }
            }
        }
    }
}

// A response representing an outcome from `AxumNope`, usable in both
// HTML or JSON (API) based endpoints.
enum ErrorResponse {
    // Info representable both as HTML or as JSON
    ErrorInfo(ErrorInfo),
    // Redirect,
    Redirect(AxumResponse),
    // To recreate empty search page; only valid in HTML based
    // endpoints.
    Search(Search),
}

struct ErrorInfo {
    // For the title of the page
    pub title: &'static str,
    // The error message, displayed as a description
    pub message: Cow<'static, str>,
    pub status: StatusCode,
}

impl ErrorResponse {
    fn into_html_response(self) -> AxumResponse {
        match self {
            ErrorResponse::ErrorInfo(ErrorInfo {
                title,
                message,
                status,
            }) => AxumErrorPage {
                title,
                message,
                status,
            }
            .into_response(),
            ErrorResponse::Redirect(response) => response,
            ErrorResponse::Search(search) => search.into_response(),
        }
    }

    fn into_json_response(self) -> AxumResponse {
        match self {
            ErrorResponse::ErrorInfo(ErrorInfo {
                title,
                message,
                status,
            }) => (
                status,
                Json(serde_json::json!({
                    "result": "err", // XXX
                    "title": title,
                    "message": message,
                })),
            )
                .into_response(),
            ErrorResponse::Redirect(response) => response,
            ErrorResponse::Search(search) => panic!(
                "expecting that handlers that return JSON error responses \
                 don't return Search, but got: {search:?}"
            ),
        }
    }
}

impl IntoResponse for AxumNope {
    fn into_response(self) -> AxumResponse {
        self.into_error_response().into_html_response()
    }
}

/// `AxumNope` but generating error responses in JSON (for API).
pub(crate) struct JsonAxumNope(pub AxumNope);

impl IntoResponse for JsonAxumNope {
    fn into_response(self) -> AxumResponse {
        self.0.into_error_response().into_json_response()
    }
}

impl From<anyhow::Error> for AxumNope {
    fn from(err: anyhow::Error) -> Self {
        match err.downcast::<AxumNope>() {
            Ok(axum_nope) => axum_nope,
            Err(err) => match err.downcast::<PathNotFoundError>() {
                Ok(_) => AxumNope::ResourceNotFound,
                Err(err) => AxumNope::InternalError(err),
            },
        }
    }
}

impl From<sqlx::Error> for AxumNope {
    fn from(err: sqlx::Error) -> Self {
        AxumNope::InternalError(anyhow!(err))
    }
}

impl From<PoolError> for AxumNope {
    fn from(err: PoolError) -> Self {
        AxumNope::InternalError(anyhow!(err))
    }
}

pub(crate) type AxumResult<T> = Result<T, AxumNope>;
pub(crate) type JsonAxumResult<T> = Result<T, JsonAxumNope>;

#[cfg(test)]
mod tests {
    use super::{AxumNope, IntoResponse};
    use crate::{test::wrapper, web::cache::CachePolicy};
    use kuchikiki::traits::TendrilSink;

    #[test]
    fn test_redirect_error_encodes_url_path() {
        let response =
            AxumNope::Redirect("/something>".into(), CachePolicy::ForeverInCdnAndBrowser)
                .into_response();

        assert_eq!(response.status(), 302);
        assert_eq!(response.headers().get("Location").unwrap(), "/something%3E");
    }

    #[test]
    fn check_404_page_content_crate() {
        wrapper(|env| {
            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/crate-which-doesnt-exist")
                    .send()?
                    .text()?,
            );
            assert_eq!(page.select("#crate-title").unwrap().count(), 1);
            assert_eq!(
                page.select("#crate-title")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "The requested crate does not exist",
            );

            Ok(())
        });
    }

    #[test]
    fn check_404_page_content_resource() {
        wrapper(|env| {
            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/resource-which-doesnt-exist.js")
                    .send()?
                    .text()?,
            );
            assert_eq!(page.select("#crate-title").unwrap().count(), 1);
            assert_eq!(
                page.select("#crate-title")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "The requested resource does not exist",
            );

            Ok(())
        });
    }

    #[test]
    fn check_400_page_content_not_semver_version() {
        wrapper(|env| {
            env.fake_release().name("dummy").create()?;

            let response = env.frontend().get("/dummy/not-semver").send()?;
            assert_eq!(response.status(), 400);

            let page = kuchikiki::parse_html().one(response.text()?);
            assert_eq!(page.select("#crate-title").unwrap().count(), 1);
            assert_eq!(
                page.select("#crate-title")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "Bad request"
            );

            Ok(())
        });
    }

    #[test]
    fn check_404_page_content_nonexistent_version() {
        wrapper(|env| {
            env.fake_release().name("dummy").version("1.0.0").create()?;
            let page =
                kuchikiki::parse_html().one(env.frontend().get("/dummy/2.0").send()?.text()?);
            assert_eq!(page.select("#crate-title").unwrap().count(), 1);
            assert_eq!(
                page.select("#crate-title")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "The requested version does not exist",
            );

            Ok(())
        });
    }

    #[test]
    fn check_404_page_content_any_version_all_yanked() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("1.0.0")
                .yanked(true)
                .create()?;
            let page = kuchikiki::parse_html().one(env.frontend().get("/dummy/*").send()?.text()?);
            assert_eq!(page.select("#crate-title").unwrap().count(), 1);
            assert_eq!(
                page.select("#crate-title")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "The requested version does not exist",
            );

            Ok(())
        });
    }
}
