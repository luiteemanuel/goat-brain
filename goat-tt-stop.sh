#!/bin/bash
# Push-to-talk: parar + transcrever + digitar no campo focado (on-release)
IPCSH="$(dirname "$0")/goat-ipc.sh"
"$IPCSH" ptt_release
