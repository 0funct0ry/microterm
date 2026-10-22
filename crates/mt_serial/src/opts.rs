//! Connection options and their text forms (CLI flags, `[[serial_ports]]`, domain refs).

use std::fmt;
use std::str::FromStr;

pub const DEFAULT_BAUD: u32 = 115_200;

/// Prefix of an ad-hoc serial domain name (`microterm serial PORT`).
pub const DOMAIN_PREFIX: &str = "serial:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptsError(pub String);

impl fmt::Display for OptsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OptsError {}

macro_rules! text_enum {
    ($name:ident, $what:literal, { $($variant:ident => $text:literal $(| $alt:literal)*),+ $(,)? }) => {
        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }
        }
        impl FromStr for $name {
            type Err = OptsError;
            fn from_str(s: &str) -> Result<Self, OptsError> {
                match s.trim().to_ascii_lowercase().as_str() {
                    $($text $(| $alt)* => Ok($name::$variant),)+
                    other => Err(OptsError(format!(concat!("invalid ", $what, " \"{}\""), other))),
                }
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.as_str()) }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataBits {
    Five,
    Six,
    Seven,
    #[default]
    Eight,
}
text_enum!(DataBits, "data bits", { Five => "5", Six => "6", Seven => "7", Eight => "8" });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
}
text_enum!(Parity, "parity", { None => "none" | "n", Odd => "odd" | "o", Even => "even" | "e" });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StopBits {
    #[default]
    One,
    Two,
}
text_enum!(StopBits, "stop bits", { One => "1", Two => "2" });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Flow {
    #[default]
    None,
    /// XON/XOFF.
    Software,
    /// RTS/CTS.
    Hardware,
}
text_enum!(Flow, "flow control", {
    None => "none",
    Software => "software" | "xonxoff",
    Hardware => "hardware" | "rtscts"
});

/// What Enter (and any newline in pasted text) is sent as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// `\r`, what a terminal sends for Enter.
    #[default]
    Cr,
    Lf,
    Crlf,
}
text_enum!(LineEnding, "line ending", { Cr => "cr", Lf => "lf", Crlf => "crlf" });

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialOpts {
    pub port: String,
    pub baud: u32,
    pub data_bits: DataBits,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub flow: Flow,
    pub local_echo: bool,
    pub line_ending: LineEnding,
}

impl SerialOpts {
    pub fn new(port: impl Into<String>) -> Self {
        SerialOpts {
            port: port.into(),
            baud: DEFAULT_BAUD,
            data_bits: DataBits::default(),
            parity: Parity::default(),
            stop_bits: StopBits::default(),
            flow: Flow::default(),
            local_echo: false,
            line_ending: LineEnding::default(),
        }
    }

    /// `115200 8N1`, the usual shorthand.
    pub fn framing(&self) -> String {
        let p = match self.parity {
            Parity::None => 'N',
            Parity::Odd => 'O',
            Parity::Even => 'E',
        };
        format!("{} {}{}{}", self.baud, self.data_bits, p, self.stop_bits)
    }

    /// `ttyUSB0 @ 115200`: what a tab shows.
    pub fn label(&self) -> String {
        format!("{} @ {}", short_port(&self.port), self.baud)
    }

