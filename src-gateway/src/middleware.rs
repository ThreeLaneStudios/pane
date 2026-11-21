use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};
use tracing::Instrument;
use uuid::Uuid;

const REQUEST_ID_HEADER: &str = "x-request-id";

pub async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let span = tracing::info_span!(
        "request",
        request_id = %request_id,
        method = %request.method(),
        uri = %request.uri()
    );

    request.extensions_mut().insert(request_id.clone());
    let response = next.run(request).instrument(span).await;
    let (mut parts, body) = response.into_parts();
    parts.headers.insert(
        REQUEST_ID_HEADER,
        HeaderValue::from_str(&request_id).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );

    Response::from_parts(parts, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::{Request, StatusCode}, middleware, response::IntoResponse, routing::get, Router};
    use tower::ServiceExt;

    async fn test_handler() -> impl IntoResponse {
        (StatusCode::OK, "OK")
    }

    #[tokio::test]
    async fn test_request_id_propagated() {
        let app = Router::new().route("/test", get(test_handler)).layer(middleware::from_fn(request_id_middleware));
        let request_id = "test-request-id-123";
        let request = Request::builder().uri("/test").header("x-request-id", request_id).body(Body::empty()).unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get("x-request-id").unwrap(), request_id);
    }

    #[tokio::test]
    async fn test_request_id_generated_when_missing() {
        let app = Router::new().route("/test", get(test_handler)).layer(middleware::from_fn(request_id_middleware));
        let request = Request::builder().uri("/test").body(Body::empty()).unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let request_id = response.headers().get("x-request-id").unwrap().to_str().unwrap();
        assert!(Uuid::parse_str(request_id).is_ok());
    }
}
