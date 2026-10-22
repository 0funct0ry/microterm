//! The real serial backend over the `serialport` crate.

use std::io::{self, Read, Write};
use std::time::Duration;

use crate::link::{OpenFailure, Opener, SerialIo};
use crate::opts::{DataBits, Flow, Parity, SerialOpts, StopBits};

/// Read timeout: bounds how long a queued keystroke waits behind a read.
const READ_TIMEOUT: Duration = Duration::from_millis(15);
const BREAK_DURATION: Duration = Duration::from_millis(250);

pub struct SystemOpener;

impl Opener for SystemOpener {
    fn open(&self, o: &SerialOpts) -> Result<Box<dyn SerialIo>, OpenFailure> {
        let port = serialport::new(&o.port, o.baud)
            .data_bits(match o.data_bits {
                DataBits::Five => serialport::DataBits::Five,
                DataBits::Six => serialport::DataBits::Six,
                DataBits::Seven => serialport::DataBits::Seven,
                DataBits::Eight => serialport::DataBits::Eight,
            })
            .parity(match o.parity {
                Parity::None => serialport::Parity::None,
                Parity::Odd => serialport::Parity::Odd,
                Parity::Even => serialport::Parity::Even,
            })
            .stop_bits(match o.stop_bits {
                StopBits::One => serialport::StopBits::One,
                StopBits::Two => serialport::StopBits::Two,
            })
            .flow_control(match o.flow {
                Flow::None => serialport::FlowControl::None,
                Flow::Software => serialport::FlowControl::Software,
                Flow::Hardware => serialport::FlowControl::Hardware,
            })
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(classify)?;
        Ok(Box::new(SystemPort { port }))
    }
}

fn classify(e: serialport::Error) -> OpenFailure {
    use serialport::ErrorKind;
    let msg = e.to_string();
    match e.kind() {
        // The device is not there (yet): wait for it to enumerate.
        ErrorKind::NoDevice => OpenFailure::Transient(msg),
        ErrorKind::Io(io::ErrorKind::NotFound) => OpenFailure::Transient(msg),
        // Retrying cannot help until the user acts.
        ErrorKind::InvalidInput => OpenFailure::Fatal(msg),
        ErrorKind::Io(io::ErrorKind::PermissionDenied) => OpenFailure::Fatal(msg),
        _ => OpenFailure::Transient(msg),
    }
}

struct SystemPort {
    port: Box<dyn serialport::SerialPort>,
}

fn to_io(e: serialport::Error) -> io::Error {
    io::Error::other(e.to_string())
}

impl SerialIo for SystemPort {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.port.read(buf)
    }
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.port.write_all(bytes)?;
        self.port.flush()
    }
    fn send_break(&mut self) -> io::Result<()> {
        self.port.set_break().map_err(to_io)?;
        std::thread::sleep(BREAK_DURATION);
        self.port.clear_break().map_err(to_io)
    }
    fn set_dtr(&mut self, on: bool) -> io::Result<()> {
        self.port.write_data_terminal_ready(on).map_err(to_io)
    }
    fn set_rts(&mut self, on: bool) -> io::Result<()> {
        self.port.write_request_to_send(on).map_err(to_io)
    }
}
