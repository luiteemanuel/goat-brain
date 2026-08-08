#!/bin/bash
# goat-ipc.sh <comando> — envia um comando ao Goat Reunião via socket unix.
# Comandos: ptt_press | ptt_release | meeting_toggle | show | quit
CMD="${1:?uso: goat-ipc.sh <ptt_press|ptt_release|meeting_toggle|show|quit>}"
printf '%s\n' "$CMD" | python3 -c '
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect("/tmp/goat-reuniao.sock")
s.sendall(sys.stdin.buffer.read())
s.close()
' 2>/dev/null || exit 1
