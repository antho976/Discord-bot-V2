//! The dashboard: a small server-rendered site. Pages are plain HTML built with maud, styled by
//! one stylesheet, with a few lines of script for toasts and two-step deletes.

pub mod auth;
pub mod banner;
pub mod form;
pub mod pages;
pub mod ui;

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use maud::{DOCTYPE, html};

use crate::Ctx;

pub use auth::{Level, Levels, Page, User};
pub use form::FormData;

pub type WebResult<T = Response> = Result<T, WebError>;

#[derive(Debug)]
pub enum WebError {
    /// Not signed in: send them to the login page.
    Login,
    Forbidden,
    NotFound,
    /// A mistake in what was submitted, shown to the person as is.
    Bad(String),
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for WebError {
    fn from(e: E) -> Self {
        WebError::Internal(e.into())
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let (status, title, text) = match self {
            WebError::Login => return Redirect::to("/login").into_response(),
            WebError::Forbidden => (StatusCode::FORBIDDEN, "No access", "Your account can't open this page. Ask the owner to give you access.".to_owned()),
            WebError::NotFound => (StatusCode::NOT_FOUND, "Not found", "This page doesn't exist.".to_owned()),
            WebError::Bad(msg) => (StatusCode::BAD_REQUEST, "That didn't go through", msg),
            WebError::Internal(e) => {
                tracing::error!(error = ?e, "dashboard error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Something broke", "The error is in the bot's log.".to_owned())
            }
        };
        let body = html! {
            (DOCTYPE)
            html lang="en" {
                head { (ui::head(title)) }
                body.bare {
                    main.notice {
                        h1 { (title) }
                        p { (text) }
                        a.btn href="/" { "Back to the dashboard" }
                    }
                }
            }
        };
        (status, body).into_response()
    }
}

/// Redirect back to a page after a save; the page shows a "Saved" toast.
pub fn saved(path: &str) -> Response {
    let sep = if path.contains('?') { '&' } else { '?' };
    Redirect::to(&format!("{path}{sep}saved=1")).into_response()
}

/// Build the whole site: core pages plus every feature's routes.
pub fn router(ctx: Ctx) -> Router {
    let mut app = Router::new()
        .route("/static/app.css", get(|| async { ([(header::CONTENT_TYPE, "text/css"), (header::CACHE_CONTROL, "public, max-age=3600")], include_str!("assets/app.css")) }))
        .route("/static/app.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript"), (header::CACHE_CONTROL, "public, max-age=3600")], include_str!("assets/app.js")) }))
        .route("/static/icon.svg", get(|| async { ([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "public, max-age=86400")], include_str!("assets/icon.svg")) }))
        .route("/healthz", get(|| async { "ok" }))
        .merge(auth::routes(ctx.cfg.preview))
        .merge(pages::routes())
        .merge(banner::routes());
    if ctx.cfg.preview {
        app = app.merge(crate::preview::routes());
    }
    for f in ctx.features() {
        app = app.merge(f.routes());
    }
    app.layer(middleware::from_fn_with_state(ctx.clone(), same_origin))
        .with_state(ctx)
}

/// Refuse form posts that come from another site. Machine endpoints (`/api/`, `/webhooks/`)
/// carry no cookies and authenticate themselves, so they're exempt.
async fn same_origin(axum::extract::State(ctx): axum::extract::State<Ctx>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let exempt = path.starts_with("/api/") || path.starts_with("/webhooks/");
    if !exempt && !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) && !origin_ok(&ctx, req.headers()) {
        return (StatusCode::FORBIDDEN, "Cross-site request refused").into_response();
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    h.insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    h.insert(header::REFERRER_POLICY, "same-origin".parse().unwrap());
    res
}

fn origin_ok(ctx: &Ctx, headers: &HeaderMap) -> bool {
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if origin == ctx.cfg.public_url {
            return true;
        }
        // Behind a proxy the browser's origin is the host it typed.
        let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or_default();
        return origin.split("://").nth(1) == Some(host);
    }
    !matches!(headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()), Some("cross-site"))
}

/// Serve the dashboard until shutdown.
pub async fn serve(ctx: Ctx, mut shutdown: tokio::sync::watch::Receiver<bool>) -> anyhow::Result<()> {
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], ctx.cfg.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "dashboard listening");
    axum::serve(listener, router(ctx))
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await?;
    Ok(())
}
