---
title: Serial ports
description: Talk to Arduinos, routers and other devices on a serial line from a tab.
---

A serial port opens in a tab like any other domain. The tab behaves as a terminal wired to the
device: what the device sends is drawn, what you type is sent. Unlike SSH there is no daemon:
the port is opened by microterm itself, and closing the tab closes the port.

## Open a port

```sh
microterm serial /dev/ttyUSB0 --baud 115200
microterm serial /dev/cu.usbmodem1101 --baud 9600 --parity even --flow hardware
microterm serial --list        # ports with USB VID:PID and product names
```

| Flag | Values | Default |
|---|---|---|
| `--baud` | any positive rate | `115200` |
| `--data-bits` | `5` `6` `7` `8` | `8` |
| `--parity` | `none` `odd` `even` | `none` |
| `--stop-bits` | `1` `2` | `1` |
| `--flow` | `none` `software` (XON/XOFF) `hardware` (RTS/CTS) | `none` |
| `--local-echo` | show typed characters locally | off |
| `--line-ending` | what Enter is sent as: `cr` `lf` `crlf` | `cr` |

Invalid values exit with status 1 before any window opens. On macOS use the `cu.*` device
(`tty.*` twins are hidden from discovery).

## Configure ports

```toml
[[serial_ports]]
name = "uno"
port = "/dev/ttyACM0"
baud = 9600
data_bits = "8"        # optional: "5".."8"
parity = "none"        # optional: "none" | "odd" | "even"
stop_bits = "1"        # optional: "1" | "2"
flow = "none"          # optional: "none" | "software" | "hardware"
local_echo = false
line_ending = "cr"     # "cr" | "lf" | "crlf"
color = "teal"         # optional tab color
```

Configured ports appear in the [launcher](/docs/command-palette-and-launcher/) as
**New tab in uno**, in the menu bar under *New Tab in Domain*, and as
`microterm ctl tab new --domain uno`. Values are checked when the config loads. A domain is read
when its tab first opens; changing it does not disturb a tab that is already open.

## Connect to serial port…

The launcher row **Connect to serial port…** (also the `ConnectSerial` action) lists the
discovered ports with their product name and `VID:PID`. Choosing one opens it with the
`[[serial_ports]]` entry for that device when there is one, else with `115200 8N1`.
**Other port…** asks for `PORT [BAUD]`, which is the way to open a pty such as one made by
`socat -d -d pty,raw,echo=0 pty,raw,echo=0`.

## Line endings and echo

Terminals send `\r` for Enter; many devices want `\n` or `\r\n`. `line_ending` rewrites every
line break you type or paste (`\r`, `\n`, `\r\n`) to the chosen ending. Devices that do not echo
what they receive can be given `local_echo`, which draws your typing locally (Enter starts a new
line, Backspace erases).

## Modem lines and break

| Action | Effect |
|---|---|
| `SerialToggleDtr` | Flip DTR. Most Arduino boards reset when DTR drops and rises again, so run it twice. A toast shows the new DTR/RTS state. |
| `SerialToggleRts` | Flip RTS. |
| `SerialSendBreak` | Send a 250 ms break. |

None is bound by default; bind them in `[[keys]]` or run them from the command palette. Ports
open with both lines asserted, which is what the toggles assume.

## Unplugging

When the device disappears (USB unplug, board reset that re-enumerates) the tab keeps its last
screen, dims it and shows a card: "Serial port … disconnected", with the attempt count and the
time to the next try. Keyboard input is blocked. microterm reopens the same path with the
backoff 1, 2, 4, 8, 16, 30 seconds (±20 %); **Retry now** skips the wait, **Close tab** closes the
port. When the device is back the tab continues on the same screen with a dim
"[serial port reconnected]" line.

If the port cannot be opened for a reason retrying will not fix (for example permission denied),
the card says so and waits for **Retry**. On Linux add yourself to the `dialout` (or `uucp`)
group.

## Limits

- One link per port: opening a port twice, or splitting a serial tab, shares one connection, and
  the settings of the first open win.
- There is no scrollback replay across a reconnect other than the screen you already have.
- The terminal size is not sent to the device (a serial line has no window size).