    /// Parses `serial:PORT[?baud=..&data_bits=..&parity=..&stop_bits=..&flow=..&echo=..&eol=..]`.
    pub fn from_domain_ref(s: &str) -> Result<Self, OptsError> {
        let rest = s
            .strip_prefix(DOMAIN_PREFIX)
            .ok_or_else(|| OptsError(format!("not a serial reference: \"{s}\"")))?;
        let (port, query) = match rest.split_once('?') {
            Some((p, q)) => (p, q),
            None => (rest, ""),
        };
        if port.is_empty() {
            return Err(OptsError("missing serial port".into()));
        }
        let mut o = SerialOpts::new(port);
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match k {
                "baud" => o.baud = parse_baud(v)?,
                "data_bits" => o.data_bits = v.parse()?,
                "parity" => o.parity = v.parse()?,
                "stop_bits" => o.stop_bits = v.parse()?,
                "flow" => o.flow = v.parse()?,
                "echo" => o.local_echo = v == "1" || v == "true",
                "eol" => o.line_ending = v.parse()?,
                other => return Err(OptsError(format!("unknown serial option \"{other}\""))),
            }
        }
        Ok(o)
    }

    /// Inverse of [`from_domain_ref`](Self::from_domain_ref); omits defaults for readability.
    pub fn to_domain_ref(&self) -> String {
        let d = SerialOpts::new(&self.port);
        let mut q = Vec::new();
        if self.baud != d.baud {
            q.push(format!("baud={}", self.baud));
        }
        if self.data_bits != d.data_bits {
            q.push(format!("data_bits={}", self.data_bits));
        }
        if self.parity != d.parity {
            q.push(format!("parity={}", self.parity));
        }
        if self.stop_bits != d.stop_bits {
            q.push(format!("stop_bits={}", self.stop_bits));
        }
        if self.flow != d.flow {
            q.push(format!("flow={}", self.flow));
        }
        if self.local_echo {
            q.push("echo=1".into());
        }
        if self.line_ending != d.line_ending {
            q.push(format!("eol={}", self.line_ending));
        }
        if q.is_empty() {
            format!("{DOMAIN_PREFIX}{}", self.port)
        } else {
            format!("{DOMAIN_PREFIX}{}?{}", self.port, q.join("&"))
        }
    }
}

pub fn parse_baud(s: &str) -> Result<u32, OptsError> {
    match s.trim().parse::<u32>() {
        Ok(b) if b > 0 => Ok(b),
        _ => Err(OptsError(format!("invalid baud rate \"{s}\""))),
    }
}

/// `/dev/ttyUSB0` → `ttyUSB0`.
pub fn short_port(port: &str) -> &str {
    port.rsplit('/').next().unwrap_or(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_parse_case_insensitively_and_reject_junk() {
        assert_eq!("8".parse::<DataBits>().unwrap(), DataBits::Eight);
        assert_eq!("EVEN".parse::<Parity>().unwrap(), Parity::Even);
        assert_eq!("n".parse::<Parity>().unwrap(), Parity::None);
        assert_eq!("2".parse::<StopBits>().unwrap(), StopBits::Two);
        assert_eq!("rtscts".parse::<Flow>().unwrap(), Flow::Hardware);
        assert_eq!("CRLF".parse::<LineEnding>().unwrap(), LineEnding::Crlf);
        assert!("9".parse::<DataBits>().is_err());
        assert!("mark".parse::<Parity>().is_err());
        assert!("3".parse::<StopBits>().is_err());
    }

    #[test]
    fn baud_must_be_positive_integer() {
        assert_eq!(parse_baud("9600"), Ok(9600));
        assert!(parse_baud("0").is_err());
        assert!(parse_baud("fast").is_err());
    }

    #[test]
    fn domain_ref_round_trips() {
        let mut o = SerialOpts::new("/dev/ttyUSB0");
        assert_eq!(o.to_domain_ref(), "serial:/dev/ttyUSB0");
        o.baud = 9600;
        o.parity = Parity::Even;
        o.flow = Flow::Hardware;
        o.local_echo = true;
        o.line_ending = LineEnding::Crlf;
        let r = o.to_domain_ref();
        assert_eq!(SerialOpts::from_domain_ref(&r).unwrap(), o);
    }

    #[test]
    fn domain_ref_errors() {
        assert!(SerialOpts::from_domain_ref("ssh:host").is_err());
        assert!(SerialOpts::from_domain_ref("serial:").is_err());
        assert!(SerialOpts::from_domain_ref("serial:/dev/x?baud=abc").is_err());
        assert!(SerialOpts::from_domain_ref("serial:/dev/x?bogus=1").is_err());
    }

    #[test]
    fn labels() {
        let o = SerialOpts::new("/dev/ttyUSB0");
        assert_eq!(o.label(), "ttyUSB0 @ 115200");
        assert_eq!(o.framing(), "115200 8N1");
    }
}
