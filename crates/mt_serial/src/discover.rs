//! Port discovery with USB identification (SPEC §17.3).

use serialport::{SerialPortInfo, SerialPortType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// Device path, e.g. `/dev/ttyUSB0` or `/dev/cu.usbmodem1101`.
    pub path: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
}

impl PortInfo {
    /// `2341:0043`, when the port is a USB device.
    pub fn vid_pid(&self) -> Option<String> {
        Some(format!("{:04x}:{:04x}", self.vid?, self.pid?))
    }

    /// Picker detail: product (or manufacturer) and VID:PID, empty for non-USB ports.
    pub fn detail(&self) -> String {
        let name = match (&self.manufacturer, &self.product) {
            (Some(m), Some(p)) if !p.contains(m.as_str()) => format!("{m} {p}"),
            (_, Some(p)) => p.clone(),
            (Some(m), None) => m.clone(),
            (None, None) => String::new(),
        };
        match (name.is_empty(), self.vid_pid()) {
            (false, Some(id)) => format!("{name} ({id})"),
            (true, Some(id)) => id,
            (false, None) => name,
            (true, None) => String::new(),
        }
    }
}

pub fn from_info(info: SerialPortInfo) -> PortInfo {
    match info.port_type {
        SerialPortType::UsbPort(u) => PortInfo {
            path: info.port_name,
            vid: Some(u.vid),
            pid: Some(u.pid),
            manufacturer: u.manufacturer.filter(|s| !s.is_empty()),
            product: u.product.filter(|s| !s.is_empty()),
            serial_number: u.serial_number.filter(|s| !s.is_empty()),
        },
        _ => PortInfo {
            path: info.port_name,
            vid: None,
            pid: None,
            manufacturer: None,
            product: None,
            serial_number: None,
        },
    }
}

/// Lists the serial ports on this machine, USB devices first. macOS `tty.*` twins of `cu.*`
/// are hidden: `cu.*` is the one to open for an outgoing connection.
pub fn discover() -> Vec<PortInfo> {
    let mut v: Vec<PortInfo> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(from_info)
        .filter(|p| !p.path.starts_with("/dev/tty.") || cfg!(not(target_os = "macos")))
        .collect();
    v.sort_by(|a, b| {
        b.vid
            .is_some()
            .cmp(&a.vid.is_some())
            .then_with(|| a.path.cmp(&b.path))
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::UsbPortInfo;

    fn usb(m: Option<&str>, p: Option<&str>) -> SerialPortInfo {
        SerialPortInfo {
            port_name: "/dev/ttyACM0".into(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid: 0x2341,
                pid: 0x43,
                serial_number: None,
                manufacturer: m.map(Into::into),
                product: p.map(Into::into),
            }),
        }
    }

    #[test]
    fn usb_detail_has_name_and_ids() {
        let p = from_info(usb(Some("Arduino"), Some("Uno R3")));
        assert_eq!(p.vid_pid().as_deref(), Some("2341:0043"));
        assert_eq!(p.detail(), "Arduino Uno R3 (2341:0043)");
        assert_eq!(from_info(usb(None, None)).detail(), "2341:0043");
        // The manufacturer is not repeated when the product already carries it.
        assert_eq!(
            from_info(usb(Some("Arduino"), Some("Arduino Uno"))).detail(),
            "Arduino Uno (2341:0043)"
        );
    }

    #[test]
    fn non_usb_ports_have_no_detail() {
        let p = from_info(SerialPortInfo {
            port_name: "/dev/ttyS0".into(),
            port_type: SerialPortType::Unknown,
        });
        assert_eq!((p.vid, p.detail().as_str()), (None, ""));
    }
}
