use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Encoded-size bound for a capability manifest. A manifest is embedded in a
/// `response` envelope, so it must stay comfortably below the 64 KiB wire
/// limit (`message::MAX_MESSAGE_SIZE`); 32 KiB leaves room for envelope
/// overhead and IPC event wrapping.
pub const MAX_MANIFEST_BYTES: usize = 32 * 1024;

/// Upper bound on services listed per manifest.
pub const MAX_SERVICES: usize = 64;

const MAX_NAME_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 64;
const MAX_SERVICE_ID_BYTES: usize = 128;
const MAX_DESCRIPTION_BYTES: usize = 2048;
const MAX_ERROR_CODES: usize = 32;
const MAX_ERROR_CODE_BYTES: usize = 64;
const MAX_CONCURRENCY: u32 = 1024;

/// Self-described catalog of the services an application handler offers.
///
/// Field bounds are enforced at deserialization: an invalid manifest is an
/// `invalid_command` at `serve` time, never a half-validated claim in the
/// cache. Unknown fields are ignored for forward compatibility (payloads are
/// opaque; the manifest schema merely adds optional structure).
///
/// Fields are crate-private on purpose: every construction path — serde via
/// `try_from`, or [`Manifest::from_parts`] — runs validation, so a `Manifest`
/// value in daemon state always satisfies every invariant (schema bounds and
/// the encoded-size limit). Hand-constructed unvalidated values are not
/// possible, which is what the `fuzz_manifest` post-parse assertions rely on.
///
/// The schema is written for LLM consumption: `description` is imperative
/// prose, and `example_request`/`example_response` are worked objects —
/// callers generalize from one example faster than from a formal schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ManifestFields")]
pub struct Manifest {
    /// Optional human/agent-readable display name for the application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// Optional application version string (distinct from the daemon version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
    /// At least one service; at most [`MAX_SERVICES`].
    pub(crate) services: Vec<ServiceEntry>,
}

/// One offered service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceEntry {
    /// Stable machine-readable identifier (e.g. `cargo_test`).
    pub(crate) id: String,
    /// What the service does and how to call it.
    pub(crate) description: String,
    /// Worked request example (JSON object). Examples teach callers faster
    /// than formal schemas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) example_request: Option<serde_json::Value>,
    /// Worked response example (JSON object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) example_response: Option<serde_json::Value>,
    /// Suggested upper bound for a single exchange, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_hint_secs: Option<u64>,
    /// Concurrent exchanges the service can absorb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) concurrency: Option<u32>,
    /// Service-specific error payload codes a caller may receive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) errors: Option<Vec<String>>,
}

/// Raw deserialization shape; validation happens in [`Manifest::from_parts`].
#[derive(Debug, Deserialize)]
struct ManifestFields {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    services: Vec<ServiceEntry>,
}

impl TryFrom<ManifestFields> for Manifest {
    type Error = anyhow::Error;

    fn try_from(fields: ManifestFields) -> Result<Self> {
        Self::from_parts(fields.name, fields.version, fields.services)
    }
}

impl Manifest {
    /// Validate and construct. Also enforced on deserialization so an
    /// invalid manifest can never enter daemon state.
    pub fn from_parts(
        name: Option<String>,
        version: Option<String>,
        services: Vec<ServiceEntry>,
    ) -> Result<Self> {
        if let Some(name) = name.as_ref() {
            validate_text("name", name, 1, MAX_NAME_BYTES)?;
        }
        if let Some(version) = version.as_ref() {
            validate_text("version", version, 1, MAX_VERSION_BYTES)?;
        }
        if services.is_empty() {
            bail!("manifest must list at least one service");
        }
        if services.len() > MAX_SERVICES {
            bail!(
                "manifest lists {} services; maximum is {MAX_SERVICES}",
                services.len()
            );
        }
        for service in &services {
            validate_service(service)?;
        }
        let manifest = Self {
            name,
            version,
            services,
        };
        // The encoded-size bound is enforced here — not only at publication —
        // so a parsed manifest always satisfies every daemon invariant: a
        // `describe` response carrying it can never approach the 64 KiB wire
        // limit, on the serve path or the remote-response path.
        let size = manifest.encoded_size()?;
        if size > MAX_MANIFEST_BYTES {
            bail!("manifest encodes to {size} bytes; maximum is {MAX_MANIFEST_BYTES}");
        }
        Ok(manifest)
    }

    /// Read-only view of the validated service list. External consumers
    /// (fuzz targets, tooling) inspect through this accessor; construction
    /// remains confined to validated paths.
    pub fn services(&self) -> &[ServiceEntry] {
        &self.services
    }

    /// Number of validated services (1..=[`MAX_SERVICES`]).
    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    /// Encoded JSON size, used for the [`MAX_MANIFEST_BYTES`] bound.
    pub fn encoded_size(&self) -> Result<usize> {
        Ok(serde_json::to_vec(self)?.len())
    }

    /// Payload value for a `describe` response envelope.
    pub fn to_payload_value(&self) -> Result<serde_json::Value> {
        Ok(serde_json::to_value(self)?)
    }
}

fn validate_service(service: &ServiceEntry) -> Result<()> {
    validate_text("service id", &service.id, 1, MAX_SERVICE_ID_BYTES)?;
    if service
        .id
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        bail!("service id must not contain whitespace or control characters");
    }
    validate_text(
        "service description",
        &service.description,
        1,
        MAX_DESCRIPTION_BYTES,
    )?;
    for (field, example) in [
        ("example_request", &service.example_request),
        ("example_response", &service.example_response),
    ] {
        if let Some(value) = example
            && !value.is_object()
        {
            bail!("{field} must be a JSON object");
        }
    }
    if let Some(concurrency) = service.concurrency
        && (concurrency == 0 || concurrency > MAX_CONCURRENCY)
    {
        bail!("service concurrency must be between 1 and {MAX_CONCURRENCY}");
    }
    if let Some(errors) = service.errors.as_ref() {
        if errors.len() > MAX_ERROR_CODES {
            bail!(
                "service lists {} error codes; maximum is {MAX_ERROR_CODES}",
                errors.len()
            );
        }
        for code in errors {
            validate_text("error code", code, 1, MAX_ERROR_CODE_BYTES)?;
        }
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, min: usize, max: usize) -> Result<()> {
    let len = value.len();
    if len < min || len > max {
        bail!("{field} length {len} is outside the permitted range {min}..={max}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
