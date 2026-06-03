use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use validator::Validate;
use crate::config::Settings;

/// Supported switch vendors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Aruba,
    Fortiswitch,
    Cisco,
}

/// Switch model information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(non_camel_case_types)]
pub enum SwitchModel {
    Aruba2530_24G_POE,
    Aruba2530_8G_POE,
    Aruba2530_48G_2SFP,  // J9855A: 48 GigE ports + 2 SFP+ uplinks
    Aruba2540_24G,
    Aruba2540_48G_4SFP,  // JL355A: 48 GigE ports + 4 SFP+ uplinks
    Aruba2930F,
    Fortiswitch124F_FPOE,
    CiscoCatalyst9300_24P_UPOE,
}

impl SwitchModel {
    pub fn vendor(&self) -> Vendor {
        match self {
            Self::Aruba2530_24G_POE | Self::Aruba2530_8G_POE | Self::Aruba2530_48G_2SFP
            | Self::Aruba2540_24G | Self::Aruba2540_48G_4SFP | Self::Aruba2930F => Vendor::Aruba,
            Self::Fortiswitch124F_FPOE => Vendor::Fortiswitch,
            Self::CiscoCatalyst9300_24P_UPOE => Vendor::Cisco,
        }
    }

    /// Get the maximum speed supported by this switch model
    /// Returns the list of supported SpeedDuplex configurations
    pub fn supported_speeds(&self) -> Vec<SpeedDuplex> {
        use SpeedDuplex::*;

        match self {
            // Aruba 2530 series: Gigabit switches with 10/100/1000 support
            Self::Aruba2530_24G_POE | Self::Aruba2530_8G_POE => {
                vec![Auto, TenHalf, TenFull, HundredHalf, HundredFull, ThousandFull]
            }
            // Aruba 2530-48G-2SFP+: Gigabit switch with 2 SFP+ uplinks (10G support on uplinks)
            Self::Aruba2530_48G_2SFP => {
                vec![Auto, TenHalf, TenFull, HundredHalf, HundredFull, ThousandFull, TenGFull]
            }
            // Aruba 2540-24G: Gigabit switch
            Self::Aruba2540_24G => {
                vec![Auto, TenHalf, TenFull, HundredHalf, HundredFull, ThousandFull]
            }
            // Aruba 2540-48G-4SFP+: 48 Gigabit ports + 4 SFP+ 10G uplinks
            Self::Aruba2540_48G_4SFP => {
                vec![Auto, TenHalf, TenFull, HundredHalf, HundredFull, ThousandFull, TenGFull]
            }
            // Aruba 2930F: Gigabit with optional 10G uplinks (SFP+)
            // For simplicity, supporting up to 1G on regular ports, 10G on uplink ports
            Self::Aruba2930F => {
                vec![Auto, TenHalf, TenFull, HundredHalf, HundredFull, ThousandFull, TenGFull]
            }
            // FortiSwitch 124F: Gigabit switch with SFP uplinks
            Self::Fortiswitch124F_FPOE => {
                vec![Auto, TenHalf, TenFull, HundredHalf, HundredFull, ThousandFull]
            }
            // Cisco Catalyst 9300: Gigabit with 10G uplinks
            Self::CiscoCatalyst9300_24P_UPOE => {
                vec![Auto, TenHalf, TenFull, HundredHalf, HundredFull, ThousandFull, TenGFull]
            }
        }
    }

    /// Check if a specific speed_duplex setting is supported by this switch model
    pub fn supports_speed(&self, speed: SpeedDuplex) -> bool {
        self.supported_speeds().contains(&speed)
    }

    /// Get port-specific speed capabilities
    /// Some switches have different capabilities on different port types (e.g., SFP+ vs RJ45)
    /// Returns true if 10G is available on uplink ports for this model
    pub fn has_10g_uplinks(&self) -> bool {
        matches!(
            self,
            Self::Aruba2530_48G_2SFP | Self::Aruba2540_48G_4SFP | Self::Aruba2930F | Self::CiscoCatalyst9300_24P_UPOE
        )
    }

    /// Parse vendor-specific port identifier to port number
    /// Examples:
    /// - Aruba: "24" -> 24
    /// - Cisco: "GigabitEthernet1/0/24" -> 24
    /// - FortiSwitch: "port24" -> 24
    pub fn parse_port_number(&self, port_id: &str) -> Option<u16> {
        match self.vendor() {
            Vendor::Aruba => port_id.parse().ok(),
            Vendor::Cisco => {
                // Extract last number from "GigabitEthernet1/0/24"
                port_id.split('/').last()?.parse().ok()
            }
            Vendor::Fortiswitch => {
                // Extract number from "port24"
                port_id.trim_start_matches("port").parse().ok()
            }
        }
    }

    /// Get total number of ports on this switch model
    pub fn total_ports(&self) -> u16 {
        match self {
            Self::Aruba2530_8G_POE => 8,
            Self::Aruba2530_24G_POE => 26,
            Self::Aruba2530_48G_2SFP => 50,
            Self::Aruba2540_24G => 28,
            Self::Aruba2540_48G_4SFP => 52,
            Self::Aruba2930F => 52, // Max config
            Self::Fortiswitch124F_FPOE => 26,
            Self::CiscoCatalyst9300_24P_UPOE => 24,
        }
    }

