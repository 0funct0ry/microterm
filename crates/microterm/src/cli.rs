//! Command-line surface (SPEC §12.1). The clap definitions live in `mt_cli`; the helpers that
//! need `mt_serial` stay here.

pub use mt_cli::cli::*;

/// The domain string the app resolves for `microterm serial`: a configured name or
/// `serial:PORT?...`.
pub fn serial_domain_ref(args: &SerialArgs) -> Result<String, String> {
    if let Some(d) = &args.domain {
        return Ok(d.clone());
    }
    let port = args.port.clone().unwrap_or_default();
    let mut o = mt_serial::SerialOpts::new(port);
    o.baud = mt_serial::opts::parse_baud(&args.baud.to_string()).map_err(|e| e.to_string())?;
    o.data_bits = args
        .data_bits
        .parse()
        .map_err(|e: mt_serial::OptsError| e.to_string())?;
    o.parity = args
        .parity
        .parse()
        .map_err(|e: mt_serial::OptsError| e.to_string())?;
    o.stop_bits = args
        .stop_bits
        .parse()
        .map_err(|e: mt_serial::OptsError| e.to_string())?;
    o.flow = args
        .flow
        .parse()
        .map_err(|e: mt_serial::OptsError| e.to_string())?;
    o.line_ending = args
        .line_ending
        .parse()
        .map_err(|e: mt_serial::OptsError| e.to_string())?;
    o.local_echo = args.local_echo;
    Ok(o.to_domain_ref())
}

/// `microterm serial --list`: one line per port.
pub fn print_serial_ports() {
    let ports = mt_serial::discover();
    if ports.is_empty() {
        println!("no serial ports found");
    }
    for p in ports {
        let d = p.detail();
        if d.is_empty() {
            println!("{}", p.path);
        } else {
            println!("{:<28} {d}", p.path);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_baud_matches_mt_serial() {
        assert_eq!(mt_cli::DEFAULT_BAUD, mt_serial::opts::DEFAULT_BAUD);
    }
}
