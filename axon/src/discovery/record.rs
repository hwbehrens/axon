//! Uniform view over the two mDNS record types this daemon parses.

use std::net::IpAddr;

use mdns_sd::{ResolvedService, ServiceInfo};

/// The record fields this daemon consumes from an mDNS browse result.
///
/// `mdns-sd` hands resolved browse events over as [`ResolvedService`], while
/// [`ServiceInfo`] stays the type used to build (and, in tests, to describe)
/// advertisements. Both expose the same information under slightly different
/// shapes, so this trait keeps parsing independent of which one arrives.
pub(super) trait ResolvedRecord {
    fn fullname(&self) -> &str;
    fn port(&self) -> u16;
    fn property(&self, key: &str) -> Option<&str>;
    fn addresses(&self) -> Vec<IpAddr>;
}

impl ResolvedRecord for ServiceInfo {
    fn fullname(&self) -> &str {
        self.get_fullname()
    }

    fn port(&self) -> u16 {
        self.get_port()
    }

    fn property(&self, key: &str) -> Option<&str> {
        self.get_property_val_str(key)
    }

    fn addresses(&self) -> Vec<IpAddr> {
        self.get_addresses().iter().copied().collect()
    }
}

impl ResolvedRecord for ResolvedService {
    fn fullname(&self) -> &str {
        self.get_fullname()
    }

    fn port(&self) -> u16 {
        self.get_port()
    }

    fn property(&self, key: &str) -> Option<&str> {
        self.get_property_val_str(key)
    }

    fn addresses(&self) -> Vec<IpAddr> {
        self.get_addresses()
            .iter()
            .map(|address| address.to_ip_addr())
            .collect()
    }
}
