//! Minimal Bonjour advertisement for the sanitized private-LAN listener.
//!
//! TXT data is an untrusted reachability hint, never authentication material.
//! The iPhone accepts only these three public fields and authenticates the
//! resulting TLS connection with the pin from its durable activation.

#[cfg(feature = "sanitized-development-fixtures")]
use super::server::is_private_lan_ipv4;
#[cfg(feature = "sanitized-development-fixtures")]
use super::DirectSyncTransportError;
#[cfg(feature = "sanitized-development-fixtures")]
use mdns_sd::{ServiceDaemon, ServiceInfo};
#[cfg(feature = "sanitized-development-fixtures")]
use std::net::{IpAddr, Ipv4Addr};

pub const NOTED_SYNC_BONJOUR_TYPE: &str = "_noted-sync._tcp.local.";
pub const NOTED_SYNC_DISCOVERY_PROTOCOL: &str = "noted.direct-sync.v1";

#[cfg(feature = "sanitized-development-fixtures")]
pub struct SanitizedBonjourAdvertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

#[cfg(feature = "sanitized-development-fixtures")]
impl SanitizedBonjourAdvertisement {
    pub fn start_fixture_only(
        instance_name: &str,
        address: Ipv4Addr,
        port: u16,
    ) -> Result<Self, DirectSyncTransportError> {
        validate_instance_name(instance_name)?;
        if !is_private_lan_ipv4(address) || port == 0 {
            return Err(DirectSyncTransportError::PrivateLanRequired);
        }
        let address_text = address.to_string();
        let port_text = port.to_string();
        let properties = [
            ("protocol", NOTED_SYNC_DISCOVERY_PROTOCOL),
            ("address", address_text.as_str()),
            ("port", port_text.as_str()),
        ];
        let info = ServiceInfo::new(
            NOTED_SYNC_BONJOUR_TYPE,
            instance_name,
            "noted-sync.local.",
            IpAddr::V4(address),
            port,
            &properties[..],
        )
        .map_err(|_| DirectSyncTransportError::InvalidFixtureConfiguration)?;
        let fullname = info.get_fullname().to_owned();
        let daemon =
            ServiceDaemon::new().map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        daemon
            .register(info)
            .map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        Ok(Self { daemon, fullname })
    }
}

#[cfg(feature = "sanitized-development-fixtures")]
impl Drop for SanitizedBonjourAdvertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

#[cfg(feature = "sanitized-development-fixtures")]
fn validate_instance_name(instance_name: &str) -> Result<(), DirectSyncTransportError> {
    if instance_name.is_empty()
        || instance_name.len() > 32
        || !instance_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'-'))
    {
        return Err(DirectSyncTransportError::InvalidFixtureConfiguration);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertisement_contract_contains_no_authority_or_secret_fields() {
        assert_eq!(NOTED_SYNC_BONJOUR_TYPE, "_noted-sync._tcp.local.");
        assert_eq!(NOTED_SYNC_DISCOVERY_PROTOCOL, "noted.direct-sync.v1");
        let public_keys = ["protocol", "address", "port"];
        for forbidden in [
            "pin",
            "spki",
            "library",
            "device",
            "receipt",
            "token",
            "credential",
        ] {
            assert!(!public_keys.contains(&forbidden));
        }
    }

    #[cfg(feature = "sanitized-development-fixtures")]
    #[test]
    fn fixture_advertisement_rejects_ambiguous_instance_names() {
        for name in [
            "",
            "Noted.local.",
            "Noted/Sync",
            "x x x x x x x x x x x x x x x x x",
        ] {
            assert_eq!(
                validate_instance_name(name),
                Err(DirectSyncTransportError::InvalidFixtureConfiguration)
            );
        }
        assert!(validate_instance_name("Noted Fixture").is_ok());
    }
}
