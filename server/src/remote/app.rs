//! Admits phones that come by the app.
use super::{
    web::{serve, Phones},
    wire::Wire,
};

/// Returns once the host is closed.
pub(super) async fn listen(host: &remote_link::Host, phones: &Phones) {
    while let Some(knock) = host.accept().await {
        let phones = phones.clone();
        tokio::spawn(async move {
            match knock.answer().await {
                Ok(link) => serve(Wire::App(link), phones).await,
                Err(error) => tracing::debug!(%error, "phone did not connect"),
            }
        });
    }
}