    /// Get comprehensive capabilities for a specific port
    ///
    /// This is the primary method for determining what a port can do.
    /// Use this for validation and command generation.
    pub fn port_capabilities(&self, port_id: &str) -> Option<PortCapabilities> {
        let port_num = self.parse_port_number(port_id)?;

        match self {
            // Aruba 2530-24G PoE+ (J9773A)
            // - Ports 1-24: 1G copper with PoE+ (30W)
            // - Ports 25-26: 1G SFP (no PoE)
            Self::Aruba2530_24G_POE => {
                if port_num >= 1 && port_num <= 24 {
                    Some(PortCapabilities::poe_plus_1g_copper(port_num))
                } else if port_num >= 25 && port_num <= 26 {
                    Some(PortCapabilities {
                        port_type: PortType::Sfp,
                        poe_support: None,
                        supported_speeds: vec![SpeedDuplex::ThousandFull],
                        max_speed_gbps: 1,
                        is_uplink: true,
                        port_number: port_num,
                    })
                } else {
                    None
                }
            }

            // Aruba 2530-8G PoE+ (J9774A)
            // - Ports 1, 3, 5, 7: 1G copper with PoE+ (30W)
            // - Ports 2, 4, 6, 8: 1G copper without PoE
            Self::Aruba2530_8G_POE => {
                if port_num >= 1 && port_num <= 8 {
                    let has_poe = matches!(port_num, 1 | 3 | 5 | 7);
                    Some(if has_poe {
                        PortCapabilities::poe_plus_1g_copper(port_num)
                    } else {
                        PortCapabilities::standard_1g_copper(port_num)
                    })
                } else {
                    None
                }
            }

            // Aruba 2530-48G-2SFP+ (J9855A)
            // - Ports 1-48: 1G copper (no PoE)
            // - Ports 49-50: 10G SFP+ uplinks
            Self::Aruba2530_48G_2SFP => {
                if port_num >= 1 && port_num <= 48 {
                    Some(PortCapabilities::standard_1g_copper(port_num))
                } else if port_num >= 49 && port_num <= 50 {
                    Some(PortCapabilities::sfp_plus_10g_uplink(port_num))
                } else {
                    None
                }
            }

            // Aruba 2540-24G (no PoE variant)
            // - Ports 1-24: 1G copper (no PoE)
            // - Ports 25-28: 1G/10G SFP+ combo ports
            Self::Aruba2540_24G => {
                if port_num >= 1 && port_num <= 24 {
                    Some(PortCapabilities::standard_1g_copper(port_num))
                } else if port_num >= 25 && port_num <= 28 {
                    Some(PortCapabilities::sfp_plus_10g_uplink(port_num))
                } else {
                    None
                }
            }

            // Aruba 2540-48G-4SFP+ (JL355A)
            // - Ports 1-48: 1G copper (no PoE)
            // - Ports 49-52: 10G SFP+ uplinks
            Self::Aruba2540_48G_4SFP => {
                if port_num >= 1 && port_num <= 48 {
                    Some(PortCapabilities::standard_1g_copper(port_num))
                } else if port_num >= 49 && port_num <= 52 {
                    Some(PortCapabilities::sfp_plus_10g_uplink(port_num))
                } else {
                    None
                }
            }

            // Aruba 2930F (various models, some with PoE)
            // Note: Some 2930F models have PoE on all 48 ports, some only on 1-24
            // We assume PoE+ for ports 1-48 for maximum compatibility
            Self::Aruba2930F => {
                if port_num >= 1 && port_num <= 48 {
                    Some(PortCapabilities::poe_plus_1g_copper(port_num))
                } else if port_num >= 49 && port_num <= 52 {
                    // 10G SFP+ uplinks
                    Some(PortCapabilities::sfp_plus_10g_uplink(port_num))
                } else {
                    None
                }
            }

            // FortiSwitch 124F-FPOE
            // - Ports 1-24: 1G copper with PoE+ (30W)
            // - Port 25-26: 10G SFP+ uplinks (needs verification)
            Self::Fortiswitch124F_FPOE => {
                if port_num >= 1 && port_num <= 24 {
                    Some(PortCapabilities::poe_plus_1g_copper(port_num))
                } else if port_num >= 25 && port_num <= 26 {
                    Some(PortCapabilities::sfp_plus_10g_uplink(port_num))
                } else {
                    None
                }
            }

            // Cisco Catalyst 9300-24P with PoE++ (60W per port, equivalent to UPoE)
            // - Ports 1-24: 1G copper with PoE++ (60W)
            // - Uplink module ports vary by configuration
            Self::CiscoCatalyst9300_24P_UPOE => {
                if port_num >= 1 && port_num <= 24 {
                    Some(PortCapabilities {
                        port_type: PortType::Copper,
                        poe_support: Some(PoEStandard::PoEPlusPlus),
                        supported_speeds: vec![
                            SpeedDuplex::Auto,
                            SpeedDuplex::TenHalf,
                            SpeedDuplex::TenFull,
                            SpeedDuplex::HundredHalf,
                            SpeedDuplex::HundredFull,
                            SpeedDuplex::ThousandFull,
                        ],
                        max_speed_gbps: 1,
                        is_uplink: false,
                        port_number: port_num,
                    })
                } else {
                    // Uplink module - would need more info
                    None
                }
            }
        }
    }

    /// Get capabilities for all ports on this switch
    pub fn all_port_capabilities(&self) -> Vec<PortCapabilities> {
        let max_port = self.total_ports();
        (1..=max_port)
            .filter_map(|port_num| {
                let port_id = format!("{}", port_num);
                self.port_capabilities(&port_id)
            })
            .collect()
    }

    /// Quick check if a port supports PoE
    pub fn port_supports_poe(&self, port_id: &str) -> bool {
        self.port_capabilities(port_id)
            .map(|cap| cap.supports_poe())
            .unwrap_or(false)
    }

    /// Check if switch has any PoE capability
    pub fn supports_poe(&self) -> bool {
        matches!(
            self,
            Self::Aruba2530_24G_POE
                | Self::Aruba2530_8G_POE
                | Self::Aruba2930F
                | Self::Fortiswitch124F_FPOE
                | Self::CiscoCatalyst9300_24P_UPOE
        )
    }

    /// Check if switch supports VLAN descriptions
    /// Aruba switches only support VLAN names, not descriptions
    pub fn supports_vlan_description(&self) -> bool {
        matches!(
            self,
            Self::Fortiswitch124F_FPOE | Self::CiscoCatalyst9300_24P_UPOE
        )
    }

    /// Get the maximum VLAN name length for this switch model
    /// Exceeding this limit causes the switch to reject or truncate the name
    pub fn max_vlan_name_length(&self) -> usize {
        match self {
            // Aruba switches: 32 character limit
            Self::Aruba2530_24G_POE
            | Self::Aruba2530_8G_POE
            | Self::Aruba2530_48G_2SFP
            | Self::Aruba2540_24G
            | Self::Aruba2540_48G_4SFP
            | Self::Aruba2930F => 32,

            // Cisco IOS: 32 character limit
            Self::CiscoCatalyst9300_24P_UPOE => 32,

            // FortiSwitch: 63 character limit (FortiOS allows longer names)
            Self::Fortiswitch124F_FPOE => 63,
        }
    }

    /// Get the known hardware product numbers for this switch model.
    ///
    /// These are extracted from the running config header line
    /// (e.g., `; J9779A Configuration Editor;` on Aruba switches).
    /// Used to verify the configured model matches the actual hardware.
    pub fn product_numbers(&self) -> &[&str] {
        match self {
            Self::Aruba2530_24G_POE => &["J9773A", "J9779A", "J9854A"],  // 2530-24G-PoE+, 2530-24-PoE+, 2530-24G-PoE+-2SFP+
            Self::Aruba2530_8G_POE => &["J9774A", "J9780A"],   // 2530-8G-PoE+ and 2530-8-PoE+
            Self::Aruba2530_48G_2SFP => &["J9855A"],           // 2530-48G-2SFP+
            Self::Aruba2540_24G => &["JL354A"],                // 2540-24G
            Self::Aruba2540_48G_4SFP => &["JL355A"],           // 2540-48G-4SFP+
            Self::Aruba2930F => &["JL253A", "JL254A", "JL255A", "JL256A", "JL258A", "JL261A", "JL262A", "JL263A", "JL264A"],
            Self::Fortiswitch124F_FPOE => &["FortiSwitch-124F-FPOE", "S124F"],
            Self::CiscoCatalyst9300_24P_UPOE => &["c9300-24u", "C9300-24U", "C9300-24P"],
        }
    }

    /// Check if switch uses legacy mirror-port syntax
    /// Aruba 2530/2540 series use "mirror-port <dest>"
    /// Aruba 2930F and newer use "mirror <session> port <dest>"
    pub fn uses_legacy_mirror_syntax(&self) -> bool {
        matches!(
            self,
            Self::Aruba2530_24G_POE
                | Self::Aruba2530_8G_POE
                | Self::Aruba2530_48G_2SFP
                | Self::Aruba2540_24G
                | Self::Aruba2540_48G_4SFP
        )
    }

    /// Get list of all PoE-capable port numbers
    pub fn poe_capable_ports(&self) -> Vec<u16> {
        self.all_port_capabilities()
            .into_iter()
            .filter(|cap| cap.supports_poe())
            .map(|cap| cap.port_number)
            .collect()
    }

