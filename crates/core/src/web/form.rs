//! Form bodies as a flat list of pairs, so repeated names (multi-selects, checkbox lists) work.

use axum::extract::{FromRequest, Request};
use axum::body::Bytes;

use super::WebError;

#[derive(Debug, Default, Clone)]
pub struct FormData(pub Vec<(String, String)>);

impl<S: Send + Sync> FromRequest<S> for FormData {
    type Rejection = WebError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|_| WebError::Bad("The form was too large or unreadable.".into()))?;
        Ok(Self::parse(&bytes))
    }
}

impl FormData {
    pub fn parse(body: &[u8]) -> Self {
        Self(form_urlencoded::parse(body).into_owned().collect())
    }

    /// The first value for `name`, trimmed. Empty counts as missing.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == name).map(|(_, v)| v.trim()).filter(|v| !v.is_empty())
    }

    pub fn string(&self, name: &str) -> String {
        self.get(name).unwrap_or_default().to_owned()
    }

    /// Like `string` but keeps inner whitespace and newlines, only trimming the ends.
    pub fn text(&self, name: &str) -> String {
        self.0.iter().find(|(k, _)| k == name).map(|(_, v)| v.trim().replace("\r\n", "\n")).unwrap_or_default()
    }

    pub fn all(&self, name: &str) -> Vec<&str> {
        self.0.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty()).collect()
    }

    /// A checkbox or switch: present means on.
    pub fn bool(&self, name: &str) -> bool {
        self.0.iter().any(|(k, v)| k == name && v != "off" && v != "0")
    }

    pub fn i64(&self, name: &str) -> Option<i64> {
        self.get(name)?.replace(',', "").parse().ok()
    }

    /// A number clamped into `min..=max`, or `default` when missing or not a number.
    pub fn int_in(&self, name: &str, min: i64, max: i64, default: i64) -> i64 {
        self.i64(name).map(|n| n.clamp(min, max)).unwrap_or(default)
    }

    pub fn f64(&self, name: &str) -> Option<f64> {
        self.get(name)?.parse().ok()
    }

    /// A Discord id from a channel/role/user select. Empty means none.
    pub fn id(&self, name: &str) -> Option<u64> {
        self.get(name)?.parse().ok()
    }

    pub fn ids(&self, name: &str) -> Vec<u64> {
        self.all(name).into_iter().filter_map(|v| v.parse().ok()).collect()
    }
}
