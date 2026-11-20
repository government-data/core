//! The language of each request: the one the UI sends in `Accept-Language`
//! (the one the person chose, or the browser's). Everything that runs inside
//! answers its messages in that language (see [`texts::tr!`]).

use axum::extract::Request;
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;
use texts::Language;

pub async fn per_request(request: Request, next: Next) -> Response {
    let language = request
        .headers()
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .map(Language::from_accept_language)
        .unwrap_or_default();
    texts::within(language, next.run(request)).await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn answer(app: &Router, language: Option<&str>) -> String {
        let mut req = Request::builder().uri("/");
        if let Some(l) = language {
            req = req.header("accept-language", l);
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn answers_in_the_language_the_interface_asks_for() {
        let app = Router::new()
            .route("/", get(|| async { texts::tr!("app.platform.notFound") }))
            .layer(axum::middleware::from_fn(super::per_request));
        assert_eq!(answer(&app, None).await, "no encontrado");
        assert_eq!(answer(&app, Some("en-US,en;q=0.9")).await, "not found");
        assert_eq!(answer(&app, Some("es-CO")).await, "no encontrado");
    }
}
