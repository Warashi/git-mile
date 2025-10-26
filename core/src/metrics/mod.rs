use std::sync::OnceLock;

use metrics_exporter_prometheus::{BuildError, PrometheusBuilder, PrometheusHandle};

static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the global Prometheus recorder used for exporting metrics.
///
/// # Errors
///
/// Returns an error when the Prometheus exporter cannot be initialized.
pub fn init_prometheus() -> Result<(), BuildError> {
    if PROMETHEUS_HANDLE.get().is_some() {
        return Ok(());
    }

    let handle = PrometheusBuilder::new().install_recorder()?;
    let _ = PROMETHEUS_HANDLE.set(handle);
    Ok(())
}

pub fn render_prometheus() -> Option<String> {
    PROMETHEUS_HANDLE.get().map(|handle| handle.render())
}
