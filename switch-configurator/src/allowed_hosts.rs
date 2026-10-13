//! Managers allowed to query an SNMP community: resolving `auto`, and
//! converting to/from the address + netmask form switch CLIs use.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

use anyhow::{anyhow, bail, Context, Result};

use crate::models::{AllowedHost, SwitchConfig};

/// The local address this host would use to reach `target` (the switch's
/// management IP), i.e. the address the switch sees our queries come from.
/// Asks the routing table: a UDP "connect" sends nothing.
pub fn local_address_towards(target: &str) -> Result<IpAddr> {
    let ip: IpAddr = target
        .parse()
        .map_err(|_| anyhow!("'{}' is not an IP address", target))?;
    let bind = if ip.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let socket = UdpSocket::bind(bind).context("opening a UDP socket to look up the local address")?;
    socket
        .connect((ip, 161))
        .with_context(|| format!("no route to {}", target))?;
    Ok(socket.local_addr()?.ip())
}

/// Replace every `auto` in the switch's SNMP communities with the local
/// address towards its management IP, as a plain network entry. The service's
/// own read community (`snmp.reads`, when on) with no `allowed_hosts` at all
/// is restricted to this host by default.
pub fn resolve_auto_hosts(switch: &mut SwitchConfig) -> Result<()> {
    use crate::models::SnmpReadsMode;

    let reads_default = |snmp: &crate::models::SnmpConfig| {
        snmp.reads.as_ref().is_some_and(|r| {
            r.mode == SnmpReadsMode::Auto && r.community.as_ref().is_some_and(|c| c.allowed_hosts.is_empty())
        })
    };
    let needs_resolution = switch.snmp.as_ref().is_some_and(|snmp| {
        snmp.communities.iter().any(|c| c.allowed_hosts.contains(&AllowedHost::Auto))
            || snmp
                .reads
                .as_ref()
                .and_then(|r| r.community.as_ref())
                .is_some_and(|c| c.allowed_hosts.contains(&AllowedHost::Auto))
            || reads_default(snmp)
    });
    if !needs_resolution {
        return Ok(());
    }

    let management_ip = switch.management_ip.clone().unwrap_or_default();
    let local = local_address_towards(&management_ip).with_context(|| {
        format!(
            "switch '{}': cannot resolve SNMP allowed_hosts 'auto' (the address this host reaches {} from)",
            switch.id, management_ip
        )
    })?;
    let here = AllowedHost::Network(local.to_string());

    if let Some(snmp) = switch.snmp.as_mut() {
        let default_reads = reads_default(snmp);
        for community in &mut snmp.communities {
            for host in &mut community.allowed_hosts {
                if *host == AllowedHost::Auto {
                    *host = here.clone();
                }
            }
        }
        if let Some(reads) = snmp.reads.as_mut().and_then(|r| r.community.as_mut()) {
            if default_reads {
                reads.allowed_hosts = vec![here.clone()];
            }
            for host in &mut reads.allowed_hosts {
                if *host == AllowedHost::Auto {
                    *host = here.clone();
                }
            }
        }
    }
    Ok(())
}

/// `allowed_hosts` only does anything on vendors that implement it. Rather
/// than accept the setting and silently not restrict anything, refuse it on
/// the others (until their SNMP restriction is verified on hardware).
pub fn check_vendor_support(switch: &SwitchConfig) -> Result<()> {
    // An overlay is partial and may not name a model; the merged switch is
    // checked in full when the config is loaded.
    let Some(model) = switch.model.as_ref() else { return Ok(()) };
    let vendor = model.vendor();
    if vendor == crate::models::Vendor::Fortiswitch {
        return Ok(());
    }
    if crate::snmp_reads::reads_enabled(switch) {
        bail!(
            "switch '{}': snmp.reads is not supported for {:?} switches yet (FortiSwitch only; see docs/learnings/snmp-aruba-cisco.md)",
            switch.id,
            vendor
        );
    }
    let restricted = switch
        .snmp
        .as_ref()
        .is_some_and(|snmp| snmp.communities.iter().any(|c| !c.allowed_hosts.is_empty()));
    if restricted {
        bail!(
            "switch '{}': SNMP allowed_hosts is not supported for {:?} switches yet (FortiSwitch only; see docs/learnings/snmp-aruba-cisco.md); remove it, or the community would be unrestricted",
            switch.id,
            vendor
        );
    }
    Ok(())
}

