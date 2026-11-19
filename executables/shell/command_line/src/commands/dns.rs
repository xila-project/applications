use core::fmt::{self, Display};

use crate::Result;
use getargs::Options;
use getargs_derive::GetArgs;
use xila::{
    file_system::Path,
    internationalization::t,
    network::{self, DnsQueryKind},
};

use super::{CommandContext, UserCommand};

pub struct DnsResolveCommand;

impl UserCommand for DnsResolveCommand {
    async fn execute<'a, I, C>(
        &self,
        context: &mut C,
        options: &mut Options<&'a str, I>,
        _paths: &[&Path],
    ) -> Result<(), crate::Error<'a>>
    where
        I: Iterator<Item = &'a str>,
        C: CommandContext,
    {
        execute_dns_resolve(context, options).await
    }
}

#[derive(GetArgs)]
#[command(name = "dns_resolve", about = "Resolve a DNS name.")]
pub(super) struct DnsResolveArguments<'a> {
    domain: &'a str,
    #[arg(short = 'a', long = "a")]
    a_enabled: bool,
    #[arg(short = 'A', long = "aaaa")]
    aaaa_enabled: bool,
    #[arg(short = 'c', long = "cname")]
    cname_enabled: bool,
    #[arg(short = 'n', long = "ns")]
    ns_enabled: bool,
    #[arg(short = 's', long = "soa")]
    soa_enabled: bool,
}

fn format_kind(kind: DnsQueryKind) -> &'static str {
    match kind {
        DnsQueryKind::A => "A",
        DnsQueryKind::Aaaa => "AAAA",
        DnsQueryKind::Cname => "CNAME",
        DnsQueryKind::Ns => "NS",
        DnsQueryKind::Soa => "SOA",
        _ => "UNKNOWN",
    }
}

pub(super) fn resolution_failure<'a>(
    error: network::Error,
    domain: &'a str,
    record_kind: &'a str,
) -> impl Display + 'a {
    fmt::from_fn(move |formatter| match error {
        network::Error::NameTooLong => {
            write!(
                formatter,
                "{}",
                t!("dns.errors.domain_name_too_long", arg0 = domain)
            )
        }
        network::Error::InvalidName => {
            write!(
                formatter,
                "{}",
                t!("dns.errors.invalid_domain_name", arg0 = domain)
            )
        }
        network::Error::NoFreeSlot => {
            write!(
                formatter,
                "{}",
                t!("dns.errors.no_free_query_slot", arg0 = domain)
            )
        }
        network::Error::NotFound => {
            write!(
                formatter,
                "{}",
                t!("dns.errors.no_network_interface", arg0 = domain)
            )
        }
        _ => write!(
            formatter,
            "{}",
            t!(
                "dns.errors.resolution_failed",
                arg0 = record_kind,
                arg1 = domain
            )
        ),
    })
}

async fn resolve_record<C: CommandContext>(
    context: &mut C,
    network_manager: &network::Manager,
    domain: &str,
    kind: DnsQueryKind,
) -> Result<()> {
    match network_manager.resolve(domain, kind, true, None).await {
        Ok(ip) if !ip.is_empty() => {
            let count = ip.len();
            let record_label = xila::internationalization::t!("dns.records.unit", count = count);
            // DNS record types (A, AAAA, etc.) remain invariant CLI identifiers.
            context
                .write_out_fmt(format_args!(
                    "{}: {} {} '{}':\n",
                    format_kind(kind),
                    count,
                    record_label,
                    domain
                ))
                .await?;
            for address in &ip {
                context
                    .write_out_fmt(format_args!(" - {}\n", address))
                    .await?;
            }
        }
        Ok(_) => {
            context
                .write_out_fmt(format_args!(
                    "{}\n",
                    format_args!(
                        "{}",
                        t!(
                            "dns.status.no_records_found",
                            arg0 = format_kind(kind),
                            arg1 = domain
                        )
                    )
                ))
                .await?;
        }
        Err(error) => {
            context
                .write_out_fmt(format_args!(
                    "{}\n",
                    resolution_failure(error, domain, format_kind(kind))
                ))
                .await?;
        }
    }

    Ok(())
}

