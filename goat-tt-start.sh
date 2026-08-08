#!/bin/bash
# Push-to-talk: iniciar gravação (on-press do bind no niri)
IPCSH="$(dirname "$0")/goat-ipc.sh"
"$IPCSH" ptt_press
