//! Bounded non-streaming and error HTTP response body reading.

use reqwest::Response;

pub(crate) enum BodyReadError {
    Transport(String),
    TooLarge(String),
}

pub(crate) async fn read_bounded_body(
    mut resp: Response,
    limit: usize,
) -> Result<Vec<u8>, BodyReadError> {
    if let Some(len) = resp.content_length()
        && len > limit as u64
    {
        return Err(BodyReadError::TooLarge(
            "response content-length exceeds limit".into(),
        ));
    }
    let cap = resp
        .content_length()
        .and_then(|n| usize::try_from(n).ok())
        .map_or(0, |n| n.min(limit).min(64 * 1024));
    let mut buf = Vec::with_capacity(cap);
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| BodyReadError::Transport(e.to_string()))?
    {
        let Some(next) = buf.len().checked_add(chunk.len()) else {
            return Err(BodyReadError::TooLarge(
                "response payload length overflow".into(),
            ));
        };
        if next > limit {
            return Err(BodyReadError::TooLarge(format!(
                "response payload exceeds {limit} byte limit"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}