async fn execute_dns_resolve<'a, I, C>(
    context: &mut C,
    options: &mut Options<&'a str, I>,
) -> Result<(), crate::Error<'a>>
where
    I: Iterator<Item = &'a str>,
    C: CommandContext,
{
    let DnsResolveArguments {
        domain,
        a_enabled,
        aaaa_enabled,
        cname_enabled,
        ns_enabled,
        soa_enabled,
    } = DnsResolveArguments::parse(options)?;

    let default = !a_enabled && !aaaa_enabled && !cname_enabled && !ns_enabled && !soa_enabled;

    let network_manager = context
        .executable_context()
        .network_manager
        .as_ref()
        .ok_or(crate::Error::NetworkUnavailable)?;

    if a_enabled || default {
        resolve_record(context, network_manager, domain, DnsQueryKind::A).await?;
    }
    if aaaa_enabled || default {
        resolve_record(context, network_manager, domain, DnsQueryKind::Aaaa).await?;
    }
    if cname_enabled {
        resolve_record(context, network_manager, domain, DnsQueryKind::Cname).await?;
    }
    if ns_enabled {
        resolve_record(context, network_manager, domain, DnsQueryKind::Ns).await?;
    }
    if soa_enabled {
        resolve_record(context, network_manager, domain, DnsQueryKind::Soa).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{format_kind, resolution_failure};
    use alloc::string::ToString;
    use xila::{
        internationalization::t,
        network::{DnsQueryKind, Error},
    };

    #[test]
    fn resolution_failure_reports_specific_dns_errors() {
        let domain = "example.org";
        for (error, expected) in [
            (
                Error::NameTooLong,
                t!("dns.errors.domain_name_too_long", arg0 = domain).to_string(),
            ),
            (
                Error::InvalidName,
                t!("dns.errors.invalid_domain_name", arg0 = domain).to_string(),
            ),
            (
                Error::NoFreeSlot,
                t!("dns.errors.no_free_query_slot", arg0 = domain).to_string(),
            ),
            (
                Error::NotFound,
                t!("dns.errors.no_network_interface", arg0 = domain).to_string(),
            ),
        ] {
            assert_eq!(resolution_failure(error, domain, "A").to_string(), expected);
        }
    }

    #[test]
    fn resolution_failure_falls_back_to_domain_and_record_kind() {
        let domain = "example.org";
        let expected = t!("dns.errors.resolution_failed", arg0 = "AAAA", arg1 = domain).to_string();
        for error in [Error::Failed, Error::TimedOut] {
            assert_eq!(
                resolution_failure(error, domain, "AAAA").to_string(),
                expected
            );
        }
    }

    #[test]
    fn dns_record_count_selects_singular_and_plural() {
        let (one, other) = match xila::internationalization::locale!() {
            "fr" => (
                "enregistrement pour le domaine",
                "enregistrements pour le domaine",
            ),
            "zh-CN" => ("条记录，域名", "条记录，域名"),
            "ja" => ("ドメインのレコード", "ドメインのレコード"),
            _ => ("record for domain", "records for domain"),
        };
        assert_eq!(
            xila::internationalization::t!("dns.records.unit", count = 1usize).to_string(),
            one
        );
        assert_eq!(
            xila::internationalization::t!("dns.records.unit", count = 2usize).to_string(),
            other
        );
    }

    #[test]
    fn format_kind_maps_a() {
        assert_eq!(format_kind(DnsQueryKind::A), "A");
    }

    #[test]
    fn format_kind_maps_aaaa() {
        assert_eq!(format_kind(DnsQueryKind::Aaaa), "AAAA");
    }

    #[test]
    fn format_kind_maps_cname() {
        assert_eq!(format_kind(DnsQueryKind::Cname), "CNAME");
    }

    #[test]
    fn format_kind_maps_ns() {
        assert_eq!(format_kind(DnsQueryKind::Ns), "NS");
    }

    #[test]
    fn format_kind_maps_soa() {
        assert_eq!(format_kind(DnsQueryKind::Soa), "SOA");
    }
}
