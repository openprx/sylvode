//! Uploaded object downloads and signed download URLs.

use api::{middleware, routes};
use axum::{
    Router, middleware as axum_middleware,
    routing::{get, post},
};
use platform::app::AppState;

pub(crate) fn routes(auth_state: &AppState) -> Router<AppState> {
    // Uploaded object downloads are gated by `uploads_access_middleware`: it accepts either a
    // signed download URL or a normal session, and the handlers then scope the object to the
    // workspace that owns it.
    let uploads_access_layer =
        || axum_middleware::from_fn_with_state(auth_state.clone(), routes::upload::uploads_access_middleware);

    Router::new()
        .route(
            "/uploads/{file_name}",
            get(routes::upload::get_uploaded_file).route_layer(uploads_access_layer()),
        )
        .route(
            "/uploads/thumbnails/{file_name}",
            get(routes::upload::get_uploaded_thumbnail).route_layer(uploads_access_layer()),
        )
        .route(
            "/uploads/previews/{file_name}",
            get(routes::upload::get_uploaded_preview).route_layer(uploads_access_layer()),
        )
        .route(
            "/uploads/variants/{file_name}",
            get(routes::upload::get_uploaded_variant).route_layer(uploads_access_layer()),
        )
        .route(
            "/uploads/signatures/{file_name}",
            get(routes::upload::get_uploaded_signature).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/{file_name}",
            get(routes::upload::get_uploaded_file).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/thumbnails/{file_name}",
            get(routes::upload::get_uploaded_thumbnail).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/previews/{file_name}",
            get(routes::upload::get_uploaded_preview).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/variants/{file_name}",
            get(routes::upload::get_uploaded_variant).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/signatures/{file_name}",
            get(routes::upload::get_uploaded_signature).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/signatures/{file_name}/download",
            get(routes::upload::download_signed_uploaded_signature),
        )
        .route(
            "/api/v1/uploads/signatures/{file_name}/signed-url",
            post(routes::upload::create_uploaded_signature_signed_url).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/form-attachments/{attachment_id}/download",
            get(routes::form::download_signed_form_attachment),
        )
}
