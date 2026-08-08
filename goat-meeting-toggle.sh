#!/bin/bash
# Liga/desliga a transcrição de reunião (toggle)
IPCSH="$(dirname "$0")/goat-ipc.sh"
"$IPCSH" meeting_toggle
