#!/usr/bin/env bash
# Prints every SGR style microterm renders, for eyeballing (docs/demos/M2.md).
e=$'\033'
row() { printf '%s%s%s\n' "$e[$1m" "$2" "$e[0m"; }

echo "== attributes"
row 1 "bold"; row 2 "dim"; row 3 "italic"; row 1\;3 "bold italic"
row 7 "reverse"; row 8 "hidden (you should see nothing between the brackets): [hidden]"; row 9 "strikethrough"
echo "== underlines"
row 4 "single (4)"; row 4:1 "single (4:1)"; row 4:2 "double (4:2)"; row 4:3 "curly (4:3)"
row 4:4 "dotted (4:4)"; row 4:5 "dashed (4:5)"
printf '%s curly in red %s\n' "$e[4:3;58;2;255;60;60m" "$e[0m"
printf '%s double in cyan %s\n' "$e[4:2;58;5;45m" "$e[0m"
echo "== 16 colours"
for i in {0..7}; do printf '%s  %d  ' "$e[4${i}m" "$i"; done; printf '%s\n' "$e[0m"
for i in {0..7}; do printf '%s  %d  ' "$e[10${i}m" "$((i+8))"; done; printf '%s\n' "$e[0m"
echo "== 256 colours"
for i in {16..51}; do printf '%s ' "$e[48;5;${i}m"; done; printf '%s\n' "$e[0m"
for i in {232..255}; do printf '%s ' "$e[48;5;${i}m"; done; printf '%s\n' "$e[0m"
echo "== truecolor ramp"
for i in {0..63}; do printf '%s ' "$e[48;2;$((i*4));$((255-i*4));180m"; done; printf '%s\n' "$e[0m"
echo "== box drawing / blocks / powerline / braille"
printf '┌─┬─┐ ╔═╦═╗ ┏━┳━┓ ╭─╮\n├─┼─┤ ╠═╬═╣ ┣━╋━┫ ╰─╯\n└─┴─┘ ╚═╩═╝ ┗━┻━┛\n'
printf '▀▄█▌▐ ░▒▓ ▖▗▘▙▚▛▜▝▞▟\n'
printf '     ⠁⠃⠇⡇⣇⣧⣷⣿\n'
echo "== cursor styles (DECSCUSR): block, beam, underline"
printf '%s' "$e[2 q"; sleep 0.4; printf '%s' "$e[6 q"; sleep 0.4; printf '%s' "$e[4 q"; sleep 0.4; printf '%s\n' "$e[0 q"
