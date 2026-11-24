//! Connectivity probes on the runtime thread (spec §5.3).

use std::time::Instant;

use sohi_core::secret::KeyName;
use sohi_providers::{mistral, probe};

use crate::chat::{SharedClient, Update, read_key, shared_client};
use crate::runtime::UiLink;

/// Starts probe `id` beside whatever is running.
pub fn start(http: &SharedClient, id: u64, ui: &UiLink<Update>) {
    let Ok(http) = shared_client(http) else {
        ui.send(Update::Probed { id, reached: false });
        return;
    };
    let ui = ui.clone();
    tokio::spawn(async move {
        let started = Instant::now();
        let key = read_key(KeyName::Mistral).ok();
        let reached = probe::probe(&http, &base_url(), key.as_ref()).await;
        log::info!(
            "probe {id}: {} after {} ms",
            if reached { "reached" } else { "no answer" },
            started.elapsed().as_millis()
        );
        ui.send(Update::Probed { id, reached });
    });
}

/// The API; with the `dev-inject` feature, `SOHI_DEV_PROBE_FILE` names a
/// file that holds another base URL while it exists (to simulate an
/// outage).
fn base_url() -> String {
    #[cfg(feature = "dev-inject")]
    if let Some(path) = std::env::var_os("SOHI_DEV_PROBE_FILE")
        && let Ok(url) = std::fs::read_to_string(path)
    {
        return url.trim().to_owned();
    }
    mistral::BASE_URL.to_owned()
}
