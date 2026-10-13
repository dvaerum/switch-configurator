//! Read-only SNMP lookups (port status, MAC table) as an alternative to the
//! console. SNMP goes over the management network, so it needs neither the
//! switch's single console session nor a queue turn.
//!
//! `client` talks the protocol, `mapping` turns raw table rows into the API's
//! types (pure, tested against rows recorded from real hardware), and
//! `lookup` ties them together per vendor.

pub mod client;
pub mod lookup;
pub mod mapping;

#[cfg(test)]
pub(crate) mod testing;

/// A value read from a switch, reduced to what the mappers need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnmpValue {
    /// INTEGER, and the unsigned counter/gauge types.
    Integer(i64),
    /// OCTET STRING: interface names and MAC addresses alike (a MAC's bytes
    /// can look like text, so the caller decides how to read it).
    Octets(Vec<u8>),
}

impl SnmpValue {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Integer(n) => Some(*n),
            Self::Octets(_) => None,
        }
    }

    /// The octets read as text (invalid UTF-8 replaced).
    pub fn as_text(&self) -> Option<String> {
        match self {
            Self::Octets(bytes) => Some(String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string()),
            Self::Integer(_) => None,
        }
    }

    pub fn as_octets(&self) -> Option<&[u8]> {
        match self {
            Self::Octets(bytes) => Some(bytes),
            Self::Integer(_) => None,
        }
    }
}

/// One row of a walk: the full OID (as numbers) and its value.
pub type Row = (Vec<u32>, SnmpValue);