    /// Validate speed/duplex setting for a specific port
    pub fn validate_port_speed(&self, port_id: &str, speed: SpeedDuplex) -> Result<(), String> {
        let cap = self
            .port_capabilities(port_id)
            .ok_or_else(|| format!("Invalid port: {}", port_id))?;

        if cap.supports_speed(speed) {
            Ok(())
        } else {
            Err(format!(
                "Port {} does not support {:?}. Supported speeds: {:?}",
                port_id, speed, cap.supported_speeds
            ))
        }
    }
}

/// Physical port type on the switch
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortType {
    /// Standard copper RJ45 port (most common)
    Copper,
    /// 1G SFP fiber port
    Sfp,
    /// 10G SFP+ fiber port
    SfpPlus,
    /// 40G QSFP+ fiber port
    QsfpPlus,
    /// 100G QSFP28 fiber port
    Qsfp28,
}

/// PoE standard supported by a port (vendor-neutral, based on IEEE 802.3 standards)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PoEStandard {
    /// IEEE 802.3af - up to 15.4W
    PoE,
    /// IEEE 802.3at - up to 30W
    PoEPlus,
    /// IEEE 802.3bt Type 3 - up to 60W (includes Cisco UPoE equivalent)
    PoEPlusPlus,
    /// IEEE 802.3bt Type 4 - up to 90W
    PoEPlusPlusPlus,
}

impl PoEStandard {
    /// Maximum power delivery in watts
    pub fn max_watts(&self) -> u16 {
        match self {
            Self::PoE => 15,
            Self::PoEPlus => 30,
            Self::PoEPlusPlus => 60,
            Self::PoEPlusPlusPlus => 90,
        }
    }
}

/// Comprehensive capabilities of a specific port
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortCapabilities {
    /// Physical port type
    pub port_type: PortType,

    /// PoE support details (None if port doesn't support PoE)
    pub poe_support: Option<PoEStandard>,

    /// Supported speed/duplex modes
    pub supported_speeds: Vec<SpeedDuplex>,

    /// Maximum speed this port can achieve (in Gbps)
    pub max_speed_gbps: u16,

    /// Whether this is typically an uplink port
    pub is_uplink: bool,

    /// Port number/identifier
    pub port_number: u16,
}

impl PortCapabilities {
    /// Create default capabilities for a standard 1G copper port without PoE
    pub fn standard_1g_copper(port_num: u16) -> Self {
        Self {
            port_type: PortType::Copper,
            poe_support: None,
            supported_speeds: vec![
                SpeedDuplex::Auto,
                SpeedDuplex::TenHalf,
                SpeedDuplex::TenFull,
                SpeedDuplex::HundredHalf,
                SpeedDuplex::HundredFull,
                SpeedDuplex::ThousandFull,
            ],
            max_speed_gbps: 1,
            is_uplink: false,
            port_number: port_num,
        }
    }

    /// Create capabilities for a 1G copper port with PoE+
    pub fn poe_plus_1g_copper(port_num: u16) -> Self {
        Self {
            poe_support: Some(PoEStandard::PoEPlus),
            ..Self::standard_1g_copper(port_num)
        }
    }

    /// Create capabilities for a 10G SFP+ uplink port
    pub fn sfp_plus_10g_uplink(port_num: u16) -> Self {
        Self {
            port_type: PortType::SfpPlus,
            poe_support: None,
            supported_speeds: vec![
                SpeedDuplex::ThousandFull,
                SpeedDuplex::TenGFull,
            ],
            max_speed_gbps: 10,
            is_uplink: true,
            port_number: port_num,
        }
    }

    /// Check if this port supports PoE
    pub fn supports_poe(&self) -> bool {
        self.poe_support.is_some()
    }

    /// Get maximum PoE wattage, or 0 if no PoE
    pub fn max_poe_watts(&self) -> u16 {
        self.poe_support.map(|std| std.max_watts()).unwrap_or(0)
    }

    /// Check if this port supports a specific speed
    pub fn supports_speed(&self, speed: SpeedDuplex) -> bool {
        self.supported_speeds.contains(&speed)
    }
}

/// IP address configuration for VLAN
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VlanIpConfig {
    /// No IP address configured
    None,
    /// Get IP address from DHCP/BOOTP
    Dhcp,
    /// Static IP address with subnet mask
    Static {
        address: String,
        netmask: String,
    },
}

impl Default for VlanIpConfig {
    fn default() -> Self {
        VlanIpConfig::None
    }
}

// Custom serialization/deserialization for VlanIpConfig
impl serde::Serialize for VlanIpConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        match self {
            VlanIpConfig::None => serializer.serialize_str("none"),
            VlanIpConfig::Dhcp => serializer.serialize_str("dhcp"),
            VlanIpConfig::Static { address, netmask } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("address", address)?;
                map.serialize_entry("netmask", netmask)?;
                map.end()
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for VlanIpConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct VlanIpConfigVisitor;

        impl<'de> Visitor<'de> for VlanIpConfigVisitor {
            type Value = VlanIpConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("'none', 'dhcp', or a map with 'address' and 'netmask'")
            }

            fn visit_str<E>(self, value: &str) -> Result<VlanIpConfig, E>
            where
                E: de::Error,
            {
                match value {
                    "none" => Ok(VlanIpConfig::None),
                    "dhcp" => Ok(VlanIpConfig::Dhcp),
                    _ => Err(de::Error::unknown_variant(value, &["none", "dhcp"])),
                }
            }

            fn visit_map<M>(self, mut map: M) -> Result<VlanIpConfig, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut address = None;
                let mut netmask = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "address" => {
                            address = Some(map.next_value()?);
                        }
                        "netmask" => {
                            netmask = Some(map.next_value()?);
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                match (address, netmask) {
                    (Some(addr), Some(mask)) => Ok(VlanIpConfig::Static {
                        address: addr,
                        netmask: mask,
                    }),
                    _ => Err(de::Error::missing_field("address or netmask")),
                }
            }
        }

        deserializer.deserialize_any(VlanIpConfigVisitor)
    }
}

/// VLAN configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate, PartialEq, Eq)]
pub struct Vlan {
    #[validate(range(min = 1, max = 4094))]
    pub id: u16,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// IP address configuration for this VLAN interface
    #[serde(default)]
    pub ip_config: VlanIpConfig,
}

/// Port access mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortMode {
    Access,
    Trunk,
}

/// Speed and duplex configuration for port
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpeedDuplex {
    /// Auto-negotiation (default)
    Auto,
    /// 10 Mbps half-duplex
    #[serde(rename = "10-half")]
    TenHalf,
    /// 10 Mbps full-duplex
    #[serde(rename = "10-full")]
    TenFull,
    /// 100 Mbps half-duplex
    #[serde(rename = "100-half")]
    HundredHalf,
    /// 100 Mbps full-duplex
    #[serde(rename = "100-full")]
    HundredFull,
    /// 1 Gbps full-duplex
    #[serde(rename = "1000-full")]
    ThousandFull,
    /// 10 Gbps full-duplex
    #[serde(rename = "10g-full")]
    TenGFull,
}

impl Default for SpeedDuplex {
    fn default() -> Self {
        SpeedDuplex::Auto
    }
}

