//! Strict public-ID navigation links shared by the native iPhone shell and
//! future mobile surfaces. Links are navigation hints only: they contain no
//! session, pairing, library key, filesystem path, or SQLite row identifier.

use crate::portable::is_uuid_v7;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const MOBILE_DEEP_LINK_SCHEME: &str = "noted";
pub const MAX_MOBILE_DEEP_LINK_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "destination",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum MobileDeepLink {
    Note {
        library_id: String,
        record_id: String,
    },
}

impl MobileDeepLink {
    pub fn note(library_id: String, record_id: String) -> Result<Self, DeepLinkError> {
        if !is_uuid_v7(&library_id) {
            return Err(DeepLinkError::InvalidLibraryId);
        }
        if !is_uuid_v7(&record_id) {
            return Err(DeepLinkError::InvalidRecordId);
        }
        Ok(Self::Note {
            library_id,
            record_id,
        })
    }

    pub fn to_uri(&self) -> String {
        match self {
            Self::Note {
                library_id,
                record_id,
            } => format!("{MOBILE_DEEP_LINK_SCHEME}://library/{library_id}/notes/{record_id}"),
        }
    }

    pub fn parse(raw: &str) -> Result<Self, DeepLinkError> {
        if raw.is_empty() || raw.len() > MAX_MOBILE_DEEP_LINK_BYTES {
            return Err(DeepLinkError::InvalidLength);
        }
        if raw
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
        {
            return Err(DeepLinkError::InvalidEncoding);
        }
        if raw.contains(['?', '#', '@', '%']) {
            return Err(DeepLinkError::UnexpectedComponent);
        }

        let prefix = format!("{MOBILE_DEEP_LINK_SCHEME}://library/");
        let path = raw
            .strip_prefix(&prefix)
            .ok_or(DeepLinkError::UnsupportedSchemeOrHost)?;
        let mut segments = path.split('/');
        let library_id = segments.next().ok_or(DeepLinkError::MalformedPath)?;
        let collection = segments.next().ok_or(DeepLinkError::MalformedPath)?;
        let record_id = segments.next().ok_or(DeepLinkError::MalformedPath)?;
        if segments.next().is_some() || collection != "notes" {
            return Err(DeepLinkError::MalformedPath);
        }
        Self::note(library_id.to_string(), record_id.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepLinkError {
    InvalidLength,
    InvalidEncoding,
    UnexpectedComponent,
    UnsupportedSchemeOrHost,
    MalformedPath,
    InvalidLibraryId,
    InvalidRecordId,
}

impl fmt::Display for DeepLinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidLength => "mobile link length is invalid",
            Self::InvalidEncoding => "mobile link contains unsafe characters",
            Self::UnexpectedComponent => "mobile link contains an unsupported component",
            Self::UnsupportedSchemeOrHost => "mobile link scheme or host is unsupported",
            Self::MalformedPath => "mobile link path is malformed",
            Self::InvalidLibraryId => "mobile link library ID is invalid",
            Self::InvalidRecordId => "mobile link record ID is invalid",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for DeepLinkError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::deterministic_backfill_uuid_v7;

    fn id(seed: &str) -> String {
        deterministic_backfill_uuid_v7(1_786_953_600_000, "mobile-deep-link", seed)
    }

    #[test]
    fn note_links_round_trip_only_public_ids() {
        let link = MobileDeepLink::note(id("library"), id("note")).unwrap();
        let uri = link.to_uri();
        assert_eq!(MobileDeepLink::parse(&uri), Ok(link));
        assert!(!uri.contains(['?', '#', '@']));
        assert!(!uri.contains("token"));
    }

    #[test]
    fn rejects_credentials_queries_fragments_paths_and_non_v7_records() {
        let library = id("library");
        let note = id("note");
        for invalid in [
            format!("https://library/{library}/notes/{note}"),
            format!("noted://device@library/{library}/notes/{note}"),
            format!("noted://library/{library}/notes/{note}?token=secret"),
            format!("noted://library/{library}/notes/{note}#fragment"),
            format!("noted://library/{library}/notes/{note}/extra"),
            format!("noted://library/{library}/meetings/{note}"),
            format!("noted://library/{library}/notes/%2e%2e"),
        ] {
            assert!(
                MobileDeepLink::parse(&invalid).is_err(),
                "accepted {invalid}"
            );
        }
        assert_eq!(
            MobileDeepLink::parse(&format!(
                "noted://library/{library}/notes/550e8400-e29b-41d4-a716-446655440000"
            )),
            Err(DeepLinkError::InvalidRecordId)
        );
    }

    #[test]
    fn rejects_empty_whitespace_and_oversized_links() {
        assert_eq!(MobileDeepLink::parse(""), Err(DeepLinkError::InvalidLength));
        assert_eq!(
            MobileDeepLink::parse("noted://library/ bad"),
            Err(DeepLinkError::InvalidEncoding)
        );
        assert_eq!(
            MobileDeepLink::parse(&"n".repeat(MAX_MOBILE_DEEP_LINK_BYTES + 1)),
            Err(DeepLinkError::InvalidLength)
        );
    }
}