/// `(address, netmask)` as FortiOS's `config hosts` / `set ip <addr> <mask>` wants it.
pub fn host_to_fortios(host: &AllowedHost) -> Result<(String, String)> {
    let network = match host {
        AllowedHost::Auto => bail!("allowed_hosts 'auto' was not resolved to an address"),
        AllowedHost::Network(network) => network,
    };
    let (addr, prefix) = match network.split_once('/') {
        Some((addr, prefix)) => (addr, Some(prefix)),
        None => (network.as_str(), None),
    };
    let ip: Ipv4Addr = match addr.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip,
        Ok(IpAddr::V6(_)) => bail!("IPv6 manager '{}' is not supported on FortiSwitch yet (only IPv4)", network),
        Err(_) => bail!("'{}' is not an IP address or network", network),
    };
    let prefix: u32 = match prefix {
        Some(p) => p.parse().map_err(|_| anyhow!("invalid prefix length in '{}'", network))?,
        None => 32,
    };
    if prefix > 32 {
        bail!("invalid prefix length in '{}'", network);
    }
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
    Ok((ip.to_string(), Ipv4Addr::from(mask).to_string()))
}

/// Inverse of `host_to_fortios`; `None` for a mask that isn't a contiguous prefix.
pub fn host_from_fortios(ip: &str, mask: &str) -> Option<AllowedHost> {
    let ip: Ipv4Addr = ip.parse().ok()?;
    let mask: u32 = u32::from(mask.parse::<Ipv4Addr>().ok()?);
    let prefix = mask.count_ones();
    // Contiguous means: ones followed only by zeros.
    if prefix != 0 && mask != u32::MAX << (32 - prefix) {
        return None;
    }
    if prefix == 0 && mask != 0 {
        return None;
    }
    Some(AllowedHost::Network(if prefix == 32 { ip.to_string() } else { format!("{}/{}", ip, prefix) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConnectionType, Credentials, SnmpAccess, SnmpCommunity, SnmpConfig, SwitchModel};

    fn switch(management_ip: &str, communities: Vec<SnmpCommunity>) -> SwitchConfig {
        SwitchConfig {
            id: "sw1".to_string(),
            hostname: Some("sw1".to_string()),
            model: Some(SwitchModel::Fortiswitch124F_FPOE),
            management_ip: Some(management_ip.to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("pw".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 115200,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: Some(SnmpConfig { communities, trap_receivers: vec![], enabled_traps: vec![], reads: None }),
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        }
    }

    #[test]
    fn local_address_towards_loopback_is_loopback() {
        assert_eq!(local_address_towards("127.0.0.1").unwrap(), "127.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn local_address_towards_rejects_a_non_ip_target() {
        assert!(local_address_towards("not-an-ip").is_err());
    }

    #[test]
    fn auto_is_replaced_by_the_local_address_and_other_entries_are_kept() {
        let mut community = SnmpCommunity::new("c", SnmpAccess::Operator);
        community.allowed_hosts = vec![AllowedHost::Auto, AllowedHost::Network("192.0.2.0/24".to_string())];
        let mut sw = switch("127.0.0.1", vec![community]);

        resolve_auto_hosts(&mut sw).unwrap();

        let hosts = &sw.snmp.as_ref().unwrap().communities[0].allowed_hosts;
        assert_eq!(hosts[0], AllowedHost::Network("127.0.0.1".to_string()));
        assert_eq!(hosts[1], AllowedHost::Network("192.0.2.0/24".to_string()));
    }

    #[test]
    fn resolving_without_any_auto_does_not_need_a_routable_management_ip() {
        let mut community = SnmpCommunity::new("c", SnmpAccess::Operator);
        community.allowed_hosts = vec![AllowedHost::Network("192.0.2.1".to_string())];
        let mut sw = switch("not-an-ip", vec![community]);
        assert!(resolve_auto_hosts(&mut sw).is_ok());
    }

    #[test]
    fn auto_with_an_unusable_management_ip_is_an_error_naming_the_switch() {
        let mut community = SnmpCommunity::new("c", SnmpAccess::Operator);
        community.allowed_hosts = vec![AllowedHost::Auto];
        let err = resolve_auto_hosts(&mut switch("not-an-ip", vec![community])).unwrap_err().to_string();
        assert!(err.contains("sw1") && err.contains("auto"), "unhelpful error: {err}");
    }

    #[test]
    fn host_to_fortios_uses_address_and_netmask() {
        let net = |s: &str| AllowedHost::try_from(s.to_string()).unwrap();
        assert_eq!(host_to_fortios(&net("192.0.2.1")).unwrap(), ("192.0.2.1".to_string(), "255.255.255.255".to_string()));
        assert_eq!(host_to_fortios(&net("192.0.2.1/32")).unwrap(), ("192.0.2.1".to_string(), "255.255.255.255".to_string()));
        assert_eq!(host_to_fortios(&net("192.0.2.0/24")).unwrap(), ("192.0.2.0".to_string(), "255.255.255.0".to_string()));
        assert_eq!(host_to_fortios(&net("10.0.0.0/8")).unwrap(), ("10.0.0.0".to_string(), "255.0.0.0".to_string()));
        assert_eq!(host_to_fortios(&net("0.0.0.0/0")).unwrap(), ("0.0.0.0".to_string(), "0.0.0.0".to_string()));
    }

    #[test]
    fn host_to_fortios_rejects_ipv6_and_unresolved_auto() {
        let v6 = AllowedHost::try_from("2001:db8::1".to_string()).unwrap();
        assert!(host_to_fortios(&v6).unwrap_err().to_string().contains("IPv6"));
        assert!(host_to_fortios(&AllowedHost::Auto).unwrap_err().to_string().contains("auto"));
    }

    #[test]
    fn host_from_fortios_is_the_inverse_and_canonical() {
        assert_eq!(host_from_fortios("192.0.2.1", "255.255.255.255"), Some(AllowedHost::Network("192.0.2.1".to_string())));
        assert_eq!(host_from_fortios("192.0.2.0", "255.255.255.0"), Some(AllowedHost::Network("192.0.2.0/24".to_string())));
        assert_eq!(host_from_fortios("192.0.2.0", "255.0.255.0"), None, "non-contiguous mask");
        assert_eq!(host_from_fortios("garbage", "255.255.255.0"), None);
    }

    #[test]
    fn a_slash_32_is_canonicalised_to_the_plain_address() {
        assert_eq!(
            AllowedHost::try_from("192.0.2.1/32".to_string()).unwrap(),
            AllowedHost::Network("192.0.2.1".to_string())
        );
        assert_eq!(
            AllowedHost::try_from("2001:db8::1/128".to_string()).unwrap(),
            AllowedHost::Network("2001:db8::1".to_string())
        );
    }

    #[test]
    fn allowed_hosts_are_refused_on_vendors_that_do_not_implement_them() {
        let mut community = SnmpCommunity::new("c", SnmpAccess::Operator);
        community.allowed_hosts = vec![AllowedHost::Auto];
        for model in [SwitchModel::Aruba2930F, SwitchModel::CiscoCatalyst9300_24P_UPOE] {
            let mut sw = switch("192.0.2.1", vec![community.clone()]);
            sw.model = Some(model);
            let err = check_vendor_support(&sw).unwrap_err().to_string();
            assert!(err.contains("sw1") && err.contains("allowed_hosts") && err.contains("FortiSwitch"), "unhelpful error: {err}");
        }
    }

    #[test]
    fn allowed_hosts_are_accepted_on_fortiswitch_and_unrestricted_communities_everywhere() {
        let mut restricted = SnmpCommunity::new("c", SnmpAccess::Operator);
        restricted.allowed_hosts = vec![AllowedHost::Auto];
        assert!(check_vendor_support(&switch("192.0.2.1", vec![restricted])).is_ok());

        let mut aruba = switch("192.0.2.1", vec![SnmpCommunity::new("c", SnmpAccess::Operator)]);
        aruba.model = Some(SwitchModel::Aruba2930F);
        assert!(check_vendor_support(&aruba).is_ok(), "no allowed_hosts, nothing to refuse");
    }

    #[test]
    fn a_partial_overlay_without_a_model_is_not_judged() {
        let mut community = SnmpCommunity::new("c", SnmpAccess::Operator);
        community.allowed_hosts = vec![AllowedHost::Auto];
        let mut overlay = switch("192.0.2.1", vec![community]);
        overlay.model = None;
        assert!(check_vendor_support(&overlay).is_ok());
    }

    fn reads_switch(model: SwitchModel, hosts: Vec<AllowedHost>) -> SwitchConfig {
        let mut community = SnmpCommunity::new("svc", SnmpAccess::Operator);
        community.allowed_hosts = hosts;
        let mut sw = switch("127.0.0.1", vec![]);
        sw.model = Some(model);
        sw.snmp.as_mut().unwrap().reads = Some(crate::models::SnmpReads {
            mode: crate::models::SnmpReadsMode::Auto,
            community: Some(community),
        });
        sw
    }

    #[test]
    fn the_reads_community_defaults_to_this_host_only() {
        let mut sw = reads_switch(SwitchModel::Fortiswitch124F_FPOE, vec![]);
        resolve_auto_hosts(&mut sw).unwrap();
        let reads = sw.snmp.unwrap().reads.unwrap().community.unwrap();
        assert_eq!(reads.allowed_hosts, vec![AllowedHost::Network("127.0.0.1".to_string())]);
    }

    #[test]
    fn explicit_hosts_on_the_reads_community_are_kept_and_auto_is_resolved() {
        let mut sw = reads_switch(
            SwitchModel::Fortiswitch124F_FPOE,
            vec![AllowedHost::Auto, AllowedHost::Network("192.0.2.0/24".to_string())],
        );
        resolve_auto_hosts(&mut sw).unwrap();
        let hosts = sw.snmp.unwrap().reads.unwrap().community.unwrap().allowed_hosts;
        assert_eq!(hosts, vec![AllowedHost::Network("127.0.0.1".to_string()), AllowedHost::Network("192.0.2.0/24".to_string())]);
    }

    #[test]
    fn a_disabled_reads_block_gets_no_default_hosts() {
        let mut sw = reads_switch(SwitchModel::Fortiswitch124F_FPOE, vec![]);
        sw.snmp.as_mut().unwrap().reads.as_mut().unwrap().mode = crate::models::SnmpReadsMode::Off;
        resolve_auto_hosts(&mut sw).unwrap();
        assert!(sw.snmp.unwrap().reads.unwrap().community.unwrap().allowed_hosts.is_empty());
    }

    #[test]
    fn snmp_reads_are_refused_on_vendors_that_do_not_support_them_yet() {
        let sw = reads_switch(SwitchModel::Aruba2930F, vec![]);
        let err = check_vendor_support(&sw).unwrap_err().to_string();
        assert!(err.contains("sw1") && err.contains("snmp.reads") && err.contains("FortiSwitch"), "unhelpful error: {err}");
        assert!(check_vendor_support(&reads_switch(SwitchModel::Fortiswitch124F_FPOE, vec![])).is_ok());
    }
}