impl SpeedDuplex {
    /// Convert to Aruba syntax (speed-duplex command)
    pub fn to_aruba_syntax(&self) -> &'static str {
        match self {
            SpeedDuplex::Auto => "auto",
            SpeedDuplex::TenHalf => "10-half",
            SpeedDuplex::TenFull => "10-full",
            SpeedDuplex::HundredHalf => "100-half",
            SpeedDuplex::HundredFull => "100-full",
            SpeedDuplex::ThousandFull => "1000-full",
            SpeedDuplex::TenGFull => "10g-full",
        }
    }

    /// Convert to Cisco speed value
    pub fn to_cisco_speed(&self) -> &'static str {
        match self {
            SpeedDuplex::Auto => "auto",
            SpeedDuplex::TenHalf | SpeedDuplex::TenFull => "10",
            SpeedDuplex::HundredHalf | SpeedDuplex::HundredFull => "100",
            SpeedDuplex::ThousandFull => "1000",
            SpeedDuplex::TenGFull => "10000",
        }
    }

    /// Convert to Cisco duplex value
    pub fn to_cisco_duplex(&self) -> &'static str {
        match self {
            SpeedDuplex::Auto => "auto",
            SpeedDuplex::TenHalf | SpeedDuplex::HundredHalf => "half",
            SpeedDuplex::TenFull | SpeedDuplex::HundredFull | SpeedDuplex::ThousandFull | SpeedDuplex::TenGFull => "full",
        }
    }

    /// Parse from Aruba output (e.g., "Auto", "100FDx", "1000FDx")
    pub fn from_aruba_output(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            // Standard formats (used in commands)
            "auto" => Some(SpeedDuplex::Auto),
            "10-half" | "10half" | "10hdx" => Some(SpeedDuplex::TenHalf),
            "10-full" | "10full" | "10fdx" => Some(SpeedDuplex::TenFull),
            "100-half" | "100half" | "100hdx" => Some(SpeedDuplex::HundredHalf),
            "100-full" | "100full" | "100fdx" => Some(SpeedDuplex::HundredFull),
            "1000-full" | "1000full" | "1000fdx" => Some(SpeedDuplex::ThousandFull),
            "10g-full" | "10gfull" | "10gfdx" => Some(SpeedDuplex::TenGFull),
            _ => None,
        }
    }
}

/// Port configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate, PartialEq)]
pub struct Port {
    /// Port identifier (e.g., "1/0/1", "GigabitEthernet1/0/1")
    pub port_id: String,

    /// Port mode (access or trunk)
    pub mode: PortMode,

    /// VLAN ID for access mode, or native VLAN for trunk mode
    #[validate(range(min = 1, max = 4094))]
    pub vlan: u16,

    /// Allowed VLANs for trunk mode
    #[serde(default)]
    pub allowed_vlans: Vec<u16>,

    /// Port description
    #[serde(default)]
    pub description: Option<String>,

    /// Enable/disable the port
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Enable/disable PoE on the port
    #[serde(default)]
    pub poe_enabled: bool,

    /// Enable MAC address change notifications on this port
    #[serde(default)]
    pub mac_notify: bool,

    /// Speed and duplex configuration
    #[serde(default)]
    pub speed_duplex: SpeedDuplex,
}

fn default_true() -> bool {
    true
}

/// Port mirroring (SPAN) configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PortMirror {
    /// Mirror session name/ID
    pub session_id: String,

    /// Source ports to mirror
    pub source_ports: Vec<String>,

    /// Destination port for mirrored traffic
    pub destination_port: String,

    /// Direction to mirror (rx, tx, both)
    #[serde(default = "default_mirror_direction")]
    pub direction: MirrorDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MirrorDirection {
    Rx,
    Tx,
    Both,
}

fn default_mirror_direction() -> MirrorDirection {
    MirrorDirection::Both
}

/// SNMP community configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnmpCommunity {
    /// Community string
    pub name: String,

    /// Access level
    #[serde(default)]
    pub access: SnmpAccess,
}

/// SNMP access level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnmpAccess {
    /// Read-write access
    Unrestricted,
    /// Read-write access (manager)
    Manager,
    /// Read-only access (operator)
    Operator,
}

impl Default for SnmpAccess {
    fn default() -> Self {
        Self::Unrestricted
    }
}

/// SNMP trap receiver configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnmpTrapReceiver {
    /// IP address or hostname of trap receiver
    pub host: String,

    /// Community string for traps
    pub community: String,

    /// SNMP version (optional, defaults to v2c)
    #[serde(default)]
    pub version: Option<String>,
}

/// Types of SNMP traps to enable
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrapType {
    /// MAC address learning notifications
    MacNotify,
    /// Port link up/down events
    LinkChange,
    /// All traps
    All,
}

/// SNMP configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnmpConfig {
    /// SNMP community strings
    #[serde(default)]
    pub communities: Vec<SnmpCommunity>,

    /// SNMP trap receivers
    #[serde(default)]
    pub trap_receivers: Vec<SnmpTrapReceiver>,

    /// Types of traps to enable
    #[serde(default)]
    pub enabled_traps: Vec<TrapType>,
}

/// Complete switch configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct SwitchConfig {
    /// Unique identifier for merging switches across configs
    #[validate(length(min = 1))]
    pub id: String,

    /// Switch hostname or identifier
    /// Optional for multi-config: only required in one config file
    /// Must match across all configs if present in multiple files
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// Switch model
    /// Optional for multi-config: only required in one config file
    /// Must match across all configs if present in multiple files
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<SwitchModel>,

    /// Management IP address
    /// Optional for multi-config: only required in one config file
    /// Must match across all configs if present in multiple files
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_ip: Option<String>,

    /// SSH credentials
    /// Optional for multi-config: only required in one config file
    /// Must match across all configs if present in multiple files
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Credentials>,

    /// VLANs to configure
    /// Defaults to empty list if not specified
    #[validate(nested)]
    #[serde(default)]
    pub vlans: Vec<Vlan>,

    /// Ports to configure
    /// Defaults to empty list if not specified
    #[validate(nested)]
    #[serde(default)]
    pub ports: Vec<Port>,

    /// Port mirroring configurations
    #[serde(default)]
    pub port_mirrors: Vec<PortMirror>,

    /// SNMP configuration (communities, trap receivers, enabled traps)
    #[serde(default)]
    pub snmp: Option<SnmpConfig>,

    /// Management VLAN configuration
    /// Specifies which VLAN should be used for switch management access.
    /// Vendor implementations:
    /// - Aruba: Restricts management access to only this VLAN (security feature)
    /// - Cisco: Designates which VLAN has the management IP/SVI
    /// - FortiSwitch: Configures allowaccess on this VLAN interface
    #[serde(default)]
    pub management_vlan: Option<u16>,

    /// Validation tests to run before saving configuration
    #[serde(default)]
    pub validation: Option<crate::validation::ValidationConfig>,

    /// Additional vendor-specific configuration
    #[serde(default)]
    pub vendor_specific: HashMap<String, serde_json::Value>,

    /// Per-switch settings (moved from global AppConfig)
    #[serde(default)]
    pub settings: Settings,
}

impl SwitchConfig {
    /// Get hostname, panicking if not present (use after validation)
    pub fn hostname(&self) -> &str {
        self.hostname.as_ref().expect("hostname validated")
    }

    /// Get model, panicking if not present (use after validation)
    pub fn model(&self) -> &SwitchModel {
        self.model.as_ref().expect("model validated")
    }

    /// Get management_ip, panicking if not present (use after validation)
    pub fn management_ip(&self) -> &str {
        self.management_ip.as_ref().expect("management_ip validated")
    }

    /// Get credentials, panicking if not present (use after validation)
    pub fn credentials(&self) -> &Credentials {
        self.credentials.as_ref().expect("credentials validated")
    }
}

/// Connection type for switch access
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionType {
    Ssh,
    Serial,
}

impl Default for ConnectionType {
    fn default() -> Self {
        Self::Ssh
    }
}

/// Jump host configuration for SSH proxy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JumpHost {
    /// Host specification, supports multiple formats:
    /// - "hostname"                    -> hostname, default port 22
    /// - "hostname:port"               -> hostname with custom port
    /// - "user@hostname"               -> hostname with embedded username
    /// - "user@hostname:port"          -> full specification
    pub host: String,

    /// Optional username override
    /// Precedence: explicit username > host-embedded username > target username
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Optional port override
    /// Precedence: explicit port > host-embedded port > 22
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// SSH private key path (tried first)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_key_path: Option<String>,

    /// Password authentication (tried after SSH key fails or if no key)
    #[serde(skip_serializing)]
    pub password: Option<String>,
}

/// Resolved jump host connection parameters
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedJumpHost {
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub ssh_key_path: Option<String>,
    pub password: Option<String>,
}

/// Credentials and connection information for switch access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    #[serde(skip_serializing)]
    pub password: Option<String>,
    pub ssh_key_path: Option<String>,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Connection type (SSH or Serial)
    #[serde(default)]
    pub connection_type: ConnectionType,
    /// Serial device path (e.g., /dev/ttyUSB0 or /dev/serial/by-id/...)
    pub serial_device: Option<String>,
    /// Serial baud rate
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,

    /// Enable secret for entering privileged exec mode (e.g., Aruba `enable`, Cisco `enable`).
    /// If not set, the login password is used as fallback.
    #[serde(default, skip_serializing)]
    pub enable_secret: Option<String>,

    /// Chain of jump hosts (bastion servers) to proxy through
    /// Connections are chained: local -> jump_hosts[0] -> jump_hosts[1] -> ... -> target
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump_hosts: Option<Vec<JumpHost>>,
}

fn default_baud_rate() -> u32 {
    9600
}

fn default_ssh_port() -> u16 {
    22
}

/// Configuration change result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigResult {
    pub switch: String,
    pub success: bool,
    pub message: String,
    pub commands_executed: Vec<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Represents the current state of a switch
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SwitchState {
    pub vlans: Vec<Vlan>,
    pub ports: Vec<Port>,
    pub port_mirrors: Vec<PortMirror>,
    pub snmp: Option<SnmpConfig>,
    pub management_vlan: Option<u16>,
    /// Warnings detected during state parsing (e.g., model mismatch)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Granular SNMP state difference for efficient configuration
/// Instead of replacing all SNMP config, this tracks individual changes
#[derive(Debug, Clone, Default, Serialize)]
pub struct SnmpStateDiff {
    /// Communities to add (not present in current config)
    pub communities_to_add: Vec<SnmpCommunity>,
    /// Community names to remove (present in current but not in desired)
    pub communities_to_remove: Vec<String>,
    /// Communities to update (same name but different access level)
    pub communities_to_update: Vec<SnmpCommunity>,

    /// Trap receivers to add (not present in current config)
    pub trap_receivers_to_add: Vec<SnmpTrapReceiver>,
    /// Trap receiver hosts to remove (present in current but not in desired)
    pub trap_receivers_to_remove: Vec<String>,

    /// Trap types to enable (not currently enabled)
    pub traps_to_enable: Vec<TrapType>,
    /// Trap types to disable (currently enabled but not in desired)
    pub traps_to_disable: Vec<TrapType>,
}

impl SnmpStateDiff {
    /// Check if there are any SNMP changes to apply
    pub fn has_changes(&self) -> bool {
        !self.communities_to_add.is_empty()
            || !self.communities_to_remove.is_empty()
            || !self.communities_to_update.is_empty()
            || !self.trap_receivers_to_add.is_empty()
            || !self.trap_receivers_to_remove.is_empty()
            || !self.traps_to_enable.is_empty()
            || !self.traps_to_disable.is_empty()
    }
}

/// Categorized CLI commands for preview (what would be sent to the switch)
#[derive(Debug, Clone, Default, Serialize)]
pub struct CommandPreview {
    pub vlan_commands: Vec<String>,
    pub port_commands: Vec<String>,
    pub mirror_commands: Vec<String>,
    pub snmp_commands: Vec<String>,
    pub reset_commands: Vec<String>,
    pub other_commands: Vec<String>,
}

/// Represents the difference between current and desired state
#[derive(Debug, Clone, Default, Serialize)]
pub struct StateDiff {
    pub vlans_to_add: Vec<Vlan>,
    pub vlans_to_remove: Vec<u16>,
    pub vlans_to_update: Vec<Vlan>,

    pub ports_to_configure: Vec<Port>,
    pub ports_to_reset: Vec<String>,  // Port IDs to reset to default state

    pub mirrors_to_add: Vec<PortMirror>,
    pub mirrors_to_remove: Vec<String>,
    pub mirrors_to_update: Vec<PortMirror>,

    /// Mirror destination port IDs that need baseline config (VLAN 1, enabled, access mode)
    pub mirror_dest_ports_to_configure: Vec<String>,

    /// Legacy field - kept for backward compatibility
    pub snmp_config_changed: bool,
    /// Legacy field - the full desired SNMP config (used as fallback)
    pub snmp_config: Option<SnmpConfig>,
    /// Granular SNMP diff for efficient updates
    pub snmp_diff: Option<SnmpStateDiff>,

    pub management_vlan_changed: bool,
    pub management_vlan: Option<u16>,
}

impl StateDiff {
    /// Check if there are any changes to apply
    pub fn has_changes(&self) -> bool {
        !self.vlans_to_add.is_empty()
            || !self.vlans_to_remove.is_empty()
            || !self.vlans_to_update.is_empty()
            || !self.ports_to_configure.is_empty()
            || !self.ports_to_reset.is_empty()
            || !self.mirrors_to_add.is_empty()
            || !self.mirrors_to_remove.is_empty()
            || !self.mirrors_to_update.is_empty()
            || !self.mirror_dest_ports_to_configure.is_empty()
            || self.snmp_config_changed
            || self.snmp_diff.as_ref().map_or(false, |d| d.has_changes())
            || self.management_vlan_changed
    }

    /// Produce a human-readable summary of remaining changes.
    /// Used for convergence warnings when changes persist after apply.
    pub fn remaining_changes_summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.vlans_to_add.is_empty() {
            parts.push(format!("{} VLANs to add", self.vlans_to_add.len()));
        }
        if !self.vlans_to_remove.is_empty() {
            parts.push(format!("{} VLANs to remove", self.vlans_to_remove.len()));
        }
        if !self.vlans_to_update.is_empty() {
            parts.push(format!("{} VLANs to update", self.vlans_to_update.len()));
        }
        if !self.ports_to_configure.is_empty() {
            let port_ids: Vec<&str> = self.ports_to_configure.iter().map(|p| p.port_id.as_str()).collect();
            parts.push(format!("ports to configure: [{}]", port_ids.join(", ")));
        }
        if !self.ports_to_reset.is_empty() {
            parts.push(format!("ports to reset: [{}]", self.ports_to_reset.join(", ")));
        }
        if !self.mirrors_to_add.is_empty() {
            parts.push(format!("{} mirrors to add", self.mirrors_to_add.len()));
        }
        if !self.mirrors_to_remove.is_empty() {
            parts.push(format!("{} mirrors to remove", self.mirrors_to_remove.len()));
        }
        if !self.mirrors_to_update.is_empty() {
            parts.push(format!("{} mirrors to update", self.mirrors_to_update.len()));
        }
        if !self.mirror_dest_ports_to_configure.is_empty() {
            parts.push(format!("mirror dest ports: [{}]", self.mirror_dest_ports_to_configure.join(", ")));
        }
        if self.snmp_config_changed || self.snmp_diff.as_ref().map_or(false, |d| d.has_changes()) {
            parts.push("SNMP config".to_string());
        }
        if self.management_vlan_changed {
            parts.push("management VLAN".to_string());
        }
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== SpeedDuplex Tests ==========

    #[test]
    fn test_speed_duplex_to_aruba_syntax() {
        assert_eq!(SpeedDuplex::Auto.to_aruba_syntax(), "auto");
        assert_eq!(SpeedDuplex::TenHalf.to_aruba_syntax(), "10-half");
        assert_eq!(SpeedDuplex::TenFull.to_aruba_syntax(), "10-full");
        assert_eq!(SpeedDuplex::HundredHalf.to_aruba_syntax(), "100-half");
        assert_eq!(SpeedDuplex::HundredFull.to_aruba_syntax(), "100-full");
        assert_eq!(SpeedDuplex::ThousandFull.to_aruba_syntax(), "1000-full");
        assert_eq!(SpeedDuplex::TenGFull.to_aruba_syntax(), "10g-full");
    }

    #[test]
    fn test_speed_duplex_to_cisco_speed() {
        assert_eq!(SpeedDuplex::Auto.to_cisco_speed(), "auto");
        assert_eq!(SpeedDuplex::TenHalf.to_cisco_speed(), "10");
        assert_eq!(SpeedDuplex::TenFull.to_cisco_speed(), "10");
        assert_eq!(SpeedDuplex::HundredHalf.to_cisco_speed(), "100");
        assert_eq!(SpeedDuplex::HundredFull.to_cisco_speed(), "100");
        assert_eq!(SpeedDuplex::ThousandFull.to_cisco_speed(), "1000");
        assert_eq!(SpeedDuplex::TenGFull.to_cisco_speed(), "10000");
    }

    #[test]
    fn test_speed_duplex_to_cisco_duplex() {
        assert_eq!(SpeedDuplex::Auto.to_cisco_duplex(), "auto");
        assert_eq!(SpeedDuplex::TenHalf.to_cisco_duplex(), "half");
        assert_eq!(SpeedDuplex::TenFull.to_cisco_duplex(), "full");
        assert_eq!(SpeedDuplex::HundredHalf.to_cisco_duplex(), "half");
        assert_eq!(SpeedDuplex::HundredFull.to_cisco_duplex(), "full");
        assert_eq!(SpeedDuplex::ThousandFull.to_cisco_duplex(), "full");
        assert_eq!(SpeedDuplex::TenGFull.to_cisco_duplex(), "full");
    }

    #[test]
    fn test_speed_duplex_from_aruba_output_standard_format() {
        // Test standard hyphenated format (what Aruba shows in running-config)
        assert_eq!(SpeedDuplex::from_aruba_output("auto"), Some(SpeedDuplex::Auto));
        assert_eq!(SpeedDuplex::from_aruba_output("10-half"), Some(SpeedDuplex::TenHalf));
        assert_eq!(SpeedDuplex::from_aruba_output("10-full"), Some(SpeedDuplex::TenFull));
        assert_eq!(SpeedDuplex::from_aruba_output("100-half"), Some(SpeedDuplex::HundredHalf));
        assert_eq!(SpeedDuplex::from_aruba_output("100-full"), Some(SpeedDuplex::HundredFull));
        assert_eq!(SpeedDuplex::from_aruba_output("1000-full"), Some(SpeedDuplex::ThousandFull));
        assert_eq!(SpeedDuplex::from_aruba_output("10g-full"), Some(SpeedDuplex::TenGFull));
    }

    #[test]
    fn test_speed_duplex_from_aruba_output_abbreviated_format() {
        // Test abbreviated format (fdx/hdx)
        assert_eq!(SpeedDuplex::from_aruba_output("10hdx"), Some(SpeedDuplex::TenHalf));
        assert_eq!(SpeedDuplex::from_aruba_output("10fdx"), Some(SpeedDuplex::TenFull));
        assert_eq!(SpeedDuplex::from_aruba_output("100hdx"), Some(SpeedDuplex::HundredHalf));
        assert_eq!(SpeedDuplex::from_aruba_output("100fdx"), Some(SpeedDuplex::HundredFull));
        assert_eq!(SpeedDuplex::from_aruba_output("1000fdx"), Some(SpeedDuplex::ThousandFull));
        assert_eq!(SpeedDuplex::from_aruba_output("10gfdx"), Some(SpeedDuplex::TenGFull));
    }

    #[test]
    fn test_speed_duplex_from_aruba_output_compact_format() {
        // Test compact format (no separator)
        assert_eq!(SpeedDuplex::from_aruba_output("10half"), Some(SpeedDuplex::TenHalf));
        assert_eq!(SpeedDuplex::from_aruba_output("10full"), Some(SpeedDuplex::TenFull));
        assert_eq!(SpeedDuplex::from_aruba_output("100half"), Some(SpeedDuplex::HundredHalf));
        assert_eq!(SpeedDuplex::from_aruba_output("100full"), Some(SpeedDuplex::HundredFull));
        assert_eq!(SpeedDuplex::from_aruba_output("1000full"), Some(SpeedDuplex::ThousandFull));
        assert_eq!(SpeedDuplex::from_aruba_output("10gfull"), Some(SpeedDuplex::TenGFull));
    }

    #[test]
    fn test_speed_duplex_from_aruba_output_case_insensitive() {
        // Test case insensitivity
        assert_eq!(SpeedDuplex::from_aruba_output("AUTO"), Some(SpeedDuplex::Auto));
        assert_eq!(SpeedDuplex::from_aruba_output("Auto"), Some(SpeedDuplex::Auto));
        assert_eq!(SpeedDuplex::from_aruba_output("100-FULL"), Some(SpeedDuplex::HundredFull));
        assert_eq!(SpeedDuplex::from_aruba_output("100-Full"), Some(SpeedDuplex::HundredFull));
        assert_eq!(SpeedDuplex::from_aruba_output("100FDX"), Some(SpeedDuplex::HundredFull));
    }

    #[test]
    fn test_speed_duplex_from_aruba_output_invalid() {
        // Test invalid inputs return None
        assert_eq!(SpeedDuplex::from_aruba_output("invalid"), None);
        assert_eq!(SpeedDuplex::from_aruba_output(""), None);
        assert_eq!(SpeedDuplex::from_aruba_output("200-full"), None);
        assert_eq!(SpeedDuplex::from_aruba_output("auto-full"), None);
    }

    #[test]
    fn test_speed_duplex_round_trip_aruba() {
        // Test that we can convert to Aruba format and parse it back
        let speeds = vec![
            SpeedDuplex::Auto,
            SpeedDuplex::TenHalf,
            SpeedDuplex::TenFull,
            SpeedDuplex::HundredHalf,
            SpeedDuplex::HundredFull,
            SpeedDuplex::ThousandFull,
            SpeedDuplex::TenGFull,
        ];

        for speed in speeds {
            let aruba_str = speed.to_aruba_syntax();
            let parsed = SpeedDuplex::from_aruba_output(&aruba_str);
            assert_eq!(parsed, Some(speed), 
                      "Round-trip failed for {:?}: {} -> {:?}", speed, aruba_str, parsed);
        }
    }

    // ========== SwitchModel Speed Support Tests ==========

    #[test]
    fn test_switch_model_supported_speeds() {
        // Aruba 2530 (Fast Ethernet, no 10G)
        let aruba_2530 = SwitchModel::Aruba2530_24G_POE;
        let speeds_2530 = aruba_2530.supported_speeds();
        assert!(speeds_2530.contains(&SpeedDuplex::Auto));
        assert!(speeds_2530.contains(&SpeedDuplex::HundredFull));
        assert!(speeds_2530.contains(&SpeedDuplex::ThousandFull));
        assert!(!speeds_2530.contains(&SpeedDuplex::TenGFull));

        // Aruba 2930F (Gigabit + 10G uplinks)
        let aruba_2930f = SwitchModel::Aruba2930F;
        let speeds_2930f = aruba_2930f.supported_speeds();
        assert!(speeds_2930f.contains(&SpeedDuplex::Auto));
        assert!(speeds_2930f.contains(&SpeedDuplex::HundredFull));
        assert!(speeds_2930f.contains(&SpeedDuplex::ThousandFull));
        assert!(speeds_2930f.contains(&SpeedDuplex::TenGFull));

        // Cisco Catalyst 9300 (all speeds)
        let cisco_9300 = SwitchModel::CiscoCatalyst9300_24P_UPOE;
        let speeds_9300 = cisco_9300.supported_speeds();
        assert!(speeds_9300.contains(&SpeedDuplex::Auto));
        assert!(speeds_9300.contains(&SpeedDuplex::TenGFull));
    }

    #[test]
    fn test_switch_model_supports_speed() {
        let aruba_2530 = SwitchModel::Aruba2530_24G_POE;
        
        // Should support common speeds
        assert!(aruba_2530.supports_speed(SpeedDuplex::Auto));
        assert!(aruba_2530.supports_speed(SpeedDuplex::TenFull));
        assert!(aruba_2530.supports_speed(SpeedDuplex::HundredFull));
        assert!(aruba_2530.supports_speed(SpeedDuplex::ThousandFull));
        
        // Should NOT support 10G
        assert!(!aruba_2530.supports_speed(SpeedDuplex::TenGFull));

        let aruba_2930f = SwitchModel::Aruba2930F;
        // Should support 10G
        assert!(aruba_2930f.supports_speed(SpeedDuplex::TenGFull));
    }

    // ========== Port Speed_Duplex Tests ==========

    #[test]
    fn test_port_with_different_speeds() {
        let port_auto = Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
        };

        let port_100full = Port {
            port_id: "2".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::HundredFull,
        };

        assert_eq!(port_auto.speed_duplex, SpeedDuplex::Auto);
        assert_eq!(port_100full.speed_duplex, SpeedDuplex::HundredFull);
        assert_ne!(port_auto.speed_duplex, port_100full.speed_duplex);
    }

    #[test]
    fn test_port_default_speed_is_auto() {
        // When deserializing a port without speed_duplex, it should default to Auto
        let yaml = r#"
            port_id: "1"
            mode: access
            vlan: 10
            enabled: true
            poe_enabled: false
        "#;

        let port: Result<Port, _> = serde_yaml::from_str(yaml);
        assert!(port.is_ok());
        assert_eq!(port.unwrap().speed_duplex, SpeedDuplex::Auto);
    }

    #[test]
    fn test_port_speed_duplex_serialization() {
        let port = Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: Some("Test".to_string()),
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::HundredFull,
        };

        let yaml = serde_yaml::to_string(&port).unwrap();
        assert!(yaml.contains("speed_duplex"));
        assert!(yaml.contains("100-full"));

        let deserialized: Port = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deserialized.speed_duplex, SpeedDuplex::HundredFull);
    }

    // ========== PoEStandard Tests ==========

    #[test]
    fn test_poe_standard_max_watts() {
        assert_eq!(PoEStandard::PoE.max_watts(), 15);
        assert_eq!(PoEStandard::PoEPlus.max_watts(), 30);
        assert_eq!(PoEStandard::PoEPlusPlus.max_watts(), 60);
        assert_eq!(PoEStandard::PoEPlusPlusPlus.max_watts(), 90);
    }

    // ========== PortCapabilities Tests ==========

    #[test]
    fn test_port_capabilities_standard_1g_copper() {
        let caps = PortCapabilities::standard_1g_copper(5);
        assert_eq!(caps.port_type, PortType::Copper);
        assert_eq!(caps.poe_support, None);
        assert_eq!(caps.max_speed_gbps, 1);
        assert!(!caps.is_uplink);
        assert_eq!(caps.port_number, 5);
        assert!(!caps.supports_poe());
        assert_eq!(caps.max_poe_watts(), 0);
    }

    #[test]
    fn test_port_capabilities_poe_plus_1g_copper() {
        let caps = PortCapabilities::poe_plus_1g_copper(10);
        assert_eq!(caps.port_type, PortType::Copper);
        assert_eq!(caps.poe_support, Some(PoEStandard::PoEPlus));
        assert_eq!(caps.max_speed_gbps, 1);
        assert!(!caps.is_uplink);
        assert_eq!(caps.port_number, 10);
        assert!(caps.supports_poe());
        assert_eq!(caps.max_poe_watts(), 30);
    }

    #[test]
    fn test_port_capabilities_sfp_plus_10g_uplink() {
        let caps = PortCapabilities::sfp_plus_10g_uplink(49);
        assert_eq!(caps.port_type, PortType::SfpPlus);
        assert_eq!(caps.poe_support, None);
        assert_eq!(caps.max_speed_gbps, 10);
        assert!(caps.is_uplink);
        assert_eq!(caps.port_number, 49);
        assert!(!caps.supports_poe());
    }

    #[test]
    fn test_port_capabilities_supports_speed() {
        let caps = PortCapabilities::standard_1g_copper(1);
        assert!(caps.supports_speed(SpeedDuplex::Auto));
        assert!(caps.supports_speed(SpeedDuplex::HundredFull));
        assert!(caps.supports_speed(SpeedDuplex::ThousandFull));
        assert!(!caps.supports_speed(SpeedDuplex::TenGFull));
    }

    // ========== SwitchModel Port Capability Tests ==========

    #[test]
    fn test_aruba_2530_24g_poe_port_capabilities() {
        let model = SwitchModel::Aruba2530_24G_POE;

        // Test PoE+ copper ports (1-24)
        let port1 = model.port_capabilities("1").unwrap();
        assert_eq!(port1.port_type, PortType::Copper);
        assert_eq!(port1.poe_support, Some(PoEStandard::PoEPlus));
        assert!(port1.supports_poe());
        assert_eq!(port1.max_poe_watts(), 30);

        let port24 = model.port_capabilities("24").unwrap();
        assert!(port24.supports_poe());

        // Test SFP uplink ports (25-26)
        let port25 = model.port_capabilities("25").unwrap();
        assert_eq!(port25.port_type, PortType::Sfp);
        assert!(!port25.supports_poe());
        assert!(port25.is_uplink);

        // Test invalid port
        assert!(model.port_capabilities("30").is_none());
    }

    #[test]
    fn test_aruba_2530_8g_poe_port_capabilities() {
        let model = SwitchModel::Aruba2530_8G_POE;

        // Odd ports have PoE
        assert!(model.port_capabilities("1").unwrap().supports_poe());
        assert!(model.port_capabilities("3").unwrap().supports_poe());
        assert!(model.port_capabilities("5").unwrap().supports_poe());
        assert!(model.port_capabilities("7").unwrap().supports_poe());

        // Even ports don't have PoE
        assert!(!model.port_capabilities("2").unwrap().supports_poe());
        assert!(!model.port_capabilities("4").unwrap().supports_poe());
        assert!(!model.port_capabilities("6").unwrap().supports_poe());
        assert!(!model.port_capabilities("8").unwrap().supports_poe());
    }

    #[test]
    fn test_aruba_2530_48g_2sfp_port_capabilities() {
        let model = SwitchModel::Aruba2530_48G_2SFP;

        // No PoE on regular ports
        assert!(!model.port_capabilities("1").unwrap().supports_poe());
        assert!(!model.port_capabilities("24").unwrap().supports_poe());
        assert!(!model.port_capabilities("48").unwrap().supports_poe());

        // SFP+ uplinks (49-50)
        let port49 = model.port_capabilities("49").unwrap();
        assert_eq!(port49.port_type, PortType::SfpPlus);
        assert_eq!(port49.max_speed_gbps, 10);
        assert!(port49.is_uplink);
    }

    #[test]
    fn test_aruba_2930f_port_capabilities() {
        let model = SwitchModel::Aruba2930F;

        // Ports 1-48 have PoE+
        assert!(model.port_capabilities("1").unwrap().supports_poe());
        assert!(model.port_capabilities("24").unwrap().supports_poe());
        assert!(model.port_capabilities("48").unwrap().supports_poe());

        // SFP+ uplinks (49-52)
        let port50 = model.port_capabilities("50").unwrap();
        assert_eq!(port50.port_type, PortType::SfpPlus);
        assert!(!port50.supports_poe());
    }

    #[test]
    fn test_cisco_catalyst_9300_port_capabilities() {
        let model = SwitchModel::CiscoCatalyst9300_24P_UPOE;

        // All 24 ports have PoE++ (60W, UPoE equivalent)
        let port1 = model.port_capabilities("1").unwrap();
        assert_eq!(port1.poe_support, Some(PoEStandard::PoEPlusPlus));
        assert_eq!(port1.max_poe_watts(), 60);

        let port24 = model.port_capabilities("24").unwrap();
        assert!(port24.supports_poe());
        assert_eq!(port24.max_poe_watts(), 60);
    }

    #[test]
    fn test_fortiswitch_124f_fpoe_port_capabilities() {
        let model = SwitchModel::Fortiswitch124F_FPOE;

        // Ports 1-24 have PoE+
        assert!(model.port_capabilities("1").unwrap().supports_poe());
        assert!(model.port_capabilities("24").unwrap().supports_poe());

        // SFP+ uplinks (25-26)
        let port25 = model.port_capabilities("25").unwrap();
        assert_eq!(port25.port_type, PortType::SfpPlus);
        assert!(port25.is_uplink);
    }

    #[test]
    fn test_switch_model_total_ports() {
        assert_eq!(SwitchModel::Aruba2530_8G_POE.total_ports(), 8);
        assert_eq!(SwitchModel::Aruba2530_24G_POE.total_ports(), 26);
        assert_eq!(SwitchModel::Aruba2530_48G_2SFP.total_ports(), 50);
        assert_eq!(SwitchModel::Aruba2540_24G.total_ports(), 28);
        assert_eq!(SwitchModel::Aruba2540_48G_4SFP.total_ports(), 52);
        assert_eq!(SwitchModel::Aruba2930F.total_ports(), 52);
        assert_eq!(SwitchModel::Fortiswitch124F_FPOE.total_ports(), 26);
        assert_eq!(SwitchModel::CiscoCatalyst9300_24P_UPOE.total_ports(), 24);
    }

    #[test]
    fn test_switch_model_supports_poe() {
        // Models with PoE
        assert!(SwitchModel::Aruba2530_24G_POE.supports_poe());
        assert!(SwitchModel::Aruba2530_8G_POE.supports_poe());
        assert!(SwitchModel::Aruba2930F.supports_poe());
        assert!(SwitchModel::Fortiswitch124F_FPOE.supports_poe());
        assert!(SwitchModel::CiscoCatalyst9300_24P_UPOE.supports_poe());

        // Models without PoE
        assert!(!SwitchModel::Aruba2530_48G_2SFP.supports_poe());
        assert!(!SwitchModel::Aruba2540_24G.supports_poe());
        assert!(!SwitchModel::Aruba2540_48G_4SFP.supports_poe());
    }

    #[test]
    fn test_switch_model_poe_capable_ports() {
        let model = SwitchModel::Aruba2530_8G_POE;
        let poe_ports = model.poe_capable_ports();

        // Only odd ports have PoE on this model
        assert_eq!(poe_ports, vec![1, 3, 5, 7]);

        let model_24g = SwitchModel::Aruba2530_24G_POE;
        let poe_ports_24g = model_24g.poe_capable_ports();

        // Ports 1-24 have PoE
        assert_eq!(poe_ports_24g.len(), 24);
        assert_eq!(poe_ports_24g[0], 1);
        assert_eq!(poe_ports_24g[23], 24);
    }

    #[test]
    fn test_switch_model_validate_port_speed() {
        let model = SwitchModel::Aruba2530_24G_POE;

        // Valid speeds for 1G copper ports
        assert!(model.validate_port_speed("1", SpeedDuplex::Auto).is_ok());
        assert!(model.validate_port_speed("1", SpeedDuplex::HundredFull).is_ok());
        assert!(model.validate_port_speed("1", SpeedDuplex::ThousandFull).is_ok());

        // Invalid speed (10G not supported on regular ports)
        assert!(model.validate_port_speed("1", SpeedDuplex::TenGFull).is_err());

        // Invalid port
        assert!(model.validate_port_speed("99", SpeedDuplex::Auto).is_err());
    }

    #[test]
    fn test_switch_model_all_port_capabilities() {
        let model = SwitchModel::Aruba2530_8G_POE;
        let all_caps = model.all_port_capabilities();

        // Should have 8 ports
        assert_eq!(all_caps.len(), 8);

        // Verify odd ports have PoE
        assert!(all_caps[0].supports_poe()); // Port 1
        assert!(!all_caps[1].supports_poe()); // Port 2
        assert!(all_caps[2].supports_poe()); // Port 3
        assert!(!all_caps[3].supports_poe()); // Port 4
    }

    #[test]
    fn test_state_diff_serializable() {
        let diff = StateDiff::default();
        let json = serde_json::to_string(&diff).expect("StateDiff should serialize to JSON");
        assert!(json.contains("vlans_to_add"));
        assert!(json.contains("ports_to_configure"));
        assert!(json.contains("mirrors_to_add"));
        assert!(json.contains("mirror_dest_ports_to_configure"));
        assert!(json.contains("snmp_config_changed"));
    }

    #[test]
    fn test_snmp_state_diff_serializable() {
        let diff = SnmpStateDiff::default();
        let json = serde_json::to_string(&diff).expect("SnmpStateDiff should serialize to JSON");
        assert!(json.contains("communities_to_add"));
        assert!(json.contains("traps_to_enable"));
    }
}
